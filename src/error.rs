use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde::Serialize;
use serde_json::json;
use thiserror::Error;
use tracing::{error, trace};

#[derive(Debug, Error)]
pub enum AppError
{
    #[error("Internal Server Error")]
    InternalServerError,

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Error occurred while calling external service")]
    ExternalServiceError(#[from] reqwest::Error),

    #[error("Error parsing response")]
    ParsingError(#[from] quick_xml::DeError),

    #[error("Bad Request: {0}")]
    BadRequest(String),

    #[error("Project operation failed: {0}")]
    ProjectError(#[from] ProjectErrorCode),

    #[error("Database operation failed: {0}")]
    DatabaseError(#[from] DatabaseErrorCode),
}

#[derive(Debug, Error)]
pub enum ConfigError
{
    #[error("Missing environment variable: {0}")]
    Missing(String),

    #[error("Invalid environment variable: {0} (value: '{1}')")]
    Invalid(String, String),
}

#[derive(Debug, Error, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProjectErrorCode
{
    #[error("This project name is already taken.")]
    ProjectNameTaken,
    #[error("You already own a project. Only one is allowed per user.")]
    OwnerAlreadyExists,
    #[error("The project owner cannot be added as a participant.")]
    OwnerCannotBeParticipant,
    #[error("The project name is invalid. It must be 1-63 characters, contain only a-z, 0-9, or '-', and not start/end with a hyphen.")]
    InvalidProjectName,
    #[error("The provided Docker image URL is invalid or contains forbidden characters.")]
    InvalidImageUrl,
    #[error("Failed to pull the Docker image. Please check the URL and registry access.")]
    ImagePullFailed,
    #[error("Security scan failed: vulnerabilities were found in the image.")]
    ImageScanFailed(String),
    #[error("Failed to create the project container.")]
    ContainerCreationFailed,
    #[error("Failed to delete the project.")]
    DeleteFailed,
    #[error("The provided GitHub URL is invalid or unsupported.")]
    InvalidGithubUrl,
    #[error("The GitHub App is not installed on the repository owner's account.")]
    GithubAccountNotLinked,
    #[error("The GitHub App installation does not have access to this repository. Please update your installation settings.")]
    GithubRepoNotAccessible,
    #[error("Images from ghcr.io must be public for direct deployment.")]
    GithubPackageNotPublic, 
    #[error("Usage of the environment variable '{0}' is forbidden.")]
    ForbiddenEnvVar(String), 
    #[error("The specified persistent volume path is invalid.")]
    InvalidVolumePath,
    #[error("A database operation failed during project creation.")]
    ProjectCreationFailedWithDatabaseError,
    #[error("The specified source root directory is invalid.")]
    InvalidSourceRootDir,
}

#[derive(Debug, Error, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DatabaseErrorCode
{
    #[error("You already own a database. Only one is allowed per user.")]
    DatabaseAlreadyExists,
    #[error("Failed to provision the database.")]
    ProvisioningFailed,
    #[error("Failed to deprovision the database.")]
    DeprovisioningFailed,
    #[error("Database not found.")]
    NotFound,
}


impl ProjectErrorCode 
{
    const fn as_str(&self) -> &'static str 
    {
        match self 
        {
            Self::ProjectNameTaken => "PROJECT_NAME_TAKEN",
            Self::OwnerAlreadyExists => "OWNER_ALREADY_EXISTS",
            Self::OwnerCannotBeParticipant => "OWNER_CANNOT_BE_PARTICIPANT",
            Self::InvalidProjectName => "INVALID_PROJECT_NAME",
            Self::InvalidImageUrl => "INVALID_IMAGE_URL",
            Self::ImagePullFailed => "IMAGE_PULL_FAILED",
            Self::ImageScanFailed(_) => "IMAGE_SCAN_FAILED",
            Self::ContainerCreationFailed => "CONTAINER_CREATION_FAILED",
            Self::DeleteFailed => "DELETE_FAILED",
            Self::GithubAccountNotLinked => "GITHUB_ACCOUNT_NOT_LINKED",
            Self::GithubRepoNotAccessible => "GITHUB_REPO_NOT_ACCESSIBLE",
            Self::GithubPackageNotPublic => "GITHUB_PACKAGE_NOT_PUBLIC",
            Self::ForbiddenEnvVar(_) => "FORBIDDEN_ENV_VAR",
            Self::InvalidVolumePath => "INVALID_VOLUME_PATH",
            Self::InvalidGithubUrl => "INVALID_GITHUB_URL",
            Self::ProjectCreationFailedWithDatabaseError => "PROJECT_CREATION_FAILED_WITH_DATABASE_ERROR",
            Self::InvalidSourceRootDir => "INVALID_SOURCE_ROOT_DIR",
        }
    }
}

impl DatabaseErrorCode 
{
    const fn as_str(&self) -> &'static str 
    {
        match self 
        {
            Self::DatabaseAlreadyExists => "DATABASE_ALREADY_EXISTS",
            Self::ProvisioningFailed => "PROVISIONING_FAILED",
            Self::DeprovisioningFailed => "DEPROVISIONING_FAILED",
            Self::NotFound => "NOT_FOUND",
        }
    }
}

impl IntoResponse for AppError
{
    fn into_response(self) -> Response
    {
        let (status, body) = match self
        {
            Self::InternalServerError
            | Self::ExternalServiceError(_)
            | Self::ParsingError(_) =>
            {
                error!("--> SERVER ERROR (500): {:?}", self);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error_code": "INTERNAL_SERVER_ERROR", "message": "An internal error has occurred" })),
                )
            }

            Self::Unauthorized(message) =>
            {
                trace!("--> NOT AUTHORIZED (401): {}", message);
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error_code": "UNAUTHORIZED", "message": message })),
                )
            }

            Self::NotFound(ressource) =>
            {
                trace!("--> RESOURCE NOT FOUND (404): {}", ressource);
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error_code": "NOT_FOUND", "message": ressource })),
                )
            }

            Self::BadRequest(message) =>
            {
                trace!("--> BAD REQUEST (400): {}", message);
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error_code": "BAD_REQUEST", "message": message })),
                )
            }

            Self::DatabaseError(code) =>
            {
                trace!("--> DATABASE ERROR (400): {}", code);
                let status = match code 
                {
                    DatabaseErrorCode::ProvisioningFailed | DatabaseErrorCode::DeprovisioningFailed => StatusCode::INTERNAL_SERVER_ERROR,
                    _ => StatusCode::BAD_REQUEST
                };

                let error_json = json!(
                {
                    "error_code": code.as_str(),
                    "message": code.to_string()
                });

                (
                    status,
                    Json(error_json),
                )
            }
            
            Self::ProjectError(code) =>
            {
                trace!("--> PROJECT ERROR (400): {}", code);
                let status = match code 
                {
                    ProjectErrorCode::ImagePullFailed | ProjectErrorCode::ContainerCreationFailed => StatusCode::INTERNAL_SERVER_ERROR,
                    _ => StatusCode::BAD_REQUEST
                };

                let mut error_json = json!(
                {
                    "error_code": code.as_str(),
                    "message": code.to_string()
                });

                if let Some(obj) = error_json.as_object_mut()
                {
                    match code
                    {
                        ProjectErrorCode::ImageScanFailed(details) =>
                        {
                            obj.insert("details".to_string(), json!(details));
                        }
                        ProjectErrorCode::ForbiddenEnvVar(var) =>
                        {
                             obj.insert("details".to_string(), json!({ "variable": var }));
                        }
                        _ => {}
                    }
                }

                (
                    status,
                    Json(error_json),
                )
            }
        };

        (status, body).into_response()
    }
}