//! 插件控制面合同；进程和文件系统由 Host 实现。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginStatus {
    pub id: String,
    pub version: String,
    pub available: bool,
    pub enabled: bool,
    pub menu_label: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum PluginManagement {
    Install { url: String, sha256: Option<String> },
    Enable { id: String },
    Disable { id: String },
    Uninstall { id: String },
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum PluginOperationError {
    #[error("plugin not found")]
    NotFound,
    #[error("invalid plugin operation")]
    Invalid,
    #[error("plugin operation rejected")]
    Rejected,
    #[error("plugin unavailable")]
    Unavailable,
    #[error("plugin package download or validation failed")]
    Package,
    #[error("plugin already installed or limit reached")]
    Conflict,
    #[error("plugin state could not be saved")]
    Storage,
}

#[async_trait]
pub trait PluginOperations: Send + Sync {
    async fn list(&self) -> Vec<PluginStatus>;
    async fn manage(&self, _operation: PluginManagement) -> Result<(), PluginOperationError> {
        Err(PluginOperationError::Unavailable)
    }
    async fn invoke(
        &self,
        id: &str,
        method: &str,
        input: Value,
    ) -> Result<Value, PluginOperationError>;
}
