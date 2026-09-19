use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use gateway_admin::ports::plugins::{PluginOperationError, PluginOperations, PluginStatus};
use gateway_core::engine::extensions::{
    ExtensionDecision, ExtensionRequest, ExtensionUnavailable, RequestExtension,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Mutex, Semaphore};

use super::{PluginError, PluginPackage, PluginProcess};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfig {
    pub directory: PathBuf,
    pub data_directory: PathBuf,
}

impl PluginConfig {
    pub fn resolve(&mut self, root: &Path) -> Result<(), PluginError> {
        if self.directory.as_os_str().is_empty() || self.data_directory.as_os_str().is_empty() {
            return Err(PluginError::InvalidPackage);
        }
        if self.directory.is_relative() {
            self.directory = root.join(&self.directory);
        }
        if self.data_directory.is_relative() {
            self.data_directory = root.join(&self.data_directory);
        }
        Ok(())
    }
}

struct Entry {
    admission: Semaphore,
    request_openai: bool,
    version: String,
    process: Mutex<PluginProcess>,
    available: AtomicBool,
}

/// 插件列表在启动时冻结；单个进程失效不影响其他插件或宿主。
pub struct PluginRegistry {
    entries: BTreeMap<String, Entry>,
}

impl PluginRegistry {
    pub async fn start(configs: Vec<PluginConfig>) -> Result<Arc<Self>, PluginError> {
        if configs.len() > 16 {
            return Err(PluginError::InvalidPackage);
        }
        let mut entries = BTreeMap::new();
        let mut directories = std::collections::BTreeSet::new();
        for config in configs {
            let package = PluginPackage::open(&config.directory)?;
            let id = package.manifest().id.clone();
            if entries.contains_key(&id) {
                return Err(PluginError::InvalidPackage);
            }
            std::fs::create_dir_all(&config.data_directory).map_err(|_| PluginError::Io)?;
            let data = config
                .data_directory
                .canonicalize()
                .map_err(|_| PluginError::Io)?;
            if data.starts_with(package.root())
                || package.root().starts_with(&data)
                || !directories.insert(data.clone())
            {
                return Err(PluginError::InvalidPackage);
            }
            let process = PluginProcess::start(&package, &data, Duration::from_secs(5)).await?;
            entries.insert(
                id,
                Entry {
                    admission: Semaphore::new(16),
                    request_openai: package
                        .manifest()
                        .capabilities
                        .iter()
                        .any(|c| c == "request.openai"),
                    version: package.manifest().version.clone(),
                    process: Mutex::new(process),
                    available: AtomicBool::new(true),
                },
            );
        }
        Ok(Arc::new(Self { entries }))
    }

    pub async fn shutdown(&self) {
        for entry in self.entries.values() {
            entry.available.store(false, Ordering::Release);
            if let Ok(mut process) =
                tokio::time::timeout(Duration::from_secs(6), entry.process.lock()).await
            {
                let _ = process.stop().await;
            }
        }
    }
}

#[async_trait]
impl RequestExtension for PluginRegistry {
    async fn before_send(
        &self,
        request: ExtensionRequest,
    ) -> Result<ExtensionDecision, ExtensionUnavailable> {
        let mut decision = ExtensionDecision::default();
        for entry in self
            .entries
            .values()
            .filter(|e| e.request_openai && request.provider == "openai")
        {
            if !entry.available.load(Ordering::Acquire) {
                return Err(ExtensionUnavailable);
            }
            let _permit = entry
                .admission
                .try_acquire()
                .map_err(|_| ExtensionUnavailable)?;
            let mut process =
                tokio::time::timeout(Duration::from_millis(500), entry.process.lock())
                    .await
                    .map_err(|_| ExtensionUnavailable)?;
            let result = process
                .call(
                    "request.before_send",
                    serde_json::to_value(&request).map_err(|_| ExtensionUnavailable)?,
                    Duration::from_millis(500),
                )
                .await;
            let result = result.map_err(|_| {
                entry.available.store(false, Ordering::Release);
                ExtensionUnavailable
            })?;
            let next: ExtensionDecision =
                serde_json::from_value(result).map_err(|_| ExtensionUnavailable)?;
            if next.values.len() > 8
                || next
                    .values
                    .iter()
                    .any(|(k, v)| k.len() > 64 || v.len() > 4096)
            {
                return Err(ExtensionUnavailable);
            }
            decision.deny |= next.deny;
            for (key, value) in next.values {
                // 多个插件不得静默覆盖同一字段。
                if decision.values.insert(key, value).is_some() {
                    return Err(ExtensionUnavailable);
                }
            }
        }
        Ok(decision)
    }
}

#[async_trait]
impl PluginOperations for PluginRegistry {
    async fn list(&self) -> Vec<PluginStatus> {
        self.entries
            .iter()
            .map(|(id, entry)| PluginStatus {
                id: id.clone(),
                version: entry.version.clone(),
                available: entry.available.load(Ordering::Acquire),
            })
            .collect()
    }

    async fn invoke(
        &self,
        id: &str,
        method: &str,
        input: Value,
    ) -> Result<Value, PluginOperationError> {
        // 管理页面不能调用宿主握手或未来的数据面内部方法。
        if !method.starts_with("admin.")
            || method.len() > 128
            || !method
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(PluginOperationError::Invalid);
        }
        let entry = self.entries.get(id).ok_or(PluginOperationError::NotFound)?;
        if !entry.available.load(Ordering::Acquire) {
            return Err(PluginOperationError::Unavailable);
        }
        // 不建立无限等待队列；同一插件忙时立即返回，避免拖慢整个管理面。
        let mut process = entry
            .process
            .try_lock()
            .map_err(|_| PluginOperationError::Unavailable)?;
        let result = process.call(method, input, Duration::from_secs(5)).await;
        if result.is_err() {
            entry.available.store(false, Ordering::Release);
        }
        result.map_err(|_| PluginOperationError::Unavailable)
    }
}
