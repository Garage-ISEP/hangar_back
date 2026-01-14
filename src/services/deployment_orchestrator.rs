use std::future::Future;

use tracing::{debug, error, info};

use crate::error::AppError;
use crate::sse::emitter::{emit_creation_deployment_stage, emit_deployment_stage};
use crate::sse::types::DeploymentStage;
use crate::state::AppState;

/// Orchestrateur de déploiement pour un projet.
///
/// Gère automatiquement l'émission d'événements SSE selon le contexte :
/// - Création de projet (project_id = None) → canal "creation"
/// - Mise à jour de projet (project_id = Some) → canal projet spécifique
pub struct DeploymentOrchestrator<'a>
{
    state: &'a AppState,
    project_name: String,
    user_login: String,
    project_id: Option<i32>,
}

impl<'a> DeploymentOrchestrator<'a>
{
    pub fn for_creation(state: &'a AppState, project_name: String, user_login: String) -> Self
    {
        Self 
        {
            state,
            project_name,
            user_login,
            project_id: None,
        }
    }

    pub fn for_update(
        state: &'a AppState,
        project_name: String,
        user_login: String,
        project_id: i32,
    ) -> Self
    {
        Self 
        {
            state,
            project_name,
            user_login,
            project_id: Some(project_id),
        }
    }

    pub fn set_project_id(&mut self, project_id: i32)
    {
        self.project_id = Some(project_id);
    }

    pub async fn emit_stage(&self, stage: DeploymentStage)
    {
        match self.project_id
        {
            Some(id) =>
            {
                debug!(
                    "Emitting stage {:?} for project '{}' (ID: {})",
                    stage, self.project_name, id
                );
                emit_deployment_stage(self.state, id, self.project_name.clone(), stage).await;
            }
            None =>
            {
                debug!(
                    "Emitting creation stage {:?} for project '{}' (user: {})",
                    stage, self.project_name, self.user_login
                );
                emit_creation_deployment_stage(
                    self.state,
                    &self.user_login,
                    self.project_name.clone(),
                    stage,
                    None,
                )
                .await;
            }
        }
    }

    /// Exécute une opération en l'encadrant avec une étape de déploiement.
    ///
    /// Si l'opération réussit, l'étape est émise avant l'exécution.
    /// Si elle échoue, un événement `Failed` est émis avec le détail de l'erreur.
    ///
    /// # Arguments
    /// * `stage` - L'étape à émettre avant l'opération
    /// * `operation_name` - Nom de l'opération (pour les logs d'erreur)
    /// * `f` - La future à exécuter
    pub async fn with_stage<F, T>(
        &self,
        stage: DeploymentStage,
        operation_name: &str,
        f: F,
    ) -> Result<T, AppError>
    where
        F: Future<Output = Result<T, AppError>>,
    {
        self.emit_stage(stage).await;

        match f.await
        {
            Ok(result) =>
            {
                debug!(
                    "Operation '{}' succeeded for project '{}'",
                    operation_name, self.project_name
                );
                Ok(result)
            }
            Err(e) =>
            {
                error!(
                    "Operation '{}' failed for project '{}': {}",
                    operation_name, self.project_name, e
                );

                let error_message = format!("{}", e);
                self.emit_stage(DeploymentStage::Failed 
                {
                    error: error_message,
                    stage: operation_name.to_string(),
                })
                .await;

                Err(e)
            }
        }
    }

    /// Exécute une séquence d'opérations avec émission d'étapes avant/après.
    ///
    /// Émet `before_stage` avant l'opération, puis `after_stage` si elle réussit.
    /// En cas d'échec, émet automatiquement un événement `Failed`.
    pub async fn with_stages<F, T>(
        &self,
        before_stage: DeploymentStage,
        after_stage: DeploymentStage,
        operation_name: &str,
        f: F,
    ) -> Result<T, AppError>
    where
        F: Future<Output = Result<T, AppError>>,
    {
        self.emit_stage(before_stage).await;

        match f.await
        {
            Ok(result) =>
            {
                debug!(
                    "Operation '{}' succeeded for project '{}'",
                    operation_name, self.project_name
                );
                self.emit_stage(after_stage).await;
                Ok(result)
            }
            Err(e) =>
            {
                error!(
                    "Operation '{}' failed for project '{}': {}",
                    operation_name, self.project_name, e
                );

                let error_message = format!("{}", e);
                self.emit_stage(DeploymentStage::Failed {
                    error: error_message,
                    stage: operation_name.to_string(),
                })
                .await;

                Err(e)
            }
        }
    }

    /// Émet l'étape de complétion avec les informations du container.
    pub async fn emit_completed(&self, container_name: String, project_id: i32)
    {
        info!("Deployment completed for project '{}' (container: {})", self.project_name, container_name);
        
        let stage = DeploymentStage::Completed { container_name };
        
        debug!("Emitting creation completion for project '{}' (ID: {}, user: {})", self.project_name, project_id, self.user_login);
        emit_creation_deployment_stage
        (
            self.state,
            &self.user_login,
            self.project_name.clone(),
            stage,
            Some(project_id),
        ).await;
    }

    /// Émet une étape d'échec avec contexte.
    pub async fn emit_failed(&self, error: String, stage: String)
    {
        error!(
            "Deployment failed for project '{}' at stage '{}': {}",
            self.project_name, stage, error
        );
        self.emit_stage(DeploymentStage::Failed { error, stage }).await;
    }
}
