//! 插件控制面合同；进程和文件系统由 Host 实现。

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginStatus {
    pub id: String,
    pub version: String,
    pub available: bool,
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
}

#[async_trait]
pub trait PluginOperations: Send + Sync {
    async fn list(&self) -> Vec<PluginStatus>;
    async fn invoke(
        &self,
        id: &str,
        method: &str,
        input: Value,
    ) -> Result<Value, PluginOperationError>;
}
