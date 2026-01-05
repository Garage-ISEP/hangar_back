use std::collections::HashMap;
use sqlx::{PgPool, Postgres, Transaction};
use tracing::{error, warn};
use crate::{error::{AppError, ProjectErrorCode}, model::project::{Project, ProjectSourceType}, services::crypto_service};
use base64::prelude::*;

pub async fn check_project_name_exists(pool: &PgPool, name: &str) -> Result<bool, AppError> 
{
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects WHERE name = $1")
        .bind(name)
        .fetch_one(pool)
        .await
        .map_err(|_| AppError::InternalServerError)?;
    Ok(count.0 > 0)
}

pub async fn check_owner_exists(pool: &PgPool, owner: &str) -> Result<bool, AppError> 
{
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects WHERE owner = $1")
        .bind(owner)
        .fetch_one(pool)
        .await
        .map_err(|_| AppError::InternalServerError)?;
    Ok(count.0 > 0)
}

pub async fn create_project<'a>(
    tx: &mut Transaction<'a, Postgres>,
    name: &str,
    owner: &str,
    container_name: &str,
    source_type: ProjectSourceType,
    source_url: &str,
    source_branch: &Option<String>,
    source_root_dir: &Option<String>,
    deployed_image_tag: &str,
    deployed_image_digest: &str,
    env_vars: &Option<HashMap<String, String>>,
    persistent_volume_path: &Option<String>,
    volume_name: &Option<String>,
    encryption_key: &[u8]
) -> Result<Project, AppError> 
{
    let encrypted_env_vars = match env_vars
    {
        Some(vars) => Some(encrypt_env_vars(vars, encryption_key)?),
        None => None,
    };

    let env_vars_json = encrypted_env_vars.as_ref().map(serde_json::to_value).transpose()
        .map_err(|_| AppError::InternalServerError)?;

    let project = sqlx::query_as::<_, Project>(
        "INSERT INTO projects (name, owner, container_name, source_type, source_url, source_branch, source_root_dir, deployed_image_tag, deployed_image_digest, env_vars, persistent_volume_path, volume_name)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         RETURNING id, name, owner, container_name, source_type, source_url, source_branch, source_root_dir, deployed_image_tag, deployed_image_digest, created_at, env_vars, persistent_volume_path, volume_name",
    )
    .bind(name)
    .bind(owner)
    .bind(container_name)
    .bind(source_type)
    .bind(source_url)
    .bind(source_branch)
    .bind(source_root_dir)
    .bind(deployed_image_tag)
    .bind(deployed_image_digest)
    .bind(env_vars_json)
    .bind(persistent_volume_path)
    .bind(volume_name)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e: sqlx::Error| 
    {
        error!("Failed to create project in DB: {}", e);
        if let Some(db_err) = e.as_database_error()
            && db_err.is_unique_violation() 
            {
                return AppError::ProjectError(ProjectErrorCode::ProjectNameTaken);
            }
        AppError::InternalServerError
    })?;

    Ok(project)
}

pub async fn delete_project_by_id(pool: &PgPool, project_id: i32) -> Result<(), AppError> 
{
    let result = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project_id)
        .execute(pool)
        .await
        .map_err(|e| 
        {
            error!("Failed to delete project with id '{}': {}", project_id, e);
            AppError::ProjectError(ProjectErrorCode::DeleteFailed)
        })?;

    if result.rows_affected() == 0 
    {
        return Err(AppError::NotFound(format!("Project with id {} not found for deletion.", project_id)));
    }

    Ok(())
}

const SELECT_PROJECT_FIELDS: &str = "SELECT id, name, owner, container_name, source_type, source_url, source_branch, source_root_dir, deployed_image_tag, deployed_image_digest, created_at, env_vars, persistent_volume_path, volume_name FROM projects";

pub async fn get_projects_by_owner(pool: &PgPool, owner: &str) -> Result<Vec<Project>, AppError> 
{
    let query = format!("{} WHERE owner = $1 ORDER BY created_at DESC", SELECT_PROJECT_FIELDS);
    sqlx::query_as::<_, Project>(&query)
        .bind(owner)
        .fetch_all(pool)
        .await
        .map_err(|e| 
        {
            error!("Failed to fetch projects for owner '{}': {}", owner, e);
            AppError::InternalServerError
        })
}

pub async fn get_project_by_id_and_owner(
    pool: &PgPool,
    project_id: i32,
    owner: &str,
    is_admin: bool,
) -> Result<Option<Project>, AppError> 
{
    if is_admin 
    {
        let query = format!("{} WHERE id = $1", SELECT_PROJECT_FIELDS);
        return sqlx::query_as::<_, Project>(&query)
            .bind(project_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| 
            {
                error!("Admin failed to fetch project by id {}: {}", project_id, e);
                AppError::InternalServerError
            });
    }

    let query = format!("{} WHERE id = $1 AND owner = $2", SELECT_PROJECT_FIELDS);
    sqlx::query_as::<_, Project>(&query)
        .bind(project_id)
        .bind(owner)
        .fetch_optional(pool)
        .await
        .map_err(|e| 
        {
            error!("Failed to fetch project by id {} and owner '{}': {}", project_id, owner, e);
            AppError::InternalServerError
        })
}

pub async fn get_participating_projects(pool: &PgPool, participant_id: &str) -> Result<Vec<Project>, AppError> 
{
    sqlx::query_as::<_, Project>(
        "SELECT p.id, p.name, p.owner, p.container_name, p.source_type, p.source_url, p.source_branch, p.source_root_dir, p.deployed_image_tag, p.deployed_image_digest, p.created_at, p.env_vars, p.persistent_volume_path, p.volume_name
         FROM projects p
         JOIN project_participants pp ON p.id = pp.project_id
         WHERE pp.participant_id = $1
         ORDER BY p.created_at DESC"
    )
        .bind(participant_id)
        .fetch_all(pool)
        .await
        .map_err(|e| 
        {
            error!("Failed to fetch participating projects for user '{}': {}", participant_id, e);
            AppError::InternalServerError
        })
}

pub async fn get_project_by_id_for_user(
    pool: &PgPool,
    project_id: i32,
    user_login: &str,
    is_admin: bool,
) -> Result<Option<Project>, AppError> 
{
    if is_admin 
    {
        return sqlx::query_as::<_, Project>(&format!("{} WHERE id = $1", SELECT_PROJECT_FIELDS))
            .bind(project_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| 
            {
                error!("Failed to fetch project {} for admin '{}': {}", project_id, user_login, e);
                AppError::InternalServerError
            });
    }

    sqlx::query_as::<_, Project>(
        "SELECT p.id, p.name, p.owner, p.container_name, p.source_type, p.source_url, p.source_branch, p.source_root_dir, p.deployed_image_tag, p.deployed_image_digest, p.created_at, p.env_vars, p.persistent_volume_path, p.volume_name
         FROM projects p
         LEFT JOIN project_participants pp ON p.id = pp.project_id
         WHERE p.id = $1 AND (p.owner = $2 OR pp.participant_id = $2)"
    )
        .bind(project_id)
        .bind(user_login)
        .fetch_optional(pool)
        .await
        .map_err(|e| 
        {
            error!("Failed to fetch project {} for user '{}': {}", project_id, user_login, e);
            AppError::InternalServerError
        })
}

pub async fn get_project_participants(pool: &PgPool, project_id: i32) -> Result<Vec<String>, AppError> 
{
    sqlx::query_scalar("SELECT participant_id FROM project_participants WHERE project_id = $1")
        .bind(project_id)
        .fetch_all(pool)
        .await
        .map_err(|e| 
        {
            error!("Failed to fetch participants for project {}: {}", project_id, e);
            AppError::InternalServerError
        })
}

pub async fn get_all_projects(pool: &PgPool) -> Result<Vec<Project>, AppError> 
{
    let query = format!("{} ORDER BY created_at DESC", SELECT_PROJECT_FIELDS);
    sqlx::query_as::<_, Project>(&query)
        .fetch_all(pool)
        .await
        .map_err(|e| 
        {
            error!("Failed to fetch all projects: {}", e);
            AppError::InternalServerError
        })
}


pub async fn add_project_participants<'a>(
    tx: &mut Transaction<'a, Postgres>,
    project_id: i32,
    participants: &[String],
) -> Result<(), AppError> 
{
    if participants.is_empty() 
    {
        return Ok(());
    }

    let mut query_builder = sqlx::QueryBuilder::new(
        "INSERT INTO project_participants (project_id, participant_id) "
    );

    query_builder.push_values(participants.iter(), |mut b, participant| 
    {
        b.push_bind(project_id)
         .push_bind(participant);
    });

    let query = query_builder.build();

    query.execute(&mut **tx).await.map_err(|e| 
    {
        error!("Failed to add participants for project {}: {}", project_id, e);
        AppError::InternalServerError
    })?;

    Ok(())
}


pub async fn add_participant_to_project(
    pool: &PgPool,
    project_id: i32,
    participant_id: &str,
) -> Result<(), AppError> 
{
    sqlx::query(
        "INSERT INTO project_participants (project_id, participant_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
    )
    .bind(project_id)
    .bind(participant_id)
    .execute(pool)
    .await
    .map_err(|e| 
    {
        error!("Failed to add participant '{}' to project {}: {}", participant_id, project_id, e);
        AppError::InternalServerError
    })?;
    Ok(())
}

pub async fn remove_participant_from_project(
    pool: &PgPool,
    project_id: i32,
    participant_id: &str,
) -> Result<(), AppError> 
{
    let result = sqlx::query(
        "DELETE FROM project_participants WHERE project_id = $1 AND participant_id = $2"
    )
    .bind(project_id)
    .bind(participant_id)
    .execute(pool)
    .await
    .map_err(|e| 
    {
        error!("Failed to remove participant '{}' from project {}: {}", participant_id, project_id, e);
        AppError::InternalServerError
    })?;

    if result.rows_affected() == 0 
    {
        warn!("Attempted to remove non-existent participant '{}' from project {}", participant_id, project_id);
    }

    Ok(())
}

fn encrypt_env_vars(
    env_vars: &HashMap<String, String>,
    key: &[u8],
) -> Result<HashMap<String, String>, AppError>
{
    env_vars.iter()
        .map(|(k, v)|
        {
            let encrypted_val = crypto_service::encrypt(v, key)?;
            Ok((k.clone(), base64::prelude::BASE64_STANDARD.encode(encrypted_val)))
        })
        .collect()
}

pub async fn update_project_env_vars(
    pool: &PgPool,
    project_id: i32,
    env_vars: &HashMap<String, String>,
    encryption_key: &[u8],
) -> Result<(), AppError>
{
    let encrypted_vars = encrypt_env_vars(env_vars, encryption_key)?;
    let env_vars_json = serde_json::to_value(encrypted_vars).map_err(|_| AppError::InternalServerError)?;

    sqlx::query("UPDATE projects SET env_vars = $1 WHERE id = $2")
        .bind(env_vars_json)
        .bind(project_id)
        .execute(pool)
        .await
        .map_err(|e|
        {
            error!("Failed to update env vars for project {}: {}", project_id, e);
            AppError::InternalServerError
        })?;
    Ok(())
}

pub async fn update_project_container_name(
    pool: &PgPool,
    project_id: i32,
    new_container_name: &str,
) -> Result<(), AppError>
{
    sqlx::query("UPDATE projects SET container_name = $1 WHERE id = $2")
        .bind(new_container_name)
        .bind(project_id)
        .execute(pool)
        .await
        .map_err(|e|
        {
            error!("Failed to update container name for project {}: {}", project_id, e);
            AppError::InternalServerError
        })?;
    Ok(())
}

pub async fn update_project_image_and_digest(
    pool: &PgPool,
    project_id: i32,
    new_image_tag: &str,
    new_image_digest: &str,
) -> Result<(), AppError> 
{
    sqlx::query("UPDATE projects SET deployed_image_tag = $1, deployed_image_digest = $2 WHERE id = $3")
        .bind(new_image_tag)
        .bind(new_image_digest)
        .bind(project_id)
        .execute(pool)
        .await
        .map_err(|e| 
        {
            error!("Failed to update project {} with new image and digest: {}", project_id, e);
            AppError::InternalServerError
        })?;
    Ok(())
}

pub async fn update_project_source_url(
    pool: &PgPool,
    project_id: i32,
    new_source_url: &str,
) -> Result<(), AppError>
{
    sqlx::query("UPDATE projects SET source_url = $1 WHERE id = $2")
        .bind(new_source_url)
        .bind(project_id)
        .execute(pool)
        .await
        .map_err(|e|
        {
            error!("Failed to update source_url for project {}: {}", project_id, e);
            AppError::InternalServerError
        })?;
    Ok(())
}

pub async fn get_project_by_container_name(
    pool: &PgPool,
    container_name: &str,
) -> Result<Option<Project>, AppError> 
{
    sqlx::query_as::<_, Project>(&format!("{} WHERE container_name = $1", SELECT_PROJECT_FIELDS))
        .bind(container_name)
        .fetch_optional(pool)
        .await
        .map_err(|e| 
        {
            error!("Failed to fetch project by container name '{}': {}", container_name, e);
            AppError::InternalServerError
        })
}

pub async fn get_projects_by_ids(pool: &PgPool, ids: &[i32]) -> Result<Vec<Project>, AppError> 
{
    if ids.is_empty() 
    {
        return Ok(Vec::new());
    }

    let query = format!("{} WHERE id = ANY($1)", SELECT_PROJECT_FIELDS);
    sqlx::query_as::<_, Project>(&query)
        .bind(ids)
        .fetch_all(pool)
        .await
        .map_err(|e| 
        {
            error!("Failed to fetch projects by ids {:?}: {}", ids, e);
            AppError::InternalServerError
        })
}