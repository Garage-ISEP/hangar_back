use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::model::project::ProjectMetrics;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseEvent 
{
    Deployment(DeploymentEvent),
    ContainerStatus(ContainerStatusEvent),
    Metrics(MetricsEvent),
    System(SystemEvent),
}

impl SseEvent 
{
    pub fn event_type(&self) -> &'static str 
    {
        match self 
        {
            Self::Deployment(_) => "deployment",
            Self::ContainerStatus(_) => "container_status",
            Self::Metrics(_) => "metrics",
            Self::System(_) => "system",
        }
    }

    pub fn generate_id(&self) -> String 
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        format!("{}_{}", self.event_type(), timestamp)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemEvent 
{
    pub level: SystemEventLevel,
    pub message: String,
    pub context: Option<serde_json::Value>,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SystemEventLevel 
{
    Info,
    Warning,
    Error,
}

impl SystemEvent
{
    pub fn info(message: String) -> Self
    {
        Self
        {
            level: SystemEventLevel::Info,
            message,
            context: None,
            timestamp: OffsetDateTime::now_utc(),
        }
    }
    
    pub fn warning(message: String) -> Self
    {
        Self
        {
            level: SystemEventLevel::Warning,
            message,
            context: None,
            timestamp: OffsetDateTime::now_utc(),
        }
    }
    
    pub fn error(message: String) -> Self
    {
        Self
        {
            level: SystemEventLevel::Error,
            message,
            context: None,
            timestamp: OffsetDateTime::now_utc(),
        }
    }
    
    pub fn with_context(mut self, context: serde_json::Value) -> Self
    {
        self.context = Some(context);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentEvent 
{
    pub project_id: i32,
    pub project_name: String,
    pub stage: DeploymentStage,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStage 
{
    Started,
    ValidatingInput,
    PullingImage { image_url: String },
    ImagePulled,
    ScanningImage,
    ImageScanned,
    CloningRepository { repo_url: String },
    RepositoryCloned,
    BuildingImage,
    ImageBuilt,
    GettingImageDigest,
    CreatingContainer,
    ContainerCreated,
    WaitingHealthCheck,
    HealthCheckPassed,
    ProvisioningDatabase,
    DatabaseProvisioned,
    LinkingDatabase,
    DatabaseLinked,
    CleaningUp,
    Completed { container_name: String },
    Failed { error: String, stage: String },
}

impl DeploymentEvent 
{
    pub fn new(project_id: i32, project_name: String, stage: DeploymentStage) -> Self 
    {
        Self 
        {
            project_id,
            project_name,
            stage,
            timestamp: OffsetDateTime::now_utc(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerStatusEvent
{
    pub project_id: i32,
    pub project_name: String,
    pub container_name: String,
    pub status: ContainerStatus,
    
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerStatus
{
    Created,
    Restarting,
    Running,
    Removing,
    Paused,
    Exited,
    Dead,
    Unknown,
}

impl From<bollard::secret::ContainerStateStatusEnum> for ContainerStatus
{
    fn from(status: bollard::secret::ContainerStateStatusEnum) -> Self
    {
        use bollard::secret::ContainerStateStatusEnum;
        
        match status
        {
            ContainerStateStatusEnum::CREATED => Self::Created,
            ContainerStateStatusEnum::RESTARTING => Self::Restarting,
            ContainerStateStatusEnum::RUNNING => Self::Running,
            ContainerStateStatusEnum::REMOVING => Self::Removing,
            ContainerStateStatusEnum::PAUSED => Self::Paused,
            ContainerStateStatusEnum::EXITED => Self::Exited,
            ContainerStateStatusEnum::DEAD => Self::Dead,
            ContainerStateStatusEnum::EMPTY => Self::Unknown,
        }
    }
}

impl From<Option<bollard::secret::ContainerStateStatusEnum>> for ContainerStatus
{
    fn from(status: Option<bollard::secret::ContainerStateStatusEnum>) -> Self
    {
        match status
        {
            Some(s) => Self::from(s),
            None => Self::Unknown,
        }
    }
}

impl ContainerStatusEvent
{
    pub fn new(
        project_id: i32,
        project_name: String,
        container_name: String,
        status: ContainerStatus,
    ) -> Self
    {
        Self
        {
            project_id,
            project_name,
            container_name,
            status,
            timestamp: OffsetDateTime::now_utc(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsEvent
{
    pub project_id: i32,
    pub project_name: String,

    #[serde(flatten)]
    pub metrics: ProjectMetrics,
    
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

impl MetricsEvent
{
    pub fn new(project_id: i32, project_name: String, metrics: ProjectMetrics) -> Self
    {
        Self
        {
            project_id,
            project_name,
            metrics,
            timestamp: OffsetDateTime::now_utc(),
        }
    }
}