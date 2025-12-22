//! Service de validation pour les ressources du Hangar.
//! 
//! Ce module centralise les règles de sécurité et les contraintes de format
//! pour les noms de projets, les images Docker, les variables d'environnement et les volumes.

use crate::error::{AppError, ProjectErrorCode};
use std::collections::{HashMap, HashSet};

/// Valide le nom d'un projet selon les standards DNS/RFC 1123.
///
/// # Arguments
/// * `name` - Le nom du projet à valider.
///
/// # Returns
/// * `Ok(String)` - Le nom converti en minuscules si valide.
/// * `Err(AppError)` - Si le nom est vide, trop long (>63), contient des caractères invalides
///   ou commence/finit par un tiret.
///
/// # Errors
/// Retourne [`ProjectErrorCode::InvalidProjectName`] en cas d'échec.
///
/// # Examples
/// ```
/// # use hangar::services::validation_service::validate_project_name;
/// assert!(validate_project_name("Mon-Projet").is_ok());
/// assert_eq!(validate_project_name("Mon-Projet").unwrap(), "mon-projet");
/// assert!(validate_project_name("-invalid").is_err());
/// ```
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

/// Vérifie qu'une URL d'image Docker ne contient pas de caractères malveillants.
///
/// Empêche l'injection de commandes shell lors de l'appel à `docker pull`.
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

/// Valide les variables d'environnement utilisateur.
/// 
/// Interdit l'écrasement de variables sensibles (PATH, etc.) ou de configuration Traefik
/// qui pourraient compromettre l'isolation du réseau ou du système.
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

/// Valide le chemin de destination d'un volume persistant dans le conteneur.
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

/// Valide le répertoire racine des sources (pour les builds GitHub).
/// 
/// Empêche la sortie du répertoire de travail (Path Traversal) et l'accès
/// à des dossiers sensibles comme `.git`.
pub fn validate_source_root_dir(path: &str) -> Result<(), AppError> 
{
    if path.is_empty() 
    { 
        return Ok(()); 
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_project_name() 
    {
        // Cas valides
        assert_eq!(validate_project_name("my-app").unwrap(), "my-app");
        assert_eq!(validate_project_name("My-App").unwrap(), "my-app"); // Lowercase normalization

        // Cas invalides
        assert!(validate_project_name("").is_err());
        assert!(validate_project_name("a".repeat(64).as_str()).is_err());
        assert!(validate_project_name("invalid_name").is_err()); // underscore non autorisé
        assert!(validate_project_name("-start-with-hyphen").is_err());
        assert!(validate_project_name("end-with-hyphen-").is_err());
        assert!(validate_project_name("space in name").is_err());
    }

    #[test]
    fn test_validate_image_url() 
    {
        assert!(validate_image_url("nginx:latest").is_ok());
        assert!(validate_image_url("ghcr.io/owner/repo:v1.0.0").is_ok());

        assert!(validate_image_url("").is_err());
        assert!(validate_image_url("image; rm -rf /").is_err());
        assert!(validate_image_url("image name").is_err());
        assert!(validate_image_url("image$tag").is_err());
    }

    #[test]
    fn test_validate_env_vars() 
    {
        let mut vars = HashMap::new();
        vars.insert("APP_COLOR".into(), "blue".into());
        assert!(validate_env_vars(&vars).is_ok());

        // Test variables interdites (case insensitive)
        let mut bad_vars = HashMap::new();
        bad_vars.insert("path".into(), "/usr/bin".into());
        assert!(validate_env_vars(&bad_vars).is_err());

        // Test préfixe Traefik
        let mut traefik_vars = HashMap::new();
        traefik_vars.insert("TRAEFIK_HTTP_ROUTERS".into(), "rule".into());
        assert!(validate_env_vars(&traefik_vars).is_err());
    }

    #[test]
    fn test_validate_volume_path() 
    {
        assert!(validate_volume_path("/data").is_ok());
        assert!(validate_volume_path("/var/www/html").is_ok());

        assert!(validate_volume_path("relative/path").is_err());
        assert!(validate_volume_path("/data/../etc").is_err());
        assert!(validate_volume_path("").is_err());
        assert!(validate_volume_path("/etc").is_err()); // Forbidden
        assert!(validate_volume_path("/").is_err());    // Forbidden
    }

    #[test]
    fn test_validate_source_root_dir() 
    {
        assert!(validate_source_root_dir("src").is_ok());
        assert!(validate_source_root_dir("app/web").is_ok());
        assert!(validate_source_root_dir("").is_ok()); // Root is fine

        assert!(validate_source_root_dir("/absolute").is_err());
        assert!(validate_source_root_dir("../outside").is_err());
        assert!(validate_source_root_dir("dossier/../secret").is_err());
        assert!(validate_source_root_dir("my.git").is_err());
        assert!(validate_source_root_dir(".ssh/config").is_err());
    }
}