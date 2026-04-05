//! Log connector — writes every entity change to `tracing::info`.
//!
//! Useful for development, debugging, and auditing. Enabled by default
//! when the connector system is active.

use std::future::Future;
use std::pin::Pin;

use tracing::info;
use uuid::Uuid;

use super::{Connector, EntityChangeEvent};
use crate::error::Result;

/// A connector that logs every entity change event via `tracing`.
pub struct LogConnector;

impl LogConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LogConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl Connector for LogConnector {
    fn name(&self) -> &str {
        "log"
    }

    fn on_entity_changed(
        &self,
        event: EntityChangeEvent,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            info!(
                entity_id = %event.entity_id,
                entity_type = %event.entity_type,
                tx_id = event.tx_id,
                changed_attributes = ?event.changed_attributes,
                attribute_count = event.attributes.len(),
                "connector: entity changed"
            );
            Ok(())
        })
    }

    fn on_entity_deleted(
        &self,
        entity_id: Uuid,
        entity_type: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        let entity_type = entity_type.to_owned();
        Box::pin(async move {
            info!(
                entity_id = %entity_id,
                entity_type = %entity_type,
                "connector: entity deleted"
            );
            Ok(())
        })
    }

    fn initialize(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async {
            info!("log connector initialized");
            Ok(())
        })
    }
}
