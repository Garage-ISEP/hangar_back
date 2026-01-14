use std::path::Path;

use crate::{config::Config, error::{AppError, ProjectErrorCode}};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::{debug, error, info, warn};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use git2::{Cred, FetchOptions, RemoteCallbacks, build::RepoBuilder};

#[derive(Debug, Deserialize)]
struct Installation
{
    id: u64,
    account: Account,
}


#[derive(Debug, Deserialize)]
struct Account
{
    login: String,
}

#[derive(Debug, Serialize)]
struct AppJwtClaims
{
    iat: u64, // Issued at
    exp: u64, // Expiration time
    iss: String, // Issuer (App ID)
}

#[derive(Debug, Deserialize)]
struct InstallationTokenResponse
{
    token: String,
}


pub async fn extract_repo_owner_and_name(repo_url: &str) -> Result<(String, String), AppError>
{
    let url = repo_url.trim();
    
    if !url.contains("github.com") 
    {
        return Err(ProjectErrorCode::InvalidGithubUrl.into());
    }
    
    let url_without_protocol = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    
    let parts: Vec<&str> = url_without_protocol
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .split('/')
        .collect();

    if parts.len() < 3 {
        return Err(ProjectErrorCode::InvalidGithubUrl.into());
    }
    
    let owner = parts[1];
    let repo_name = parts[2];
    
    if owner.is_empty() || repo_name.is_empty() 
    {
        return Err(ProjectErrorCode::InvalidGithubUrl.into());
    }
    
    info!("Extracted GitHub owner '{}' and repo '{}' from URL '{}'", owner, repo_name, repo_url);
    Ok((owner.to_string(), repo_name.to_string()))
}

pub async fn check_repo_accessibility(
    http_client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
) -> Result<(), AppError> 
{
    let url = format!("https://api.github.com/repos/{owner}/{repo}");
    info!("Checking repository accessibility at: {}", url);

    let response = http_client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hangar App")
        .send()
        .await?;

    if response.status().is_success() 
    {
        info!("Access to repository '{}/{}' confirmed.", owner, repo);
        Ok(())
    } 
    else if response.status() == reqwest::StatusCode::NOT_FOUND 
    {
        warn!(
            "Access check for repo '{}/{}' failed with 404. The App likely lacks permission.",
            owner, repo
        );
        Err(ProjectErrorCode::GithubRepoNotAccessible.into())
    } 
    else 
    {
        let error_body = response.text().await.unwrap_or_default();
        error!(
            "GitHub API request to check repo accessibility failed: {}",
            error_body
        );
        Err(AppError::InternalServerError)
    }
}


async fn generate_app_jwt(config: &Config) -> Result<String, AppError>
{
    let now = OffsetDateTime::now_utc().unix_timestamp() as u64;
    let claims = AppJwtClaims 
    {
        iat: now - 60,        // 60 secondes dans le passé pour éviter les problèmes de synchronisation d'horloge
        exp: now + (10 * 60), // Le token est valide pour 10 minutes maximum
        iss: config.github_app_id.clone(),
    };
    let header = Header::new(Algorithm::RS256);
    let key = EncodingKey::from_rsa_pem(&config.github_private_key).map_err(|e| 
    {
        error!("Failed to create encoding key from RSA PEM: {}", e);
        AppError::InternalServerError
    })?;

    encode(&header, &claims, &key).map_err(|e| 
    {
        error!("Failed to encode GitHub App JWT: {}", e);
        AppError::InternalServerError
    })
}


pub async fn get_installation_id_by_user(http_client: &reqwest::Client, config: &Config, github_username: &str) -> Result<u64, AppError>
{
    let app_jwt = generate_app_jwt(config).await?;

    let response = http_client
        .get("https://api.github.com/app/installations")
        .header("Authorization", format!("Bearer {app_jwt}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hangar App")
        .send()
        .await?;

    if !response.status().is_success()
    {
        error!("Failed to fetch installations from GitHub.");
        return Err(AppError::InternalServerError);
    }

    let installations_response: Vec<Installation> = response.json().await?;

    for inst in installations_response
    {
        if inst.account.login.eq_ignore_ascii_case(github_username)
        {
            debug!("Found matching GitHub App installation with ID: {} for user {}", inst.id, github_username);
            return Ok(inst.id);
        }
    }

    Err(ProjectErrorCode::GithubAccountNotLinked.into())
}

pub async fn get_installation_token(installation_id: u64, http_client: &reqwest::Client, config: &Config) -> Result<String, AppError>
{
    let app_jwt = generate_app_jwt(config).await?;
    let url = format!("https://api.github.com/app/installations/{installation_id}/access_tokens");

    let response = http_client
        .post(&url)
        .header("Authorization", format!("Bearer {app_jwt}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hangar App")
        .send()
        .await?;
    
    if !response.status().is_success()
    {
        let error_body = response.text().await.unwrap_or_default();
        error!("GitHub installation token request failed: {}", error_body);
        return Err(AppError::InternalServerError);
    }

    let token_response: InstallationTokenResponse = response.json().await?;
    Ok(token_response.token)
}

pub async fn clone_repo(repo_url: &str, target_dir: &Path, token: Option<&str>, branch: Option<&str>) -> Result<(), AppError>
{
    let repo_url_owned = repo_url.to_string();
    let target_dir = target_dir.to_path_buf();
    let token = token.map(std::string::ToString::to_string);
    let branch = branch.map(std::string::ToString::to_string);

    let repo_url_for_log = repo_url_owned.clone();

    let clone_result = tokio::task::spawn_blocking(move ||
    {
        let mut callbacks = RemoteCallbacks::new();

        if let Some(t) = &token
        {
            callbacks.credentials(move |_url, _username_from_url, _allowed_types|
            {
                Cred::userpass_plaintext("x-access-token", t)
            });
        }

        let mut fo = FetchOptions::new();
        fo.remote_callbacks(callbacks);
        fo.depth(1);

        let mut builder = RepoBuilder::new();
        builder.fetch_options(fo);

        if let Some(b) = &branch
        {
            builder.branch(b);
        }

        builder.clone(&repo_url_owned, &target_dir)
    })
    .await
    .map_err(|_| AppError::InternalServerError)?;

    clone_result.map_err(|e|
    {
        let msg = e.message().to_lowercase();
        if msg.contains("authentication required") || msg.contains("credentials callback returned an error")
        {
            AppError::ProjectError(ProjectErrorCode::GithubAccountNotLinked)
        }
        else
        {   error!("git2 clone failed for repo '{}': {}", repo_url_for_log, msg);
            AppError::ProjectError(ProjectErrorCode::InvalidGithubUrl)
        }
    })?;

    info!("Repository {} cloned successfully.", repo_url_for_log);
    Ok(())
}