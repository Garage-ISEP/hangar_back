use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::model::database::DatabaseDetailsResponse;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "project_source_type", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ProjectSourceType 
{
    Direct,
    Github,
}

#[derive(Debug, Serialize, Deserialize, Clone, sqlx::FromRow)]
pub struct Project 
{
    pub id: i32,
    pub name: String,
    pub owner: String,

    pub container_name: String,

    #[sqlx(rename = "source_type")]
    pub source: ProjectSourceType,

    pub source_url: String,
    #[sqlx(default)]
    pub source_branch: Option<String>,
    #[sqlx(default)]
    pub source_root_dir: Option<String>,
    pub deployed_image_tag: String,
    pub deployed_image_digest: String,

    #[sqlx(default)]
    pub env_vars: Option<serde_json::Value>,
    #[sqlx(default)]
    pub persistent_volume_path: Option<String>,
    #[sqlx(default)]
    pub volume_name: Option<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize, Clone)]
pub struct ProjectDetailsResponse 
{
    #[serde(flatten)]
    pub project: Project,
    pub participants: Vec<String>,
    pub database: Option<DatabaseDetailsResponse>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProjectMetrics 
{
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub memory_limit: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GlobalMetrics 
{
    pub total_projects: i64,
    pub running_containers: u64,
    pub total_cpu_usage: f64,
    pub total_memory_usage_mb: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DownProjectInfo 
{
    #[serde(flatten)]
    pub project: Project,
    pub stopped_at: String,
    pub downtime_seconds: i64,
}