use crate::error::{AppError, ProjectErrorCode};
use std::collections::{HashMap, HashSet};

pub fn validate_project_name(name: &str) -> Result<String, AppError>
{
    if name.is_empty() 
    {
        return Err(ProjectErrorCode::InvalidProjectName.into());
    }
    if name.len() > 63 
    {
        return Err(ProjectErrorCode::InvalidProjectName.into());
    }

    let is_valid_chars = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !is_valid_chars 
    {
        return Err(ProjectErrorCode::InvalidProjectName.into());
    }

    if name.starts_with('-') || name.ends_with('-') 
    {
        return Err(ProjectErrorCode::InvalidProjectName.into());
    }
    
    Ok(name.to_lowercase())
}

pub fn validate_image_url(url: &str) -> Result<(), AppError> 
{
    if url.is_empty() 
    {
        return Err(ProjectErrorCode::InvalidImageUrl.into());
    }

    let forbidden_chars: HashSet<char> = " $`'\"\\".chars().collect();
    if url.chars().any(|c| forbidden_chars.contains(&c)) 
    {
        return Err(ProjectErrorCode::InvalidImageUrl.into());
    }
    Ok(())
}

pub fn validate_env_vars(vars: &HashMap<String, String>) -> Result<(), AppError>
{
    const FORBIDDEN_ENV_VARS: &[&str] = &[
        "PATH", "LD_PRELOAD", "DOCKER_HOST", "HOST", "HOSTNAME",
        "TRAEFIK_ENABLE",
    ];

    for key in vars.keys()
    {
        if FORBIDDEN_ENV_VARS.iter().any(|&forbidden| key.eq_ignore_ascii_case(forbidden))
            || key.to_uppercase().starts_with("TRAEFIK_")
        {
            return Err(ProjectErrorCode::ForbiddenEnvVar(key.clone()).into());
        }
    }
    Ok(())
}

pub fn validate_volume_path(path: &str) -> Result<(), AppError>
{
    if path.is_empty()
    {
        return Err(ProjectErrorCode::InvalidVolumePath.into());
    }
    if !path.starts_with('/')
    {
        return Err(ProjectErrorCode::InvalidVolumePath.into());
    }
    if path.contains("..")
    {
        return Err(ProjectErrorCode::InvalidVolumePath.into());
    }

    const FORBIDDEN_PATHS: &[&str] = &["/", "/etc", "/bin", "/sbin", "/usr", "/boot", "/dev", "/lib", "/proc", "/sys"];
    if FORBIDDEN_PATHS.contains(&path)
    {
        return Err(ProjectErrorCode::InvalidVolumePath.into());
    }

    Ok(())
}

pub fn validate_source_root_dir(path: &str) -> Result<(), AppError> 
{
    if path.contains("..") || path.starts_with('/') || path.starts_with('\\') 
    {
        return Err(ProjectErrorCode::InvalidSourceRootDir.into());
    }

    let normalized = std::path::Path::new(path);
    for component in normalized.components() 
    {
        if let std::path::Component::ParentDir = component 
        {
            return Err(ProjectErrorCode::InvalidSourceRootDir.into());
        }
    }

    const FORBIDDEN_DIRS: &[&str] = &[".git", ".env", ".ssh"];
    if FORBIDDEN_DIRS.iter().any(|&forbidden| path.contains(forbidden))
    {
        return Err(ProjectErrorCode::InvalidSourceRootDir.into());
    }
    
    Ok(())
}