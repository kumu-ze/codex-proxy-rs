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
    pub capabilities: Vec<String>,
    pub author: Option<String>,
    pub repository: Option<String>,
    pub update_supported: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginUpdateInfo {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub release_url: Option<String>,
    pub download_url: Option<String>,
    pub sha256: Option<String>,
    pub prerelease: bool,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum PluginManagement {
    Install {
        url: String,
        sha256: Option<String>,
        #[serde(rename = "proxyId")]
        proxy_id: Option<String>,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Uninstall {
        id: String,
    },
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
    #[error("existing plugin package differs from requested package")]
    PackageConflict,
    #[error("invalid update source response")]
    UpdateSource,
    #[error("plugin download failed")]
    Download,
    #[error("plugin download timed out")]
    DownloadTimeout,
    #[error("plugin download HTTP {0}")]
    DownloadHttp(u16),
    #[error("plugin checksum mismatch")]
    Checksum,
    #[error("download proxy unavailable")]
    Proxy,
    #[error("plugin already installed or limit reached")]
    Conflict,
    #[error("plugin state could not be saved")]
    Storage,
}

#[async_trait]
pub trait PluginOperations: Send + Sync {
    async fn list(&self) -> Vec<PluginStatus>;
    async fn check_update(
        &self,
        _id: &str,
        _proxy_id: Option<&str>,
    ) -> Result<PluginUpdateInfo, PluginOperationError> {
        Err(PluginOperationError::Invalid)
    }
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
