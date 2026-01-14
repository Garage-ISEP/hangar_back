use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{sync::{RwLock, broadcast}, time::interval};
use tracing::{debug, error, info};

use crate::sse::types::SseEvent;

const BROADCAST_CAPACITY: usize = 1000;

#[derive(Clone)]
pub struct SseManager 
{
    /// Canal pour les admins (dashboard admin)
    admin_tx: broadcast::Sender<SseEvent>,
    
    /// Canal pour tous les utilisateurs (notifications globales)
    all_tx: broadcast::Sender<SseEvent>,

    /// Canaux spécifiques par projet (project_id -> sender)
    project_channels: Arc<RwLock<HashMap<i32, broadcast::Sender<SseEvent>>>>,

    /// Canaux temporaires pour les créations en cours (user_login -> sender)
    /// Utilisé pendant /projects/create avant que le projet n'existe
    creation_channels: Arc<RwLock<HashMap<String, broadcast::Sender<SseEvent>>>>,
}

impl SseManager 
{
    pub fn new() -> Self 
    {
        let (admin_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (all_tx, _) = broadcast::channel(BROADCAST_CAPACITY);

        Self 
        {
            admin_tx,
            all_tx,
            project_channels: Arc::new(RwLock::new(HashMap::new())),
            creation_channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn admin_subscriber_count(&self) -> usize
    {
        self.admin_tx.receiver_count()
    }

    pub fn all_subscriber_count(&self) -> usize
    {
        self.all_tx.receiver_count()
    }

    pub async fn project_subscriber_count(&self, project_id: i32) -> usize 
    {
        let map = self.project_channels.read().await;

        map.get(&project_id)
            .map(|tx| tx.receiver_count())
            .unwrap_or(0)
    }

    pub async fn active_project_channels(&self) -> usize 
    {
        let map = self.project_channels.read().await;
        map.len()
    }

    pub async fn active_creation_channels(&self) -> usize
    {
        let map = self.creation_channels.read().await;
        map.len()
    }

    // ========================================================================
    // Émission d'événements
    // ========================================================================
    
    /// Émet un événement visible uniquement par les admins
    /// 
    /// Cas d'usage :
    /// - Erreurs sur un projet
    /// - Métriques globales de la plateforme
    /// - Projets actifs/inactifs
    /// - Alertes système
    pub fn emit_to_admin(&self, event: SseEvent) 
    {
        let subscriber_count = self.admin_tx.receiver_count();

        if subscriber_count == 0 
        {
            debug!("No admin subscribers, event dropped: {:?}", event.event_type());
            return;
        }

        match self.admin_tx.send(event.clone()) 
        {
            Ok(count) => 
            {
                debug!("Admin event '{}' sent to {} admin(s)", event.event_type(), count);
            }
            Err(e) => 
            {
                error!("Failed to send admin event: {:?}", e);
            }
        }
    }

    /// Émet un événement visible par tous les utilisateurs connectés
    /// 
    /// Cas d'usage :
    /// - Maintenance planifiée
    /// - Annonces générales
    /// - Alertes globales
    pub fn emit_to_all(&self, event: SseEvent)
    {
        let subscriber_count = self.all_tx.receiver_count();
        
        if subscriber_count == 0
        {
            debug!("No subscribers on 'all' channel, event dropped: {:?}", event.event_type());
            return;
        }
        
        match self.all_tx.send(event.clone())
        {
            Ok(count) =>
            {
                info!("Global event '{}' sent to {} user(s)", event.event_type(), count);
            }
            Err(e) =>
            {
                error!("Failed to send global event: {:?}", e);
            }
        }
    }

    /// Émet un événement sur le canal d'un projet spécifique
    /// 
    /// Cas d'usage :
    /// - Métriques du projet (CPU/RAM)
    /// - Status du container
    /// - Logs
    /// - Événements de déploiement
    pub async fn emit_to_project(&self, project_id: i32, event: SseEvent) 
    {
        let tx = 
        {
            let mut map = self.project_channels.write().await;

            map.entry(project_id)
                .or_insert_with(|| broadcast::channel(BROADCAST_CAPACITY).0)
                .clone()
        };

        let subscriber_count = tx.receiver_count();

        if subscriber_count == 0 
        {
            debug!("No subscribers for project {}, event dropped: {:?}", project_id, event.event_type());

            // Nettoyer le canal si personne n'écoute
            self.cleanup_project_channel(project_id).await;
            return;
        }

        match tx.send(event.clone()) 
        {
            Ok(count) => 
            {
                debug!("Project {} event '{}' sent to {} client(s)", project_id, event.event_type(), count);
            }
            Err(e) => 
            {
                error!("Failed to send event to project {}: {:?}", project_id, e);
            }
        }
    }

    /// Émet un événement sur le canal de création temporaire d'un utilisateur
    /// 
    /// Cas d'usage :
    /// - Événements de création de projet (avant que project_id n'existe)
    /// - Pulling image
    /// - Scanning
    /// - Building
    /// 
    /// Le canal est automatiquement nettoyé après utilisation.
    pub async fn emit_to_creation(&self, user_login: &str, event: SseEvent)
    {
        let tx = 
        {
            let mut map = self.creation_channels.write().await;

            map.entry(user_login.to_string())
                .or_insert_with(|| broadcast::channel(BROADCAST_CAPACITY).0)
                .clone()
        };
        
        let subscriber_count = tx.receiver_count();
        
        if subscriber_count == 0
        {
            debug!(
                "No subscribers for creation channel '{}', event dropped: {:?}",
                user_login,
                event.event_type()
            );
            self.cleanup_creation_channel(user_login).await;
            return;
        }
        
        match tx.send(event.clone())
        {
            Ok(count) =>
            {
                debug!(
                    "Creation event '{}' sent to user '{}' ({} subscriber(s))",
                    event.event_type(),
                    user_login,
                    count
                );
            }
            Err(e) =>
            {
                error!("Failed to send creation event to '{}': {:?}", user_login, e);
            }
        }
    }

    /// S'abonne au canal admin (réservé aux admins)
    pub fn subscribe_admin(&self) -> broadcast::Receiver<SseEvent> 
    {
        let rx = self.admin_tx.subscribe();
        info!("New admin SSE subscription (total: {})", self.admin_subscriber_count());
        rx
    }

    /// S'abonne au canal "all" (tous les utilisateurs)
    pub fn subscribe_all(&self) -> broadcast::Receiver<SseEvent>
    {
        let rx = self.all_tx.subscribe();
        info!("New 'all' SSE subscription (total: {})", self.all_subscriber_count());
        rx
    }

    /// S'abonne aux événements d'un projet spécifique
    pub async fn subscribe_to_project(&self, project_id: i32) -> broadcast::Receiver<SseEvent> 
    {
        let tx = 
        {
            let mut map = self.project_channels.write().await;
            map.entry(project_id)
                .or_insert_with(|| broadcast::channel(BROADCAST_CAPACITY).0)
                .clone()
        };

        let rx = tx.subscribe();

        let subscriber_count = tx.receiver_count();
        info!(
            "New SSE subscription for project {} (total for project: {})",
            project_id, subscriber_count
        );
        rx
    }

    /// S'abonne au canal de création temporaire d'un utilisateur
    /// 
    /// Utilisé pendant `/projects/create` pour recevoir les événements
    /// de création en temps réel avant que le projet n'existe.
    pub async fn subscribe_to_creation(&self, user_login: &str) -> broadcast::Receiver<SseEvent>
    {
        let tx = 
        {
            let mut map = self.creation_channels.write().await;
            map.entry(user_login.to_string())
                .or_insert_with(|| broadcast::channel(BROADCAST_CAPACITY).0)
                .clone()
        };
        
        let rx = tx.subscribe();
        
        debug!("User '{}' subscribed to creation channel", user_login);
        
        rx
    }

    pub async fn cleanup_project_channel(&self, project_id: i32) 
    {
        let remove = 
        {
            let map = self.project_channels.read().await;
            map.get(&project_id)
                .map(|tx| tx.receiver_count() == 0)
                .unwrap_or(false)
        };

        if remove 
        {
            let mut map = self.project_channels.write().await;
            if map.get(&project_id).map(|tx| tx.receiver_count() == 0).unwrap_or(false)
            {
                map.remove(&project_id);
                debug!("Cleaned up empty project channel for project {}", project_id);
            }
        }
    }

    pub async fn cleanup_creation_channel(&self, user_login: &str) 
    {
        let remove = 
        {
            let map = self.creation_channels.read().await;
            map.get(user_login)
                .map(|tx| tx.receiver_count() == 0)
                .unwrap_or(false)
        };

        if remove 
        {
            let mut map = self.creation_channels.write().await;
            if map.get(user_login).map(|tx| tx.receiver_count() == 0).unwrap_or(false)
            {
                map.remove(user_login);
                debug!("Cleaned up empty creation channel for user '{}'", user_login);
            }
        }
    }

    pub async fn cleanup_empty_channels(&self) 
    {
        let mut removed_projects = 0;
        let mut removed_creations = 0;
        
        // --- Project channels ---
        {
            let mut map = self.project_channels.write().await;
            map.retain(|project_id, tx| 
            {
                let has_subscribers = tx.receiver_count() > 0;
                if !has_subscribers 
                {
                    debug!("Removing empty channel for project {}", project_id);
                    removed_projects += 1;
                }
                has_subscribers
            });
        }

        // --- Creation channels ---
        {
            let mut map = self.creation_channels.write().await;
            map.retain(|user_login, tx| 
            {
                let has_subscribers = tx.receiver_count() > 0;
                if !has_subscribers 
                {
                    debug!(
                        "Removing empty creation channel for user '{}'",
                        user_login
                    );
                    removed_creations += 1;
                }
                has_subscribers
            });
        }

        if removed_projects > 0 || removed_creations > 0 
        {
            info!(
                "Cleaned up {} project channel(s) and {} creation channel(s)",
                removed_projects,
                removed_creations
            );
        }
    }

    pub async fn stats(&self) -> SseManagerStats
    {
        let total_project_subscribers = 
        {
            let map = self.project_channels.read().await;
            map.values()
                .map(|tx| tx.receiver_count())
                .sum()
        };

        SseManagerStats
        {
            admin_subscribers: self.admin_subscriber_count(),
            all_subscribers: self.all_subscriber_count(),
            active_project_channels: self.active_project_channels().await,
            active_creation_channels: self.active_creation_channels().await,
            total_project_subscribers,
        }
    }

    pub async fn get_active_project_ids(&self) -> Vec<i32> 
    {
        let map = self.project_channels.read().await;
        map.iter()
            .filter(|(_, tx)| tx.receiver_count() > 0)
            .map(|(id, _)| *id)
            .collect()
    }

}

impl Default for SseManager 
{
    fn default() -> Self 
    {
        Self::new()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SseManagerStats 
{
    pub admin_subscribers: usize,
    pub all_subscribers: usize,
    pub active_project_channels: usize,
    pub active_creation_channels: usize,
    pub total_project_subscribers: usize,
}

pub async fn start_cleanup_task(manager: SseManager, mut shutdown_signal: tokio::sync::broadcast::Receiver<()>) 
{
    info!("Starting SSE Manager cleanup task");
    let mut interval = interval(Duration::from_secs(300));
    loop 
    {
        tokio::select! 
        {
            _ = shutdown_signal.recv() => 
            {
                info!("SSE Manager cleanup task shutting down");
                break;
            }
            _ = interval.tick() => {}
        }
        manager.cleanup_empty_channels().await;
    }
}
