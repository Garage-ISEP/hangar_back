use crate::error::ConfigError;
use serde::Deserialize;
use base64::prelude::*;
use std::collections::HashSet;

#[derive(Deserialize, Clone)]
pub struct Config
{
    pub host: String,
    pub port: u16,
    pub db_url: String,
    pub mariadb_url: String,
    pub mariadb_public_host: String,
    pub mariadb_public_port: u16,
    pub public_address: String,
    pub jwt_secret: String,
    pub jwt_expiration_seconds: u64,
    pub cas_validation_url: String,
    pub app_prefix: String,
    pub app_domain_suffix: String,
    pub build_base_image: String,
    pub github_app_id: String,
    pub github_private_key: Vec<u8>,
    pub docker_network: String,
    pub traefik_entrypoint: String,
    pub traefik_cert_resolver: String,
    pub container_memory_mb: i64,
    pub container_cpu_quota: i64,
    pub grype_enabled: bool,
    pub grype_fail_on_severity: String,
    pub db_max_connections: u32,
    pub timeout_normal: u64,
    pub timeout_long: u64,
    pub admin_logins: HashSet<String>,
    pub encryption_key: Vec<u8>,
}

impl Config
{
    pub fn from_env() -> Result<Self, ConfigError>
    {
        let host = std::env::var("APP_HOST").map_err(|_| ConfigError::Missing("APP_HOST".to_string()))?;

        let port_str = std::env::var("APP_PORT").map_err(|_| ConfigError::Missing("APP_PORT".to_string()))?;
        let port = port_str.parse::<u16>().map_err(|_|
        {
            ConfigError::Invalid("APP_PORT".to_string(), port_str)
        })?;

        let public_address = std::env::var("APP_PUBLIC_ADDRESS")
            .map_err(|_| ConfigError::Missing("APP_PUBLIC_ADDRESS".to_string()))?;

        let db_url = std::env::var("DATABASE_URL")
            .map_err(|_| ConfigError::Missing("DATABASE_URL".to_string()))?;

        let mariadb_url = std::env::var("MARIADB_URL")
            .map_err(|_| ConfigError::Missing("MARIADB_URL".to_string()))?;
            
        let mariadb_public_host = std::env::var("MARIADB_PUBLIC_HOST")
            .map_err(|_| ConfigError::Missing("MARIADB_PUBLIC_HOST".to_string()))?;
            
        let mariadb_public_port_str = std::env::var("MARIADB_PUBLIC_PORT")
            .map_err(|_| ConfigError::Missing("MARIADB_PUBLIC_PORT".to_string()))?;
        
        let mariadb_public_port = mariadb_public_port_str.parse::<u16>().map_err(|_|
        {
            ConfigError::Invalid("MARIADB_PUBLIC_PORT".to_string(), mariadb_public_port_str)
        })?;

        let jwt_secret = std::env::var("APP_JWT_SECRET")
            .map_err(|_| ConfigError::Missing("APP_JWT_SECRET".to_string()))?;

        let jwt_expiration_seconds = std::env::var("JWT_EXPIRATION_SECONDS")
            .map_err(|_| ConfigError::Missing("JWT_EXPIRATION_SECONDS".to_string()))?
            .parse().map_err(|_| ConfigError::Invalid("JWT_EXPIRATION_SECONDS".to_string(), "Invalid number".to_string()))?;

        let cas_validation_url = std::env::var("CAS_VALIDATION_URL")
            .map_err(|_| ConfigError::Missing("CAS_VALIDATION_URL".to_string()))?;

        let app_prefix = std::env::var("APP_PREFIX").map_err(|_| ConfigError::Missing("APP_PREFIX".to_string()))?;
        let app_domain_suffix = std::env::var("APP_DOMAIN_SUFFIX").map_err(|_| ConfigError::Missing("APP_DOMAIN_SUFFIX".to_string()))?;

        let build_base_image = std::env::var("BUILD_BASE_IMAGE")
            .map_err(|_| ConfigError::Missing("BUILD_BASE_IMAGE".to_string()))?;

        let github_app_id = std::env::var("GITHUB_APP_ID")
            .map_err(|_| ConfigError::Missing("GITHUB_APP_ID".to_string()))?;

        let private_key_b64 = std::env::var("GITHUB_PRIVATE_KEY_B64")
            .map_err(|_| ConfigError::Missing("GITHUB_PRIVATE_KEY_B64".to_string()))?;

        let github_private_key = BASE64_STANDARD.decode(private_key_b64)
            .map_err(|_| ConfigError::Invalid("GITHUB_PRIVATE_KEY_B64".to_string(), "Invalid Base64".to_string()))?;

        let docker_network = std::env::var("DOCKER_NETWORK").map_err(|_| ConfigError::Missing("DOCKER_NETWORK".to_string()))?;
        let traefik_entrypoint = std::env::var("DOCKER_TRAEFIK_ENTRYPOINT").map_err(|_| ConfigError::Missing("DOCKER_TRAEFIK_ENTRYPOINT".to_string()))?;
        let traefik_cert_resolver = std::env::var("DOCKER_TRAEFIK_CERTRESOLVER")
            .map_err(|_| ConfigError::Missing("DOCKER_TRAEFIK_CERTRESOLVER".to_string()))?;

        let grype_enabled_str = std::env::var("GRYPE_ENABLED")
            .map_err(|_| ConfigError::Missing("GRYPE_ENABLED".to_string()))?;
        let grype_enabled = grype_enabled_str.parse::<bool>().map_err(|_|
        {
            ConfigError::Invalid("GRYPE_ENABLED".to_string(), grype_enabled_str)
        })?;


        let grype_fail_on_severity = std::env::var("GRYPE_FAIL_ON_SEVERITY")
            .map_err(|_| ConfigError::Missing("GRYPE_FAIL_ON_SEVERITY".to_string()))?;

        let container_memory_mb = std::env::var("DOCKER_CONTAINER_MEMORY_MB")
            .map_err(|_| ConfigError::Missing("DOCKER_CONTAINER_MEMORY_MB".to_string()))?
            .parse().map_err(|_| ConfigError::Invalid("DOCKER_CONTAINER_MEMORY_MB".to_string(), "Invalid number".to_string()))?;

        let container_cpu_quota = std::env::var("DOCKER_CONTAINER_CPU_QUOTA")
            .map_err(|_| ConfigError::Missing("DOCKER_CONTAINER_CPU_QUOTA".to_string()))?
            .parse().map_err(|_| ConfigError::Invalid("DOCKER_CONTAINER_CPU_QUOTA".to_string(), "Invalid number".to_string()))?;

        let db_max_connections = std::env::var("DB_MAX_CONNECTIONS")
            .map_err(|_| ConfigError::Missing("DB_MAX_CONNECTIONS".to_string()))?
            .parse().map_err(|_| ConfigError::Invalid("DB_MAX_CONNECTIONS".to_string(), "Invalid number".to_string()))?;

        let timeout_normal = std::env::var("TIMEOUT_SECONDS_NORMAL")
            .map_err(|_| ConfigError::Missing("TIMEOUT_SECONDS_NORMAL".to_string()))?
            .parse().map_err(|_| ConfigError::Invalid("TIMEOUT_SECONDS_NORMAL".to_string(), "Invalid number".to_string()))?;

        let timeout_long = std::env::var("TIMEOUT_SECONDS_LONG")
            .map_err(|_| ConfigError::Missing("TIMEOUT_SECONDS_LONG".to_string()))?
            .parse().map_err(|_| ConfigError::Invalid("TIMEOUT_SECONDS_LONG".to_string(), "Invalid number".to_string()))?;

        let admin_logins = std::env::var("APP_ADMINS")
            .map_err(|_| ConfigError::Missing("APP_ADMINS".to_string()))?
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<HashSet<String>>();

        let encryption_key_hex = std::env::var("APP_ENCRYPTION_KEY")
            .map_err(|_| ConfigError::Missing("APP_ENCRYPTION_KEY".to_string()))?;

        let encryption_key: Vec<u8> = (0..encryption_key_hex.len())
                                        .step_by(2)
                                        .map(|i| u8::from_str_radix(&encryption_key_hex[i..i + 2], 16))
                                        .collect::<Result<_, _>>()
                                        .map_err(|_| ConfigError::Invalid(
                                            "APP_ENCRYPTION_KEY".to_string(), 
                                            "Invalid hex format".to_string()
                                        ))?;

        if encryption_key.len() != 32
        {
            return Err(ConfigError::Invalid("APP_ENCRYPTION_KEY".to_string(), "Key must be 32 bytes (64 hex characters)".to_string()));
        }


        Ok(Self 
        {
            host,
            port,
            db_url,
            mariadb_url,
            mariadb_public_host,
            mariadb_public_port,
            public_address,
            jwt_secret,
            jwt_expiration_seconds,
            cas_validation_url,
            app_prefix,
            app_domain_suffix,
            build_base_image,
            github_app_id,
            github_private_key,
            docker_network,
            traefik_entrypoint,
            traefik_cert_resolver,
            container_memory_mb,
            container_cpu_quota,
            grype_enabled,
            grype_fail_on_severity,
            db_max_connections,
            timeout_normal,
            timeout_long,
            admin_logins,
            encryption_key
        })
    }
}