use std::
{
    collections::{HashMap, HashSet},
    fs,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::
{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use base64::prelude::*;
use serde::Deserialize;
use serde_json::json;
use tempfile::Builder as TempBuilder;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::
{
    error::{AppError, DatabaseErrorCode, ProjectErrorCode}, model::project::{ProjectDetailsResponse, ProjectMetrics, ProjectSourceType}, services::
    {
        crypto_service, database_service, deployment_orchestrator::DeploymentOrchestrator, docker_service, github_service, jwt::Claims, project_service, validation_service
    }, sse::types::DeploymentStage, state::AppState
};

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Deserialize)]
pub struct DeployPayload
{
    project_name: String,
    image_url: Option<String>,
    github_repo_url: Option<String>,
    github_branch: Option<String>,
    github_root_dir: Option<String>,
    participants: Vec<String>,
    env_vars: Option<HashMap<String, String>>,
    persistent_volume_path: Option<String>,
    create_database: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateEnvPayload
{
    env_vars: HashMap<String, String>,
}

#[derive(Deserialize)]
pub struct UpdateImagePayload
{
    new_image_url: String,
}

#[derive(Deserialize)]
pub struct ParticipantPayload
{
    participant_id: String,
}

// ============================================================================
// Internal Types
// ============================================================================

#[derive(Clone, Copy)]
enum ProjectAction
{
    Start,
    Stop,
    Restart,
}

impl ProjectAction
{
    async fn execute(
        self,
        docker: bollard::Docker,
        container_name: String,
    ) -> Result<(), AppError>
    {
        match self
        {
            Self::Start => docker_service::start_container_by_name(&docker, &container_name).await,
            Self::Stop => docker_service::stop_container_by_name(&docker, &container_name).await,
            Self::Restart => docker_service::restart_container_by_name(&docker, &container_name).await,
        }
    }
}

struct DeploymentSource
{
    source_type: ProjectSourceType,
    source_url: String,
    image_tag: String,
}

struct BlueGreenDeployment
{
    old_container_name: String,
    new_container_name: String,
    new_image_tag: String,
    new_image_digest: String,
}

// ============================================================================
// Public Handlers
// ============================================================================

pub async fn deploy_project_handler(
    State(state): State<AppState>,
    claims: Claims,
    Json(mut payload): Json<DeployPayload>,
) -> Result<impl IntoResponse, AppError>
{
    let mut orchestrator = DeploymentOrchestrator::for_creation
    (
        &state,
        payload.project_name.clone(),
        claims.sub.clone(),
    );
    
    orchestrator.emit_stage(DeploymentStage::Started).await;

    orchestrator.with_stage
    (
        DeploymentStage::ValidatingInput,
        "Input validation",
        validate_deploy_payload(&mut payload),
    ).await?;

    
    let user_login = claims.sub;

    orchestrator.with_stage
    (
        DeploymentStage::ValidatingInput,
        "Preconditions check",
        check_deployment_preconditions(&state, &user_login, &payload),
    ).await?;

    let participants = prepare_participants(payload.participants.clone(), &user_login)?;

    let deployment_source = prepare_deployment_source_with_events
    (
        &state, 
        &payload, 
        &orchestrator
    ).await?;

    let deployed_image_digest = orchestrator.with_stage
    (
        DeploymentStage::GettingImageDigest,
        "Image digest retrieval",
        get_image_digest(&state, &deployment_source.image_tag),
    ).await?;

    let container_name = format!("{}-{}", state.config.app_prefix, payload.project_name);
    
    let volume_name = orchestrator.with_stages
    (
        DeploymentStage::CreatingContainer,
        DeploymentStage::ContainerCreated,
        "Container creation",
        create_container_with_rollback
        (
            &state,
            &container_name,
            &payload.project_name,
            &deployed_image_digest,
            &payload.env_vars,
            &payload.persistent_volume_path,
            &deployment_source.image_tag,
        ),
    ).await?;

    if let Err(e) = orchestrator.with_stages
    (
        DeploymentStage::WaitingHealthCheck,
        DeploymentStage::HealthCheckPassed,
        "Health check",
        wait_for_container_health(&state, &container_name, 10),
    ).await
    {
        warn!("Health check failed : {}, rolling back container '{}'", e, container_name);
        let _ = docker_service::remove_container(&state.docker_client, &container_name).await;
        if let Some(volume_name) = &volume_name
        {
            let _ = docker_service::remove_volume_by_name(&state.docker_client, volume_name).await?;
        }
        remove_image_best_effort(&state, &deployed_image_digest).await;
    }

    let new_project = persist_project_with_rollback_and_events(
        &state,
        &mut orchestrator,
        &payload,
        &user_login,
        &container_name,
        &deployment_source,
        &deployed_image_digest,
        &volume_name,
        &participants,
    ).await?;

    orchestrator.emit_completed(container_name).await;

    info!(
        "Project '{}' by user '{}' created successfully.",
        payload.project_name, user_login
    );

    Ok(create_deploy_response(new_project, participants))
}

pub async fn purge_project_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, AppError>
{
    let user_login = claims.sub;
    info!("User '{}' initiated purge for project ID: {}", user_login, project_id);

    let project = get_project_for_owner(&state, project_id, &user_login, claims.is_admin).await?;

    deprovision_linked_database(&state, project_id, &user_login, claims.is_admin).await?;

    docker_service::remove_container(&state.docker_client, &project.container_name).await?;

    remove_persistent_volume(&state, &project).await?;

    remove_image_best_effort(&state, &project.deployed_image_tag).await;

    project_service::delete_project_by_id(&state.db_pool, project.id).await?;

    info!("Successfully purged project '{}' for user '{}'.", project.name, user_login);

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "message": "Project purged successfully."
        })),
    ))
}

pub async fn list_owned_projects_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<impl IntoResponse, AppError>
{
    let user_login = claims.sub;
    info!("Fetching owned projects for user '{}'", user_login);
    
    let projects = project_service::get_projects_by_owner(&state.db_pool, &user_login).await?;
    
    Ok((StatusCode::OK, Json(json!({ "projects": projects }))))
}

pub async fn list_participating_projects_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<impl IntoResponse, AppError>
{
    let user_login = claims.sub;
    info!("Fetching projects where user '{}' is a participant", user_login);
    
    let projects = project_service::get_participating_projects(&state.db_pool, &user_login).await?;
    
    Ok((StatusCode::OK, Json(json!({ "projects": projects }))))
}

pub async fn get_project_details_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, AppError>
{
    let user_login = claims.sub;
    debug!("User '{}' fetching details for project ID: {}", user_login, project_id);

    let project = get_project_for_user(&state, project_id, &user_login, claims.is_admin).await?;

    let mut project_data = project;
    decrypt_project_env_vars(&mut project_data, &state.config.encryption_key)?;

    let database_details = get_database_details(&state, project_data.id).await?;
    let participants = project_service::get_project_participants(&state.db_pool, project_data.id).await?;

    let response = ProjectDetailsResponse
    {
        project: project_data,
        participants,
        database: database_details,
    };

    Ok((StatusCode::OK, Json(json!({ "project": response }))))
}

pub async fn get_project_status_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, AppError>
{
    let project = get_project_for_user(&state, project_id, &claims.sub, claims.is_admin).await?;
    
    let status = docker_service::get_container_status(&state.docker_client, &project.container_name).await?;
    
    Ok(Json(json!({ "status": status.and_then(|s| s.status) })))
}

pub async fn start_project_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, AppError>
{
    project_control_handler(state, claims, project_id, ProjectAction::Start).await
}

pub async fn stop_project_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, AppError>
{
    project_control_handler(state, claims, project_id, ProjectAction::Stop).await
}

pub async fn restart_project_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, AppError>
{
    project_control_handler(state, claims, project_id, ProjectAction::Restart).await
}

pub async fn get_project_logs_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, AppError>
{
    let project = get_project_for_user(&state, project_id, &claims.sub, claims.is_admin).await?;
    
    let logs = docker_service::get_container_logs(&state.docker_client, &project.container_name, "200").await?;
    
    Ok(Json(json!({ "logs": logs })))
}

pub async fn get_project_metrics_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
) -> Result<Json<ProjectMetrics>, AppError>
{
    let project = get_project_for_user(&state, project_id, &claims.sub, claims.is_admin).await?;
    
    debug!("Fetching metrics for container '{}' (Project ID: {})", project.container_name, project.id);
    
    let metrics = docker_service::get_container_metrics(&state.docker_client, &project.container_name).await?;
    
    Ok(Json(metrics))
}

pub async fn update_project_image_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
    Json(payload): Json<UpdateImagePayload>,
) -> Result<impl IntoResponse, AppError>
{
    let user_login = &claims.sub;
    info!("User '{}' initiated blue-green image update for project ID: {}", user_login, project_id);

    let project = get_project_for_user(&state, project_id, user_login, claims.is_admin).await?;

    validate_project_source(&project.source, ProjectSourceType::Direct, "Image update")?;

    let orchestrator = DeploymentOrchestrator::for_update
    (
        &state,
        project.name.clone(),
        user_login.to_string(),
        project.id,
    );

    orchestrator.emit_stage(DeploymentStage::Started).await;

    let deployment = prepare_blue_green_deployment_with_events(
        &state,
        &orchestrator,
        &project,
        &payload.new_image_url,
        None,
    ).await?;

    if project.deployed_image_digest == deployment.new_image_digest
    {
        info!
        (
            "Project '{}' is already running the latest version of '{}'",
            project.name, payload.new_image_url
        );
        return Ok(create_no_change_response("The project is already running the latest version of the image."));
    }

    let env_vars = get_decrypted_env_vars(&project, &state.config.encryption_key)?;

    execute_blue_green_deployment_with_events(
        &state,
        &orchestrator,
        &project,
        &deployment,
        env_vars.as_ref(),
        &deployment.new_image_tag,
    ).await?;

    orchestrator.emit_completed(deployment.new_container_name).await;
    Ok(create_success_response("Project image updated successfully without downtime."))
}

pub async fn rebuild_project_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, AppError>
{
    let user_login = &claims.sub;
    info!("User '{}' initiated source rebuild for project ID: {}", user_login, project_id);

    let project = get_project_for_user(&state, project_id, user_login, claims.is_admin).await?;

    validate_project_source(&project.source, ProjectSourceType::Github, "Source rebuild")?;

    let orchestrator = DeploymentOrchestrator::for_update
    (
        &state,
        project.name.clone(),
        user_login.to_string(),
        project.id,
    );

    orchestrator.emit_stage(DeploymentStage::Started).await;

    let new_image_tag = build_image_from_github_source_with_events(
        &state,
        &orchestrator,
        &project.name,
        &project.source_url,
        project.source_branch.as_deref(),
        project.source_root_dir.as_deref(),
    ).await?;

    let deployment = prepare_blue_green_deployment_with_events(
        &state,
        &orchestrator,
        &project,
        &new_image_tag,
        Some(&project.deployed_image_tag),
    ).await?;

    if project.deployed_image_digest == deployment.new_image_digest
    {
        info!
        (
            "Project '{}' source is already up to date (digest: {})",
            project.name, project.deployed_image_digest
        );
        let _ = docker_service::remove_image(&state.docker_client, &new_image_tag).await;
        return Ok(create_no_change_response("The project source is already up to date."));
    }

    let env_vars = get_decrypted_env_vars(&project, &state.config.encryption_key)?;

    execute_blue_green_deployment_with_events(
        &state,
        &orchestrator,
        &project,
        &deployment,
        env_vars.as_ref(),
        &project.deployed_image_tag,
    ).await?;

    orchestrator.emit_completed(deployment.new_container_name).await;

    Ok(create_success_response("Project rebuilt and updated successfully from the latest source."))
}

pub async fn add_participant_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
    Json(payload): Json<ParticipantPayload>,
) -> Result<impl IntoResponse, AppError>
{
    let user_login = &claims.sub;
    info!(
        "User '{}' trying to add participant '{}' to project {}",
        user_login, payload.participant_id, project_id
    );

    let project = get_project_for_owner(&state, project_id, user_login, claims.is_admin).await?;

    if project.owner == payload.participant_id
    {
        return Err(ProjectErrorCode::OwnerCannotBeParticipant.into());
    }

    project_service::add_participant_to_project(&state.db_pool, project_id, &payload.participant_id).await?;

    info!("Participant '{}' added successfully to project {}", payload.participant_id, project_id);
    
    Ok((
        StatusCode::CREATED,
        Json(json!({"status": "success", "message": "Participant added."})),
    ))
}

pub async fn remove_participant_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path((project_id, participant_id)): Path<(i32, String)>,
) -> Result<impl IntoResponse, AppError>
{
    let user_login = &claims.sub;
    info!(
        "User '{}' trying to remove participant '{}' from project {}",
        user_login, participant_id, project_id
    );

    get_project_for_owner(&state, project_id, user_login, claims.is_admin).await?;

    project_service::remove_participant_from_project(&state.db_pool, project_id, &participant_id).await?;

    info!("Participant '{}' removed successfully from project {}", participant_id, project_id);
    
    Ok((
        StatusCode::OK,
        Json(json!({"status": "success", "message": "Participant removed."})),
    ))
}

pub async fn update_env_vars_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
    Json(payload): Json<UpdateEnvPayload>,
) -> Result<impl IntoResponse, AppError>
{
    let user_login = &claims.sub;
    info!("User '{}' initiated blue-green env var update for project ID: {}", user_login, project_id);

    validation_service::validate_env_vars(&payload.env_vars)?;

    let project = get_project_for_user(&state, project_id, user_login, claims.is_admin).await?;

    let deployment = create_blue_green_deployment_for_env_update(&state, &project);

    execute_env_vars_blue_green_deployment(
        &state,
        &project,
        &deployment,
        &payload.env_vars,
    ).await?;

    Ok(create_success_response("Environment variables updated successfully. The project has been restarted."))
}

// ============================================================================
// Private Helper Functions - Validation
// ============================================================================

async fn validate_deploy_payload(payload: &mut DeployPayload) -> Result<(), AppError>
{
    payload.project_name = validation_service::validate_project_name(&payload.project_name)?;

    if let Some(vars) = &payload.env_vars
    {
        validation_service::validate_env_vars(vars)?;
    }

    if let Some(path) = &payload.persistent_volume_path
    {
        validation_service::validate_volume_path(path)?;
    }

    if let Some(root_dir) = &payload.github_root_dir
    {
        validation_service::validate_source_root_dir(root_dir)?;
    }

    Ok(())
}

fn validate_project_source(
    actual: &ProjectSourceType,
    expected: ProjectSourceType,
    operation: &str,
) -> Result<(), AppError>
{
    if !matches!(actual, t if *t == expected)
    {
        let source_name = match expected
        {
            ProjectSourceType::Direct => "direct",
            ProjectSourceType::Github => "github",
        };
        
        return Err(AppError::BadRequest(
            format!("{} is only supported for '{}' source projects.", operation, source_name)
        ));
    }
    
    Ok(())
}

// ============================================================================
// Private Helper Functions - Preconditions & Preparation
// ============================================================================

async fn check_deployment_preconditions(
    state: &AppState,
    user_login: &str,
    payload: &DeployPayload,
) -> Result<(), AppError>
{
    if project_service::check_owner_exists(&state.db_pool, user_login).await?
    {
        return Err(ProjectErrorCode::OwnerAlreadyExists.into());
    }

    if project_service::check_project_name_exists(&state.db_pool, &payload.project_name).await?
    {
        return Err(ProjectErrorCode::ProjectNameTaken.into());
    }

    if payload.create_database.unwrap_or(false)
        && database_service::check_database_exists_for_owner(&state.db_pool, user_login).await?
    {
        return Err(AppError::DatabaseError(DatabaseErrorCode::DatabaseAlreadyExists));
    }

    Ok(())
}

fn prepare_participants(
    participants: Vec<String>,
    user_login: &str,
) -> Result<Vec<String>, AppError>
{
    let participants_set: HashSet<String> = participants.into_iter().collect();
    
    if participants_set.contains(user_login)
    {
        return Err(ProjectErrorCode::OwnerCannotBeParticipant.into());
    }
    
    Ok(participants_set.into_iter().collect())
}

async fn prepare_deployment_source_with_events(
    state: &AppState,
    payload: &DeployPayload,
    orchestrator: &DeploymentOrchestrator<'_>,
) -> Result<DeploymentSource, AppError>
{
    if let Some(image_url) = &payload.image_url
    {
        let tag = prepare_direct_source_with_events(state, image_url, orchestrator).await?;
        return Ok(DeploymentSource
        {
            source_type: ProjectSourceType::Direct,
            source_url: image_url.clone(),
            image_tag: tag,
        });
    }

    if let Some(github_repo_url) = &payload.github_repo_url
    {
        let tag = build_image_from_github_source_with_events(
            state,
            orchestrator,
            &payload.project_name,
            github_repo_url,
            payload.github_branch.as_deref(),
            payload.github_root_dir.as_deref(),
        ).await?;
        
        return Ok(DeploymentSource
        {
            source_type: ProjectSourceType::Github,
            source_url: github_repo_url.clone(),
            image_tag: tag,
        });
    }

    Err(AppError::BadRequest("You must provide either an 'image_url' or a 'github_repo_url'.".to_string()))
}

// ============================================================================
// Private Helper Functions - GitHub Operations
// ============================================================================

async fn build_image_from_github_source_with_events
(
    state: &AppState,
    orchestrator: &DeploymentOrchestrator<'_>,
    project_name: &str,
    repo_url: &str,
    branch: Option<&str>,
    root_dir: Option<&str>,
) -> Result<String, AppError>
{
    info!(
        "Building from GitHub source for project '{}'. Repo: '{}', Branch: {:?}, Root Dir: {:?}",
        project_name, repo_url, branch, root_dir
    );

    let temp_dir = TempBuilder::new()
        .prefix("hangar-build-")
        .tempdir()
        .map_err(|_| AppError::InternalServerError)?;

    orchestrator.with_stages
    (
        DeploymentStage::CloningRepository 
        {
            repo_url: repo_url.to_string(),
        },
        DeploymentStage::RepositoryCloned,
        "Repository clone",
        clone_repository(state, repo_url, temp_dir.path(), branch),
    ).await?;

    create_dockerfile(&state.config.build_base_image, root_dir, temp_dir.path())?;

    let tarball = docker_service::create_tarball(temp_dir.path())?;
    let image_tag = generate_image_tag(project_name);
    
    orchestrator.with_stages
    (
        DeploymentStage::BuildingImage,
        DeploymentStage::ImageBuilt,
        "Image build",
        docker_service::build_image_from_tar(&state.docker_client, tarball, &image_tag),
    ).await?;

    if let Err(scan_error) = orchestrator.with_stages
    (
        DeploymentStage::ScanningImage,
        DeploymentStage::ImageScanned,
        "Image scan",
        docker_service::scan_image_with_grype(&image_tag, &state.config),
    ).await
    {
        warn!("Image scan failed, rolling back by removing built image '{}'", image_tag);
        let _ = docker_service::remove_image(&state.docker_client, &image_tag).await;
        return Err(scan_error);
    }

    Ok(image_tag)
}

async fn clone_repository(
    state: &AppState,
    repo_url: &str,
    destination: &std::path::Path,
    branch: Option<&str>,
) -> Result<(), AppError>
{
    match github_service::clone_repo(repo_url, destination, None, branch).await
    {
        Ok(_) =>
        {
            info!("Successfully cloned public repository '{}'", repo_url);
            Ok(())
        }
        Err(AppError::ProjectError(ProjectErrorCode::GithubAccountNotLinked))
        | Err(AppError::ProjectError(ProjectErrorCode::InvalidGithubUrl)) =>
        {
            warn!(
                "Public clone failed for '{}'. Assuming private repo and trying authenticated clone.",
                repo_url
            );
            clone_private_repository(state, repo_url, destination, branch).await
        }
        Err(e) => Err(e),
    }
}

async fn clone_private_repository(
    state: &AppState,
    repo_url: &str,
    destination: &std::path::Path,
    branch: Option<&str>,
) -> Result<(), AppError>
{
    let (github_owner, repo_name) = github_service::extract_repo_owner_and_name(repo_url).await?;
    
    let installation_id = github_service::get_installation_id_by_user(
        &state.http_client,
        &state.config,
        &github_owner,
    ).await?;
    
    let token = github_service::get_installation_token(
        installation_id,
        &state.http_client,
        &state.config,
    ).await?;
    
    github_service::check_repo_accessibility(
        &state.http_client,
        &token,
        &github_owner,
        &repo_name,
    ).await?;
    
    github_service::clone_repo(repo_url, destination, Some(&token), branch).await?;
    
    info!("Successfully cloned private repository '{}' using GitHub App token", repo_url);
    
    Ok(())
}

fn create_dockerfile(
    base_image: &str,
    root_dir: Option<&str>,
    temp_dir: &std::path::Path,
) -> Result<(), AppError>
{
    let dockerfile_content = format!(
        "FROM {}\nCOPY --chown=appuser:appgroup . /var/www/html/\n",
        base_image
    );

    let dockerfile_content = if let Some(dir) = root_dir 
    {
        format!(
            "{}ENV HANGAR_WEBROOT_DIR=/var/www/html/{}\n",
            dockerfile_content,
            dir
        )
    } 
    else 
    {
        dockerfile_content
    };
    
    fs::write(temp_dir.join("Dockerfile"), dockerfile_content)
        .map_err(|_| AppError::InternalServerError)?;
    
    Ok(())
}

// ============================================================================
// Private Helper Functions - Direct Source Operations
// ============================================================================

async fn prepare_direct_source_with_events
(
    state: &AppState, 
    image_url: &str,
    orchestrator: &DeploymentOrchestrator<'_>,
) -> Result<String, AppError>
{
    info!("Preparing 'direct' source from image '{}'", image_url);
    
    validation_service::validate_image_url(image_url)?;

    orchestrator.with_stages
    (
        DeploymentStage::PullingImage 
        {
            image_url: image_url.to_string(),
        },
        DeploymentStage::ImagePulled,
        "Image pull",
        pull_image_with_error_handling(state, image_url),
    ).await?;

    orchestrator.with_stages
    (
        DeploymentStage::ScanningImage,
        DeploymentStage::ImageScanned,
        "Image scan",
        scan_image_with_rollback(state, image_url),
    ).await?;


    Ok(image_url.to_string())
}

async fn pull_image_with_error_handling(state: &AppState, image_url: &str) -> Result<(), AppError>
{
    match docker_service::pull_image(&state.docker_client, image_url, None).await
    {
        Ok(_) =>
        {
            info!("Successfully pulled public image '{}'", image_url);
            Ok(())
        }
        Err(e) =>
        {
            if image_url.starts_with("ghcr.io/")
                && let bollard::errors::Error::DockerResponseServerError { status_code, .. } = &e
                    && (*status_code == 401 || *status_code == 403)
                    {
                        warn!("Failed to pull private image from ghcr.io: {}", image_url);
                        return Err(ProjectErrorCode::GithubPackageNotPublic.into());
                    }

            error!("Failed to pull image '{}': {}", image_url, e);
            Err(ProjectErrorCode::ImagePullFailed.into())
        }
    }
}

async fn scan_image_with_rollback(state: &AppState, image_url: &str) -> Result<(), AppError>
{
    if let Err(scan_error) = docker_service::scan_image_with_grype(image_url, &state.config).await
    {
        warn!("Image scan failed, rolling back by removing pulled image '{}'", image_url);
        let _ = docker_service::remove_image(&state.docker_client, image_url).await;
        return Err(scan_error);
    }
    
    Ok(())
}

// ============================================================================
// Private Helper Functions - Container & Image Operations
// ============================================================================

async fn create_container_with_rollback(
    state: &AppState,
    container_name: &str,
    project_name: &str,
    image_digest: &str,
    env_vars: &Option<HashMap<String, String>>,
    persistent_volume_path: &Option<String>,
    image_tag: &str,
) -> Result<Option<String>, AppError>
{
    match docker_service::create_project_container(
        &state.docker_client,
        container_name,
        project_name,
        image_digest,
        &state.config,
        env_vars,
        persistent_volume_path,
    ).await
    {
        Ok(volume_name) => Ok(volume_name),
        Err(e) =>
        {
            warn!("Container creation failed, rolling back image '{}'", image_tag);
            let _ = docker_service::remove_image(&state.docker_client, image_tag).await;
            Err(e)
        }
    }
}

async fn get_image_digest(state: &AppState, image_tag: &str) -> Result<String, AppError>
{
    match docker_service::get_image_digest(&state.docker_client, image_tag).await
    {
        Ok(Some(digest)) => Ok(digest),
        Ok(None) =>
        {
            error!("Image '{}' not found when retrieving digest", image_tag);
            remove_image_best_effort(state, image_tag).await;
            Err(AppError::InternalServerError)
        }
        Err(e) =>
        {
            error!("Failed to retrieve image digest for '{}': {}", image_tag, e);
            remove_image_best_effort(state, image_tag).await;
            Err(AppError::InternalServerError)
        }
    }
}

fn generate_image_tag(project_name: &str) -> String
{
    format!(
        "hangar-local/{}:{}",
        project_name,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    )
}

async fn wait_for_container_health(
    state: &AppState,
    container_name: &str,
    max_attempts: u32,
) -> Result<(), AppError>
{
    info!("Waiting for new container '{}' to be healthy...", container_name);

    for _ in 0..max_attempts
    {
        if is_container_healthy(state, container_name).await?
        {
            info!("Container '{}' is healthy", container_name);
            return Ok(());
        }
        sleep(Duration::from_secs(1)).await;
    }

    error!("Container '{}' did not become healthy in time", container_name);
    Err(AppError::InternalServerError)
}

async fn is_container_healthy(state: &AppState, container_name: &str) -> Result<bool, AppError>
{
    if let Ok(Some(details)) = docker_service::inspect_container_details(&state.docker_client, container_name).await
        && let Some(container_state) = details.state
        {
            return Ok(container_state.running.unwrap_or(false));
        }
    Ok(false)
}

async fn remove_image_best_effort(state: &AppState, image_tag: &str)
{
    match docker_service::remove_image(&state.docker_client, image_tag).await
    {
        Ok(_) => info!("Successfully removed image '{}'", image_tag),
        Err(e) => warn!(
            "Failed to remove image '{}': {}. Manual cleanup may be required.",
            image_tag, e
        ),
    }
}

// ============================================================================
// Private Helper Functions - Database Operations
// ============================================================================

async fn persist_project_with_rollback_and_events(
    state: &AppState,
    orchestrator: &mut DeploymentOrchestrator<'_>,
    payload: &DeployPayload,
    user_login: &str,
    container_name: &str,
    deployment_source: &DeploymentSource,
    deployed_image_digest: &str,
    volume_name: &Option<String>,
    participants: &[String],
) -> Result<crate::model::project::Project, AppError>
{
    let mut tx = state.db_pool.begin()
        .await
        .map_err(|_| AppError::InternalServerError)?;

    let db_operations = async 
    {
        let new_project = create_project_in_transaction(
            &mut tx,
            state,
            payload,
            user_login,
            container_name,
            deployment_source,
            deployed_image_digest,
            volume_name,
        ).await?;

        orchestrator.set_project_id(new_project.id);

        if payload.create_database.unwrap_or(false)
        {
            orchestrator.with_stages
            (
                DeploymentStage::ProvisioningDatabase,
                DeploymentStage::DatabaseProvisioned,
                "Database provisioning",
                provision_database_in_transaction(&mut tx, state, user_login, new_project.id),
            ).await?;
        }

        add_participants_in_transaction(&mut tx, new_project.id, participants).await?;

        Ok(new_project)
    };

    match db_operations.await 
    {
        Ok(project) => 
        {
            tx.commit().await.map_err(|_| AppError::InternalServerError)?;
            Ok(project)
        }
        Err(e) => 
        {
            warn!("Database transaction failed. Rolling back Docker resources for container '{}'...", container_name);
            let _ = docker_service::remove_container(&state.docker_client, container_name).await;
            if let Some(vol) = volume_name 
            {
                let _ = docker_service::remove_volume_by_name(&state.docker_client, vol).await;
            }
            remove_image_best_effort(state, &deployment_source.image_tag).await;
            
            Err(e)
        }
    }
}

async fn create_project_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &AppState,
    payload: &DeployPayload,
    user_login: &str,
    container_name: &str,
    deployment_source: &DeploymentSource,
    deployed_image_digest: &str,
    volume_name: &Option<String>,
) -> Result<crate::model::project::Project, AppError>
{
    project_service::create_project(
        tx,
        &payload.project_name,
        user_login,
        container_name,
        deployment_source.source_type,
        &deployment_source.source_url,
        &payload.github_branch,
        &payload.github_root_dir,
        &deployment_source.image_tag,
        deployed_image_digest,
        &payload.env_vars,
        &payload.persistent_volume_path,
        volume_name,
        &state.config.encryption_key,
    ).await.map_err(|e| 
    {
        error!("Failed to persist project in DB: {}", e);
        e
    })
}

async fn provision_database_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &AppState,
    user_login: &str,
    project_id: i32,
) -> Result<(), AppError>
{
    if let Err(db_error) = database_service::provision_and_link_database_tx(
        tx,
        &state.mariadb_pool,
        user_login,
        project_id,
        &state.config.encryption_key,
    ).await
    {
        warn!("Database provisioning failed during project creation, rolling back transaction...");
        Err(db_error)
    }
    else
    {
        Ok(())
    }
}

async fn add_participants_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: i32,
    participants: &[String],
) -> Result<(), AppError>
{
    if let Err(e) = project_service::add_project_participants(tx, project_id, participants).await
    {
        warn!("Failed to add participants, rolling back transaction...");
        Err(e)
    }
    else
    {
        Ok(())
    }
}


async fn deprovision_linked_database(
    state: &AppState,
    project_id: i32,
    user_login: &str,
    is_admin: bool,
) -> Result<(), AppError>
{
    if let Some(db) = database_service::get_database_by_project_id(&state.db_pool, project_id).await?
    {
        info!("Project has a linked database (ID: {}). Deprovisioning it.", db.id);
        
        database_service::deprovision_database(
            &state.db_pool,
            &state.mariadb_pool,
            db.id,
            user_login,
            is_admin,
        ).await?;
        
        info!("Linked database deprovisioned successfully.");
    }
    
    Ok(())
}

async fn remove_persistent_volume(
    state: &AppState,
    project: &crate::model::project::Project,
) -> Result<(), AppError>
{
    if project.persistent_volume_path.is_some()
    {
        let volume_name = project.volume_name
            .as_ref()
            .ok_or_else(||
            {
                error!("Project '{}' has a persistent volume path but no volume name recorded", project.name);
                AppError::InternalServerError
            })?;

        docker_service::remove_volume_by_name(&state.docker_client, volume_name).await?;
    }
    
    Ok(())
}

async fn get_database_details(
    state: &AppState,
    project_id: i32,
) -> Result<Option<crate::model::database::DatabaseDetailsResponse>, AppError>
{
    match database_service::get_database_by_project_id(&state.db_pool, project_id).await?
    {
        Some(db) =>
        {
            let details = database_service::create_db_details_response(
                db,
                &state.config,
                &state.config.encryption_key,
            )?;
            Ok(Some(details))
        }
        None => Ok(None),
    }
}

// ============================================================================
// Private Helper Functions - Project Retrieval
// ============================================================================

async fn get_project_for_owner(
    state: &AppState,
    project_id: i32,
    user_login: &str,
    is_admin: bool,
) -> Result<crate::model::project::Project, AppError>
{
    project_service::get_project_by_id_and_owner(&state.db_pool, project_id, user_login, is_admin)
        .await?
        .ok_or_else(||
        {
            AppError::NotFound(format!(
                "Project with ID {} not found or you don't have access.",
                project_id
            ))
        })
}

async fn get_project_for_user(
    state: &AppState,
    project_id: i32,
    user_login: &str,
    is_admin: bool,
) -> Result<crate::model::project::Project, AppError>
{
    project_service::get_project_by_id_for_user(&state.db_pool, project_id, user_login, is_admin)
        .await?
        .ok_or_else(||
        {
            AppError::NotFound(format!(
                "Project with ID {} not found or you don't have access.",
                project_id
            ))
        })
}

// ============================================================================
// Private Helper Functions - Project Control
// ============================================================================

async fn project_control_handler(
    state: AppState,
    claims: Claims,
    project_id: i32,
    action: ProjectAction,
) -> Result<impl IntoResponse, AppError>
{
    let project = get_project_for_user(&state, project_id, &claims.sub, claims.is_admin).await?;

    validate_container_exists_for_action(&state, &project, action).await?;

    action.execute(state.docker_client.clone(), project.container_name).await?;

    Ok(StatusCode::OK)
}

async fn validate_container_exists_for_action(
    state: &AppState,
    project: &crate::model::project::Project,
    action: ProjectAction,
) -> Result<(), AppError>
{
    let status = docker_service::get_container_status(&state.docker_client, &project.container_name).await?;

    if status.is_none() && matches!(action, ProjectAction::Start | ProjectAction::Restart)
    {
        warn!(
            "Container '{}' not found for project ID {}. It might be lost.",
            project.container_name, project.id
        );
        
        return Err(AppError::NotFound(format!(
            "Container for project '{}' seems to be lost. Please contact support or try to redeploy.",
            project.name
        )));
    }

    Ok(())
}

// ============================================================================
// Private Helper Functions - Blue-Green Deployment
// ============================================================================

async fn prepare_blue_green_deployment_with_events(
    state: &AppState,
    orchestrator: &DeploymentOrchestrator<'_>,
    project: &crate::model::project::Project,
    new_image_url: &str,
    old_image_tag: Option<&str>,
) -> Result<BlueGreenDeployment, AppError>
{
    if old_image_tag.is_none()
    {
        prepare_direct_source_with_events(state, new_image_url, orchestrator).await?;
    }

    let new_image_digest = orchestrator.with_stage
    (
        DeploymentStage::GettingImageDigest,
        "Image digest retrieval",
        get_image_digest(state, new_image_url),
    ).await?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    Ok(BlueGreenDeployment
    {
        old_container_name: project.container_name.clone(),
        new_container_name: format!("{}-{}-{}", state.config.app_prefix, project.name, timestamp),
        new_image_tag: new_image_url.to_string(),
        new_image_digest,
    })
}

fn create_blue_green_deployment_for_env_update(
    state: &AppState,
    project: &crate::model::project::Project,
) -> BlueGreenDeployment
{
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    BlueGreenDeployment
    {
        old_container_name: project.container_name.clone(),
        new_container_name: format!("{}-{}-{}", state.config.app_prefix, project.name, timestamp),
        new_image_tag: project.deployed_image_tag.clone(),
        new_image_digest: project.deployed_image_digest.clone(),
    }
}

async fn execute_blue_green_deployment_with_events(
    state: &AppState,
    orchestrator: &DeploymentOrchestrator<'_>,
    project: &crate::model::project::Project,
    deployment: &BlueGreenDeployment,
    env_vars: Option<&HashMap<String, String>>,
    old_image_to_cleanup: &str,
) -> Result<(), AppError>
{
    info!("Creating new container '{}' for project '{}'", deployment.new_container_name, project.name);

    orchestrator.with_stages
    (
        DeploymentStage::CreatingContainer,
        DeploymentStage::ContainerCreated,
        "New container creation",
        create_new_container_for_deployment(state, project, deployment, env_vars),
    ).await?;


    orchestrator.with_stages
    (
        DeploymentStage::WaitingHealthCheck,
        DeploymentStage::HealthCheckPassed,
        "Health check",
        wait_for_container_health(state, &deployment.new_container_name, 10),
    ).await.inspect_err(|_|
    {
        let docker = state.docker_client.clone();
        let container = deployment.new_container_name.clone();
        let image = deployment.new_image_tag.clone();
        
        tokio::spawn(async move
        {
            let _ = docker_service::remove_container(&docker, &container).await;
            let _ = docker_service::remove_image(&docker, &image).await;
        });
    })?;

    update_project_metadata(state, project.id, deployment, &project.source).await
        .inspect_err(|_| 
        {
            error!("Failed to update project metadata. Rolling back new container...");
            
            let docker = state.docker_client.clone();
            let container = deployment.new_container_name.clone();
            let image = deployment.new_image_tag.clone();
            
            tokio::spawn(async move 
            {
                let _ = docker_service::remove_container(&docker, &container).await;
                let _ = docker_service::remove_image(&docker, &image).await;
            });
        })?;

    orchestrator.emit_stage(DeploymentStage::CleaningUp).await;
    cleanup_old_deployment(state, &deployment.old_container_name, old_image_to_cleanup).await;

    info!(
        "Project '{}' deployment completed successfully. New container is '{}'.",
        project.name, deployment.new_container_name
    );

    Ok(())
}

async fn create_new_container_for_deployment(
    state: &AppState,
    project: &crate::model::project::Project,
    deployment: &BlueGreenDeployment,
    env_vars: Option<&HashMap<String, String>>,
) -> Result<(), AppError>
{
    let owned_env_vars: Option<HashMap<String, String>> = env_vars.cloned();

    match docker_service::create_project_container(
        &state.docker_client,
        &deployment.new_container_name,
        &project.name,
        &deployment.new_image_digest,
        &state.config,
        &owned_env_vars,
        &project.persistent_volume_path,
    ).await
    {
        Ok(volume) => Ok(volume),
        Err(e) =>
        {
            error!("Failed to create new container for project '{}'. Aborting update.", project.name);
            let _ = docker_service::remove_image(&state.docker_client, &deployment.new_image_tag).await;
            Err(e)
        }
    }?;

    Ok(())
}

async fn update_project_metadata(
    state: &AppState,
    project_id: i32,
    deployment: &BlueGreenDeployment,
    project_source: &ProjectSourceType,
) -> Result<(), AppError>
{
    project_service::update_project_container_name(
        &state.db_pool,
        project_id,
        &deployment.new_container_name,
    ).await?;

    project_service::update_project_image_and_digest(
        &state.db_pool,
        project_id,
        &deployment.new_image_tag,
        &deployment.new_image_digest,
    ).await?;

    if *project_source == ProjectSourceType::Direct
    {
        project_service::update_project_source_url(
            &state.db_pool,
            project_id,
            &deployment.new_image_tag,
        ).await?;
    }

    Ok(())
}

async fn cleanup_old_deployment(
    state: &AppState,
    old_container_name: &str,
    old_image_tag: &str,
)
{
    info!("Removing old container '{}'", old_container_name);
    
    if let Err(e) = docker_service::remove_container(&state.docker_client, old_container_name).await
    {
        warn!(
            "Could not remove old container '{}', but update is successful. Manual cleanup may be needed. Error: {}",
            old_container_name, e
        );
    }

    let docker_client = state.docker_client.clone();
    let old_image_tag_clone = old_image_tag.to_string();
    
    tokio::spawn(async move
    {
        if let Err(e) = docker_service::remove_image(&docker_client, &old_image_tag_clone).await
        {
            warn!("Could not remove old image '{}' in background: {}", old_image_tag_clone, e);
        }
    });
}

async fn execute_env_vars_blue_green_deployment(
    state: &AppState,
    project: &crate::model::project::Project,
    deployment: &BlueGreenDeployment,
    env_vars: &HashMap<String, String>,
) -> Result<(), AppError>
{
    info!(
        "Creating new container '{}' for project '{}' with updated env vars",
        deployment.new_container_name, project.name
    );

    docker_service::create_project_container(
        &state.docker_client,
        &deployment.new_container_name,
        &project.name,
        &project.deployed_image_tag,
        &state.config,
        &Some(env_vars.clone()),
        &project.persistent_volume_path,
    ).await
    .inspect_err(|_|
    {
        error!("Failed to recreate container for project '{}' during env update. Aborting.", project.name);
    })?;

    wait_for_container_health(state, &deployment.new_container_name, 10).await
        .inspect_err(|_|
        {
            let docker = state.docker_client.clone();
            let container = deployment.new_container_name.clone();
            
            tokio::spawn(async move
            {
                let _ = docker_service::remove_container(&docker, &container).await;
            });
        })?;

    project_service::update_project_container_name(
        &state.db_pool,
        project.id,
        &deployment.new_container_name,
    ).await?;

    project_service::update_project_env_vars(
        &state.db_pool,
        project.id,
        env_vars,
        &state.config.encryption_key,
    ).await?;

    info!("Removing old container '{}'", deployment.old_container_name);
    
    if let Err(e) = docker_service::remove_container(&state.docker_client, &deployment.old_container_name).await
    {
        warn!(
            "Could not remove old container '{}', but update is successful. Manual cleanup may be needed. Error: {}",
            deployment.old_container_name, e
        );
    }

    info!(
        "Project '{}' environment variables updated successfully. New container is '{}'.",
        project.name, deployment.new_container_name
    );

    Ok(())
}

// ============================================================================
// Private Helper Functions - Encryption
// ============================================================================

fn decrypt_project_env_vars(
    project: &mut crate::model::project::Project,
    encryption_key: &[u8],
) -> Result<(), AppError>
{
    if let Some(env_vars_value) = &project.env_vars
    {
        let encrypted_vars: HashMap<String, String> = serde_json::from_value(env_vars_value.clone())
            .unwrap_or_default();
        
        let decrypted_vars = decrypt_env_vars(&encrypted_vars, encryption_key)?;
        
        project.env_vars = Some(serde_json::to_value(decrypted_vars).unwrap());
    }
    
    Ok(())
}

fn get_decrypted_env_vars(
    project: &crate::model::project::Project,
    encryption_key: &[u8],
) -> Result<Option<HashMap<String, String>>, AppError>
{
    if let Some(env_vars_value) = &project.env_vars
    {
        let encrypted_vars: HashMap<String, String> = serde_json::from_value(env_vars_value.clone())
            .unwrap_or_default();
        
        Ok(Some(decrypt_env_vars(&encrypted_vars, encryption_key)?))
    }
    else
    {
        Ok(None)
    }
}

fn decrypt_env_vars(
    encrypted_vars: &HashMap<String, String>,
    key: &[u8],
) -> Result<HashMap<String, String>, AppError>
{
    encrypted_vars
        .iter()
        .map(|(k, v_b64)|
        {
            let encrypted_val = BASE64_STANDARD
                .decode(v_b64)
                .map_err(|_| AppError::InternalServerError)?;
            
            let decrypted_val = crypto_service::decrypt(&encrypted_val, key)?;
            
            Ok((k.clone(), decrypted_val))
        })
        .collect()
}

// ============================================================================
// Private Helper Functions - Response Building
// ============================================================================

fn create_deploy_response(
    new_project: crate::model::project::Project,
    participants: Vec<String>,
) -> (StatusCode, Json<serde_json::Value>)
{
    let mut project_json = serde_json::to_value(new_project).unwrap_or(json!({}));
    
    if let Some(obj) = project_json.as_object_mut()
    {
        obj.insert("participants".to_string(), json!(participants));
    }

    let response_body = json!({ "project": project_json });
    
    (StatusCode::CREATED, Json(response_body))
}

fn create_no_change_response(message: &str) -> (StatusCode, Json<serde_json::Value>)
{
    (
        StatusCode::OK,
        Json(json!({
            "status": "no_change",
            "message": message
        })),
    )
}

fn create_success_response(message: &str) -> (StatusCode, Json<serde_json::Value>)
{
    (
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "message": message
        })),
    )
}