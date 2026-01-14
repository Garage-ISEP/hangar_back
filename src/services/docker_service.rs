use bollard::auth::DockerCredentials;
use bollard::errors::Error as BollardError;
use bollard::secret::{ContainerStatsResponse, Mount, MountTypeEnum, ResourcesUlimits, RestartPolicy};
use bollard::models::VolumeCreateOptions;
use bollard::Docker;
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::
{
    BuildImageOptions, CreateContainerOptionsBuilder, CreateImageOptions, InspectContainerOptions, ListContainersOptions, LogsOptions, RemoveContainerOptions, RemoveImageOptions, RemoveVolumeOptions, RestartContainerOptions, StartContainerOptions, StatsOptions, StopContainerOptions
};
use flate2::write::GzEncoder;
use flate2::Compression;
use futures::stream::StreamExt;
use tar::Builder;
use tokio::process::Command;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use tracing::{debug, error, info, warn};

use crate::error::{AppError, ProjectErrorCode};
use crate::model::project::{GlobalMetrics, ProjectMetrics};
use bollard::models::ContainerInspectResponse;

pub async fn pull_image(docker: &Docker, image_url: &str, credentials: Option<DockerCredentials>) -> Result<(), BollardError> 
{
    let options = Some(CreateImageOptions 
    {
        from_image: Some(image_url.to_string()),
        ..Default::default()
    });

    let mut stream = docker.create_image(options, None, credentials);

    info!("Pulling image {}", image_url);
    while let Some(result) = stream.next().await 
    {
        match result 
        {
            Ok(info) => 
            {
                if let Some(error_detail) = info.error_detail
                    && let Some(message) = error_detail.message
                        && (message.to_lowercase().contains("unauthorized") || message.to_lowercase().contains("authentication required")) 
                        {
                            warn!("Authentication error during image pull for '{}': {}", image_url, message);
                        }
            }
            Err(e) => 
            {
                return Err(e);
            }
        }
    }
    info!("Image '{}' pulled successfully.", image_url);
    Ok(())
}


pub async fn scan_image_with_grype(image_url: &str, config: &crate::config::Config) -> Result<(), AppError> 
{
    if !config.grype_enabled 
    {
        warn!("Grype scan is disabled via GRYPE_ENABLED=false. Skipping security scan for image '{}'.", image_url);
        return Ok(());
    }

    info!("Scanning image '{}' with Grype...", image_url);

    let mut command = Command::new("grype");
    command
        .arg(image_url)
        .arg("--only-fixed")
        .arg("--fail-on")
        .arg(&config.grype_fail_on_severity)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command.output().await.map_err(|e| 
    {
        error!("Failed to execute grype command: {}", e);
        AppError::InternalServerError
    })?;

    if !output.status.success() 
    {
        warn!("Grype found vulnerabilities in image '{}'", image_url);
        let report = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(ProjectErrorCode::ImageScanFailed(report).into());
    }

    info!("Grype scan passed for image '{}'.", image_url);
    Ok(())
}

pub async fn create_project_container(
    docker: &Docker,
    container_name: &str,
    project_name: &str,
    image_identifier: &str,
    config: &crate::config::Config,
    env_vars: &Option<HashMap<String, String>>,
    persistent_volume_path: &Option<String>,
) -> Result<Option<String>, AppError>
{
    let hostname = format!("{}.{}", project_name, &config.app_domain_suffix);

    let mut mounts = vec![];
    let mut volume_name_created: Option<String> = None;
    if let Some(path) = persistent_volume_path
    {
        let volume_name = format!("hangar-data-{project_name}");

        let options = VolumeCreateOptions
        {
            name: Some(volume_name.clone()),
            driver: Some("local".to_string()),
            ..Default::default()
        };
        docker.create_volume(options).await.map_err(|e|
        {
            error!("Failed to create Docker volume '{}': {}", volume_name, e);
            ProjectErrorCode::ContainerCreationFailed
        })?;

        volume_name_created = Some(volume_name.clone());

        mounts.push(Mount
        {
            target: Some(path.clone()),
            source: Some(volume_name),
            typ: Some(MountTypeEnum::VOLUME),
            ..Default::default()
        });
    }

    let host_config = HostConfig 
    {
        restart_policy: Some(RestartPolicy 
        {
            name: Some(bollard::secret::RestartPolicyNameEnum::UNLESS_STOPPED),
            maximum_retry_count: None,
        }),

        memory: Some(config.container_memory_mb * 1024 * 1024),
        cpu_quota: Some(config.container_cpu_quota),
        network_mode: Some(config.docker_network.clone()),
        security_opt: Some(vec![
            "no-new-privileges:true".to_string(),
            "apparmor:docker-default".to_string()
        ]),
        readonly_rootfs: Some(false),
        privileged: Some(false),
        pids_limit: Some(1024),
        ulimits: Some(vec![
            ResourcesUlimits { name: Some("nofile".to_string()), soft: Some(1024), hard: Some(2048) },
            ResourcesUlimits { name: Some("nproc".to_string()), soft: Some(512), hard: Some(1024) }
        ]),
        
        tmpfs: Some(HashMap::from([
            ("/tmp".to_string(), "rw,noexec,nosuid,size=100m".to_string())
        ])),
        oom_kill_disable: Some(false),
        memory_swappiness: Some(0),
        mounts: Some(mounts),
        ..Default::default()
    };

    let env = env_vars.as_ref().map(|vars|
    {
        vars.iter().map(|(k, v)| format!("{k}={v}")).collect()
    });

    let mut labels = HashMap::new();
    labels.insert("app".to_string(), config.app_prefix.clone());
    labels.insert("traefik.enable".to_string(), "true".to_string());
    labels.insert(format!("traefik.http.routers.{project_name}.rule"), format!("Host(`{hostname}`)"));
    labels.insert(format!("traefik.http.routers.{project_name}.entrypoints"), config.traefik_entrypoint.clone());
    labels.insert(format!("traefik.http.routers.{project_name}.tls.certresolver"), config.traefik_cert_resolver.clone());
    labels.insert(format!("traefik.http.services.{project_name}.loadbalancer.server.port"), "80".to_string());

    let config = ContainerCreateBody 
    {
        image: Some(image_identifier.to_string()),
        host_config: Some(host_config),
        labels: Some(labels),
        env,
        ..Default::default()
    };

    let options = Some(CreateContainerOptionsBuilder::new().name(container_name).build());

    let response = docker.create_container(options, config).await.map_err(|e| 
    {
        error!("Failed to create container '{}': {}", container_name, e);
        let docker_clone = docker.clone();
        let volume_to_cleanup = volume_name_created.clone();
        tokio::spawn(async move 
        {
            if let Some(vol) = volume_to_cleanup 
            {
                if let Err(e) = remove_volume_by_name(&docker_clone, &vol).await 
                {
                    error!("ROLLBACK FAILED: Could not remove volume '{}' after container start failure: {}", vol, e);
                } 
                else 
                {
                    info!("Rollback successful for volume '{}'", vol);
                }
            }
        });
        ProjectErrorCode::ContainerCreationFailed
    })?;

    docker.start_container(container_name, None::<StartContainerOptions>).await.map_err(|e| 
    {
        error!("Failed to start container '{}': {}", container_name, e);
        
        let docker_clone = docker.clone();
        let container_name_clone = container_name.to_string();
        let volume_to_cleanup = volume_name_created.clone();
        tokio::spawn(async move 
        {
            warn!("Attempting rollback for failed container start: {}", container_name_clone);
            if let Err(remove_err) = docker_clone.remove_container(&container_name_clone, None::<RemoveContainerOptions>).await 
            {
                error!("ROLLBACK FAILED: Could not remove container '{}' after start failure: {}", container_name_clone, remove_err);
            } 
            else 
            {
                info!("Rollback successful for container '{}'", container_name_clone);
            }

            if let Some(vol) = volume_to_cleanup 
            {
                if let Err(e) = remove_volume_by_name(&docker_clone, &vol).await 
                {
                    error!("ROLLBACK FAILED: Could not remove volume '{}' after container start failure: {}", vol, e);
                } 
                else 
                {
                    info!("Rollback successful for volume '{}'", vol);
                }
            }
        });
        
        ProjectErrorCode::ContainerCreationFailed
    })?;

    info!("Container '{}' created and started with ID: {}", container_name, response.id);
    Ok(volume_name_created)
}

pub async fn remove_container(docker: &Docker, container_name: &str) -> Result<(), AppError> 
{
    info!("Attempting to stop and remove container: {}", container_name);

    match docker.stop_container(container_name, None::<StopContainerOptions>).await 
    {
        Ok(()) => (),
        Err(bollard::errors::Error::DockerResponseServerError { status_code, .. }) if status_code == 404 || status_code == 304 =>
        {
            warn!("Container {} not found or already stopped. No action taken.", container_name);
        },
        Err(e) => 
        {
            error!("Error stopping container {}: {}", container_name, e);
        }
    }

    match docker.remove_container(container_name, None::<RemoveContainerOptions>).await 
    {
        Ok(()) => (),
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => 
        {
            warn!("Container {} not found during removal. It might have been deleted already.", container_name);
        },
        Err(e) =>
        {
            error!("Error removing container {}: {}", container_name, e);
            return Err(AppError::InternalServerError);
        }
    }

    info!("Container {} has been successfully removed.", container_name);
    Ok(())
}

pub async fn remove_image(docker: &Docker, image_url: &str) -> Result<(), AppError>
{
    info!("Attempting to remove image: {}", image_url);

    let options = Some(RemoveImageOptions 
    {
        force: true,
        ..Default::default()
    });
    if let Err(e) = docker.remove_image(image_url, options, None).await 
    {
        error!("Could not remove image '{}': {}", image_url, e);
        Err(AppError::InternalServerError)
    } 
    else 
    {
        info!("Image {} successfully removed", image_url);
        Ok(())
    }
}

pub async fn remove_volume_by_name(docker: &Docker, volume_name: &str) -> Result<(), AppError>
{
    info!("Attempting to remove volume: {}", volume_name);
    let options = Some(RemoveVolumeOptions { force: true });
    match docker.remove_volume(volume_name, options).await
    {
        Ok(()) =>
        {
            info!("Volume {} successfully removed.", volume_name);
            Ok(())
        }
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) =>
        {
            warn!("Volume {} not found during removal. It might have been deleted already.", volume_name);
            Ok(())
        }
        Err(e) =>
        {
            error!("Error removing volume {}: {}", volume_name, e);
            Err(AppError::InternalServerError)
        }
    }
}

pub async fn start_container_by_name(docker: &Docker, container_name: &str) -> Result<(), AppError> 
{
    docker.start_container(container_name, None::<StartContainerOptions>).await.map_err(|e| 
    {
        error!("Failed to start container '{}': {}", container_name, e);
        AppError::InternalServerError
    })
}

pub async fn stop_container_by_name(docker: &Docker, container_name: &str) -> Result<(), AppError> 
{
    docker.stop_container(container_name, None::<StopContainerOptions>).await.map_err(|e| 
    {
        error!("Failed to stop container '{}': {}", container_name, e);
        AppError::InternalServerError
    })
}

pub async fn restart_container_by_name(docker: &Docker, container_name: &str) -> Result<(), AppError>
{
    docker.restart_container(container_name, None::<RestartContainerOptions>).await.map_err(|e| 
    {
        error!("Failed to restart container '{}': {}", container_name, e);
        AppError::InternalServerError
    })
}

pub async fn get_container_logs(docker: &Docker, container_name: &str, tail: &str) -> Result<String, AppError> 
{
    info!("Fetching logs for container '{}' with tail '{}'", container_name, tail);
    const MAX_LOG_SIZE: usize = 10 * 1024 * 1024; // 10 MB

    let options = Some(LogsOptions 
    {
        stdout: true,
        stderr: true,
        tail: tail.to_string(),
        timestamps: true,
        ..Default::default()
    });

    let mut stream = docker.logs(container_name, options);

    let mut log_entries = Vec::new();
    let mut total_size = 0;

    while let Some(log_result) = stream.next().await 
    {
        match log_result 
        {
            Ok(log_output) => 
            {
                let log_str = log_output.to_string();
                total_size += log_str.len();
                
                if total_size > MAX_LOG_SIZE 
                {
                    log_entries.push("[...] Logs truncated (exceeded 10MB)".to_string());
                    break;
                }
                
                log_entries.push(log_str);
            }
            Err(e) => 
            {
                error!("Error streaming logs for container '{}': {}", container_name, e);
            }
        }
    }

    Ok(log_entries.join(""))
}

pub async fn get_container_metrics(docker: &Docker, container_name: &str) -> Result<ProjectMetrics, AppError> 
{
    let mut stream = docker.stats(container_name, Some(StatsOptions 
    { 
        stream: false, 
        ..Default::default() 
    }));

    if let Some(stats_result) = stream.next().await 
    {
        match stats_result 
        {
            Ok(stats) => 
            {
                debug!("Received stats for container '{}': {:?}", container_name, stats);
                
                let cpu_usage = calculate_cpu_percent(&stats);
                let (memory_usage, memory_limit) = calculate_memory(&stats);

                Ok(ProjectMetrics 
                {
                    cpu_usage,
                    memory_usage: memory_usage as f64,
                    memory_limit: memory_limit as f64,
                })
            }
            Err(e) => 
            {
                error!("Failed to get stats for container '{}': {}", container_name, e);
                Err(AppError::InternalServerError)
            }
        }
    } 
    else 
    {
        Err(AppError::NotFound(format!("No stats received for container {container_name}")))
    }
}

fn calculate_cpu_percent(stats: &ContainerStatsResponse) -> f64 
{

    let calculation = || -> Option<f64> 
    {
        let cpu_stats = stats.cpu_stats.as_ref()?;
        let precpu_stats = stats.precpu_stats.as_ref()?;

        let cpu_usage = cpu_stats.cpu_usage.as_ref()?;
        let precpu_usage = precpu_stats.cpu_usage.as_ref()?;

        let total_usage = cpu_usage.total_usage?;
        let pre_total_usage = precpu_usage.total_usage?;

        let cpu_delta = total_usage as f64 - pre_total_usage as f64;

        let system_cpu_delta = (cpu_stats.system_cpu_usage? as f64) - (precpu_stats.system_cpu_usage? as f64);

        let number_of_cpus = f64::from(cpu_stats.online_cpus.unwrap_or(1));

        if system_cpu_delta > 0.0 && cpu_delta > 0.0 
        {
            Some((cpu_delta / system_cpu_delta) * number_of_cpus * 100.0)
        } 
        else 
        {
            Some(0.0)
        }
    }();

    calculation.unwrap_or(0.0)
}

fn calculate_memory(stats: &ContainerStatsResponse) -> (u64, u64) 
{
    if let Some(mem_stats) = stats.memory_stats.as_ref() 
    {
        let usage = mem_stats.usage.unwrap_or(0);
        let limit = mem_stats.limit.unwrap_or(0);

        let cache = mem_stats.stats.as_ref()
            .and_then(|s| s.get("cache"))
            .map_or(0, |v| *v);

        let actual_usage = usage.saturating_sub(cache);
        (actual_usage, limit)
    } 
    else 
    {
        (0, 0)
    }
}

pub fn create_tarball(path: &Path) -> Result<Vec<u8>, AppError>
{
    let enc = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = Builder::new(enc);
    
    tar.append_dir_all(".", path).map_err(|e| 
    {
        error!("Failed to append directory to tarball: {}", e);
        AppError::InternalServerError
    })?;

    let tar_data = tar.into_inner().and_then(flate2::write::GzEncoder::finish).map_err(|e| 
    {
        error!("Failed to finish tarball creation: {}", e);
        AppError::InternalServerError
    })?;
    
    Ok(tar_data)
}

pub async fn build_image_from_tar(
    docker: &Docker,
    tar_stream: Vec<u8>,
    image_tag: &str,
) -> Result<(), AppError>
{
    let options = BuildImageOptions 
    {
        dockerfile: "Dockerfile".to_string(),
        t: Some(image_tag.to_string()),
        rm: true,
        ..Default::default()
    };

    let mut stream = docker.build_image(options, None, Some(bollard::body_full(tar_stream.into())));

    while let Some(result) = stream.next().await
    {
        match result
        {
            Ok(info) =>
            {
                if let Some(error_detail) = info.error_detail
                {
                    error!("Failed to build image '{}': {}", image_tag, error_detail.message.unwrap_or_default());
                    return Err(AppError::BadRequest("Failed to build Docker image from source.".to_string()));
                }
                if let Some(stream_content) = info.stream
                {
                    debug!("Build > {}", stream_content.trim());
                }
            }
            Err(e) =>
            {
                error!("Docker build stream error for image '{}': {}", image_tag, e);
                return Err(AppError::InternalServerError);
            }
        }
    }

    info!("Image '{}' built successfully.", image_tag);
    Ok(())
}

pub async fn get_global_container_stats(docker: &Docker, app_prefix: &str) -> Result<GlobalMetrics, AppError> 
{
    let mut filters = HashMap::new();
    filters.insert("label".to_string(), vec![format!("app={}", app_prefix)]);

    let options = Some(ListContainersOptions 
    {
        all: true,
        filters: Some(filters),
        ..Default::default()
    });

    let containers = docker.list_containers(options).await.map_err(|e| 
    {
        error!("Failed to list hangar containers: {}", e);
        AppError::InternalServerError
    })?;

    let mut running_containers = 0;
    let mut total_cpu_usage = 0.0;
    let mut total_memory_usage = 0;

    for container_summary in containers 
    {
        if let Some(state) = container_summary.state
            && state.to_string() == "running"
                && let Some(id) = container_summary.id 
                {
                    let mut stream = docker.stats(&id, Some(StatsOptions { stream: false, ..Default::default() }));
                    if let Some(stats_result) = stream.next().await 
                    {
                        match stats_result 
                        {
                            Ok(stats) => 
                            {
                                running_containers += 1;
                                total_cpu_usage += calculate_cpu_percent(&stats);
                                let (mem_usage, _) = calculate_memory(&stats);
                                total_memory_usage += mem_usage;
                            }
                            Err(e) => {
                                warn!("Could not get stats for running container {}: {}", id, e);
                            }
                        }
                    }
                }
    }
    
    Ok(GlobalMetrics 
    {
        total_projects: 0,
        running_containers,
        total_cpu_usage,
        total_memory_usage_mb: (total_memory_usage as f64) / (1024.0 * 1024.0),
    })
}

pub async fn inspect_container_details(docker: &Docker, container_name: &str) -> Result<Option<ContainerInspectResponse>, AppError> 
{
    match docker.inspect_container(container_name, None::<InspectContainerOptions>).await 
    {
        Ok(details) => Ok(Some(details)),
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => 
        {
            Ok(None)
        },
        Err(e) => 
        {
            error!("Failed to inspect container '{}': {}", container_name, e);
            Err(AppError::InternalServerError)
        }
    }
}

pub async fn get_image_digest(docker: &Docker, image_tag: &str) -> Result<Option<String>, AppError> 
{
    match docker.inspect_image(image_tag).await 
    {
        Ok(details) => 
        {
            if let Some(id) = details.id 
            {
                Ok(Some(id))
            } 
            else 
            {
                warn!("No ID found for image '{}'", image_tag);
                Ok(None)
            }
        },
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => 
        {
            warn!("Image '{}' not found for inspection.", image_tag);
            Ok(None)
        },
        Err(e) => 
        {
            error!("Failed to inspect image '{}': {}", image_tag, e);
            Err(AppError::InternalServerError)
        }
    }
}