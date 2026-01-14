use crate::
{
    config::Config,
    error::{AppError, DatabaseErrorCode, ProjectErrorCode},
    model::database::{Database, DatabaseDetailsResponse},
    services::crypto_service,
};
use rand::distr::{Alphanumeric, SampleString};
use sqlx::{MySqlPool, PgPool, Postgres, Transaction};
use tracing::{error, info, warn};
use base64::prelude::*;
use std::collections::HashSet;

const DB_PREFIX: &str = "hangardb";


fn valid_identifier(s: &str) -> bool 
{
    if s.is_empty() || s.len() > 64 { return false; }
    
    // Ne doit pas commencer par un chiffre
    if s.chars().next().unwrap().is_ascii_digit() { return false; }
    
    const RESERVED: &[&str] = &["SELECT", "DROP", "INSERT", "UPDATE", "DELETE", "TABLE", "DATABASE"];
    if RESERVED.contains(&s.to_uppercase().as_str()) { return false; }
    
    let allowed: HashSet<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_".chars().collect();
    s.chars().all(|c| allowed.contains(&c))
}

pub async fn check_database_exists_for_owner(pool: &PgPool, owner: &str) -> Result<bool, AppError>
{
    let count: (i64, ) = sqlx::query_as("SELECT COUNT(*) FROM databases WHERE owner_login = $1")
        .bind(owner)
        .fetch_one(pool)
        .await
        .map_err(|e|
        {
            error!("Failed to check if database exists for owner {}: {}", owner, e);
            AppError::InternalServerError
        })?;
    Ok(count.0 > 0)
}

fn generate_password() -> String
{
    
    Alphanumeric.sample_string(&mut rand::rng(), 24)
}

pub async fn provision_database(
    pg_pool: &PgPool,
    mariadb_pool: &MySqlPool,
    owner_login: &str,
    encryption_key: &[u8],
) -> Result<(Database, String), AppError>
{
    if check_database_exists_for_owner(pg_pool, owner_login).await?
    {
        return Err(DatabaseErrorCode::DatabaseAlreadyExists.into());
    }

    let db_name = format!("{DB_PREFIX}_{owner_login}");
    let username = owner_login.to_string();
    let password = generate_password();

    if let Err(e) = execute_mariadb_provisioning(mariadb_pool, &db_name, &username, &password).await
    {
        warn!("MariaDB provisioning failed for user '{}'. Attempting rollback. Error: {}", owner_login, e);
        if let Err(e) = execute_mariadb_deprovisioning(mariadb_pool, &db_name, &username).await
        {
            error!("Failed to rollback MariaDB provisioning for user '{}': {}", owner_login, e);
        }
        return Err(e);
    }

    let encrypted_password_vec = crypto_service::encrypt(&password, encryption_key)?;
    let encrypted_password = BASE64_STANDARD.encode(encrypted_password_vec);

    let db_record = sqlx::query_as::<_, Database>(
        "INSERT INTO databases (owner_login, database_name, username, encrypted_password)
         VALUES ($1, $2, $3, $4)
         RETURNING id, owner_login, database_name, username, encrypted_password, project_id, created_at",
    )
    .bind(owner_login)
    .bind(&db_name)
    .bind(&username)
    .bind(&encrypted_password)
    .fetch_one(pg_pool)
    .await
    .map_err(|e|
    {
        error!("Failed to persist database metadata for user '{}' after successful MariaDB provisioning: {}", owner_login, e);
        let mariadb_pool = mariadb_pool.clone();
        let db_name = db_name.clone();
        let username = username.clone();
        let owner_login = owner_login.to_string();
        tokio::spawn(async move
        {
            warn!("CRITICAL: Rolling back MariaDB provisioning for {} due to PostgreSQL failure.", owner_login);
            if let Err(e) = execute_mariadb_deprovisioning(&mariadb_pool, &db_name, &username).await
            {
                error!("Failed to rollback MariaDB provisioning for user '{}': {}", owner_login, e);
            }
        });
        AppError::InternalServerError
    })?;

    info!("Database for user '{}' provisioned successfully.", owner_login);
    Ok((db_record, password))
}

pub async fn deprovision_database(
    pg_pool: &PgPool,
    mariadb_pool: &MySqlPool,
    db_id: i32,
    owner_login: &str,
    is_admin: bool
) -> Result<(), AppError>
{
    let db_record = get_database_by_id_and_owner(pg_pool, db_id, owner_login, is_admin).await?
        .ok_or(DatabaseErrorCode::NotFound)?;

    execute_mariadb_deprovisioning(mariadb_pool, &db_record.database_name, &db_record.username).await?;

    sqlx::query("DELETE FROM databases WHERE id = $1")
        .bind(db_id)
        .execute(pg_pool)
        .await
        .map_err(|e|
        {
            error!("Failed to delete database metadata for ID {}: {}", db_id, e);
            AppError::InternalServerError // La DB a été supprimée mais pas la métadonnée.
        })?;

    info!("Database ID {} for user '{}' deprovisioned successfully.", db_id, db_record.owner_login);
    Ok(())
}

async fn execute_mariadb_provisioning(
    pool: &MySqlPool,
    db_name: &str,
    username: &str,
    password: &str,
) -> Result<(), AppError> 
{
    if !valid_identifier(db_name) || !valid_identifier(username) 
    {
        error!("Invalid database or username identifier: db_name='{}', username='{}'", db_name, username);
        return Err(AppError::BadRequest("Invalid identifier".into()));
    }

    let mut conn = pool.acquire().await.map_err(|e| 
    {
        error!("Failed to acquire MariaDB connection: {}", e);
        DatabaseErrorCode::ProvisioningFailed
    })?;

    let create_db_sql = format!(
        "CREATE DATABASE `{db_name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci"
    );
    sqlx::query(&create_db_sql)
        .execute(&mut *conn)
        .await
        .map_err(|e| 
        {
            error!("Failed to create database '{}': {}", db_name, e);
            DatabaseErrorCode::ProvisioningFailed
        })?;

    let escaped_password = password.replace('\'', "\\'");
    let create_user_sql = format!("CREATE USER `{username}`@'%' IDENTIFIED BY '{escaped_password}'");
    sqlx::query(&create_user_sql)
        .execute(&mut *conn)
        .await
        .map_err(|_|
        {
            error!("Failed to create user '{}' (details hidden for security)", username);
            DatabaseErrorCode::ProvisioningFailed
        })?;

    let grant_sql = format!(
        "GRANT SELECT, INSERT, UPDATE, DELETE, CREATE, DROP, INDEX, ALTER, CREATE TEMPORARY TABLES, LOCK TABLES ON `{db_name}`.* TO `{username}`@'%'"
    );
    sqlx::query(&grant_sql)
        .execute(&mut *conn)
        .await
        .map_err(|e| 
        {
            error!("Failed to grant privileges on database '{}' to user '{}': {}", db_name, username, e);
            DatabaseErrorCode::ProvisioningFailed
        })?;

    sqlx::query("FLUSH PRIVILEGES")
        .execute(&mut *conn)
        .await
        .map_err(|e| 
        {
            error!("Failed to flush privileges: {}", e);
            DatabaseErrorCode::ProvisioningFailed
        })?;

    Ok(())
}



async fn execute_mariadb_deprovisioning(
    pool: &MySqlPool,
    db_name: &str,
    username: &str,
) -> Result<(), AppError>
{
    if !valid_identifier(db_name) || !valid_identifier(username) 
    {
        return Err(AppError::BadRequest("Invalid identifier".into()));
    }

    let mut conn = pool.acquire().await.map_err(|_| DatabaseErrorCode::DeprovisioningFailed)?;
    
    sqlx::query(&format!("DROP DATABASE IF EXISTS `{db_name}`"))
    .execute(&mut *conn)
    .await
    .map_err(|e| 
    {
        error!("Failed to drop database '{}': {}", db_name, e);
        DatabaseErrorCode::DeprovisioningFailed
    })?;

    sqlx::query(&format!("DROP USER IF EXISTS `{username}`@'%'"))
    .execute(&mut *conn)
    .await
    .map_err(|e| 
    {
        error!("Failed to drop user '{}': {}", username, e);
        DatabaseErrorCode::DeprovisioningFailed
    })?;

    Ok(())
}

pub async fn get_database_by_owner(pool: &PgPool, owner: &str) -> Result<Option<Database>, AppError>
{
    sqlx::query_as("SELECT * FROM databases WHERE owner_login = $1")
        .bind(owner)
        .fetch_optional(pool)
        .await
        .map_err(|e|
        {
            error!("Failed to fetch database for owner {}: {}", owner, e);
            AppError::InternalServerError
        })
}

pub async fn get_database_by_id_and_owner(pool: &PgPool, db_id: i32, owner: &str, is_admin: bool) -> Result<Option<Database>, AppError>
{
    if is_admin 
    {
        return sqlx::query_as("SELECT * FROM databases WHERE id = $1")
            .bind(db_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| AppError::InternalServerError);
    }

    sqlx::query_as("SELECT * FROM databases WHERE id = $1 AND owner_login = $2")
        .bind(db_id)
        .bind(owner)
        .fetch_optional(pool)
        .await
        .map_err(|_| AppError::InternalServerError)
}

pub async fn get_database_by_project_id(pool: &PgPool, project_id: i32) -> Result<Option<Database>, AppError>
{
    sqlx::query_as("SELECT * FROM databases WHERE project_id = $1")
        .bind(project_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| AppError::InternalServerError)
}

pub async fn link_database_to_project(pool: &PgPool, db_id: i32, project_id: i32, owner: &str) -> Result<(), AppError>
{
    let result = sqlx::query("UPDATE databases SET project_id = $1 WHERE id = $2 AND owner_login = $3")
        .bind(project_id)
        .bind(db_id)
        .bind(owner)
        .execute(pool)
        .await
        .map_err(|_| AppError::InternalServerError)?;
    
    if result.rows_affected() == 0 {
        return Err(DatabaseErrorCode::NotFound.into());
    }
    Ok(())
}

pub async fn unlink_database_from_project(pool: &PgPool, project_id: i32, owner: &str) -> Result<(), AppError>
{
    let result = sqlx::query("UPDATE databases SET project_id = NULL WHERE project_id = $1 AND owner_login = $2")
        .bind(project_id)
        .bind(owner)
        .execute(pool)
        .await
        .map_err(|_| AppError::InternalServerError)?;
        
    if result.rows_affected() == 0 {
        return Err(DatabaseErrorCode::NotFound.into());
    }
    Ok(())
}

pub async fn provision_and_link_database_tx<'a>(
    tx: &mut Transaction<'a, Postgres>,
    mariadb_pool: &MySqlPool,
    owner_login: &str,
    project_id: i32,
    encryption_key: &[u8],
) -> Result<(), AppError>
{

    let db_name = format!("{DB_PREFIX}_{owner_login}");
    let username = db_name.clone();
    let password = generate_password();

    if let Err(e) = execute_mariadb_provisioning(mariadb_pool, &db_name, &username, &password).await
    {
        warn!("MariaDB provisioning failed during transaction for user '{}'. Error: {}", owner_login, e);
        if let Err(e) = execute_mariadb_deprovisioning(mariadb_pool, &db_name, &username).await 
        {
            error!("Failed to rollback MariaDB provisioning for user '{}': {}", owner_login, e);
        }
        return Err(e);
    }
    
    let encrypted_password_vec = crypto_service::encrypt(&password, encryption_key)?;
    let encrypted_password = BASE64_STANDARD.encode(encrypted_password_vec);

    let insert_result = sqlx::query(
        "INSERT INTO databases (owner_login, database_name, username, encrypted_password, project_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(owner_login)
    .bind(&db_name)
    .bind(&username)
    .bind(&encrypted_password)
    .bind(project_id)
    .execute(&mut **tx)
    .await;

    if let Err(db_error) = insert_result
    {
        error!("Failed to persist database metadata for user '{}' in transaction: {}", owner_login, db_error);
        if let Err(e) = execute_mariadb_deprovisioning(mariadb_pool, &db_name, &username).await 
        {
            error!("Failed to rollback MariaDB provisioning for user '{}': {}", owner_login, e);
        }
        return Err(AppError::ProjectError(ProjectErrorCode::ProjectCreationFailedWithDatabaseError));
    }

    Ok(())
}

pub fn create_db_details_response(db: Database, config: &Config, encryption_key: &[u8]) -> Result<DatabaseDetailsResponse, AppError>
{
    let encrypted_pass_vec = BASE64_STANDARD.decode(&db.encrypted_password).map_err(|_| AppError::InternalServerError)?;
    let password = crypto_service::decrypt(&encrypted_pass_vec, encryption_key)?;

    Ok(DatabaseDetailsResponse 
    {
        id: db.id,
        owner_login: db.owner_login,
        database_name: db.database_name,
        username: db.username,
        password,
        project_id: db.project_id,
        host: config.mariadb_public_host.clone(),
        port: config.mariadb_public_port,
        created_at: db.created_at,
    })
}