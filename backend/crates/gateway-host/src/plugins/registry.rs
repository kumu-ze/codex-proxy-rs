use super::management::{Catalog, Record};
use super::{PluginError, PluginPackage, PluginProcess};
use async_trait::async_trait;
use gateway_admin::ports::plugins::{
    PluginManagement, PluginOperationError, PluginOperations, PluginStatus,
};
use gateway_core::engine::extensions::{
    ExtensionDecision, ExtensionRequest, ExtensionUnavailable, RequestExtension,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, RwLock, Semaphore};

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    config: PluginConfig,
    services: Mutex<Option<super::services::ServiceEndpoint>>,
    admission: Semaphore,
    request_openai: bool,
    version: String,
    menu_label: Option<String>,
    process: Mutex<Option<PluginProcess>>,
    enabled: AtomicBool,
    available: AtomicBool,
}
/// 控制面串行修改目录；数据面只短暂取得快照，不等待下载或持久化。
#[derive(Clone)]
pub struct PluginRegistry {
    entries: Arc<RwLock<BTreeMap<String, Arc<Entry>>>>,
    services:
        Arc<std::sync::OnceLock<Arc<dyn gateway_core::engine::extensions::ExtensionServices>>>,
    catalog: Option<Arc<Catalog>>,
    management: Arc<Mutex<()>>,
    stopping: Arc<AtomicBool>,
}
impl PluginRegistry {
    pub async fn start(configs: Vec<PluginConfig>) -> Result<Arc<Self>, PluginError> {
        Self::start_inner(configs, None).await
    }
    pub async fn start_managed(
        configs: Vec<PluginConfig>,
        root: &Path,
    ) -> Result<Arc<Self>, PluginError> {
        Self::start_inner(configs, Some(Arc::new(Catalog::open(root)?))).await
    }
    async fn start_inner(
        configs: Vec<PluginConfig>,
        catalog: Option<Arc<Catalog>>,
    ) -> Result<Arc<Self>, PluginError> {
        let records = match &catalog {
            Some(c) => c.load(configs)?,
            None => configs
                .into_iter()
                .map(|config| Record {
                    config,
                    enabled: true,
                })
                .collect(),
        };
        if records.len() > 16 {
            return Err(PluginError::InvalidPackage);
        }
        let mut entries = BTreeMap::new();
        for record in records {
            let package = PluginPackage::open(&record.config.directory)?;
            std::fs::create_dir_all(&record.config.data_directory).map_err(|_| PluginError::Io)?;
            let config = PluginConfig {
                directory: package.root().into(),
                data_directory: record
                    .config
                    .data_directory
                    .canonicalize()
                    .map_err(|_| PluginError::Io)?,
            };
            let id = package.manifest().id.clone();
            if entries.contains_key(&id) {
                return Err(PluginError::InvalidPackage);
            }
            entries.insert(
                id,
                Arc::new(Entry {
                    config,
                    services: Mutex::new(None),
                    admission: Semaphore::new(16),
                    request_openai: package
                        .manifest()
                        .capabilities
                        .iter()
                        .any(|c| c == "request.openai"),
                    version: package.manifest().version.clone(),
                    menu_label: package.manifest().menu_label.clone(),
                    process: Mutex::new(None),
                    enabled: AtomicBool::new(record.enabled),
                    available: AtomicBool::new(false),
                }),
            );
        }
        // 在运行任何代码之前核对程序和数据目录，避免跨插件覆盖。
        for entry in entries.values() {
            for other in entries.values() {
                if overlaps(&entry.config.data_directory, &other.config.directory)
                    || (!Arc::ptr_eq(entry, other)
                        && overlaps(&entry.config.data_directory, &other.config.data_directory))
                {
                    return Err(PluginError::InvalidPackage);
                }
            }
        }
        let registry = Arc::new(Self {
            entries: Arc::new(RwLock::new(entries)),
            services: Arc::new(std::sync::OnceLock::new()),
            catalog,
            management: Arc::new(Mutex::new(())),
            stopping: Arc::new(AtomicBool::new(false)),
        });
        registry.save(&registry.snapshot().await, None)?;
        for entry in registry.snapshot().await.values() {
            if entry.enabled.load(Ordering::Acquire)
                && let Err(error) = registry.launch(entry).await
            {
                if registry.catalog.is_none() {
                    return Err(error);
                }
                // 已安装插件启动失败保持启用但不可用，不能静默绕过请求保护。
                tracing::warn!("plugin startup failed; administrator can retry enabling");
            }
        }
        Ok(registry)
    }
    async fn snapshot(&self) -> BTreeMap<String, Arc<Entry>> {
        self.entries.read().await.clone()
    }
    fn save(
        &self,
        entries: &BTreeMap<String, Arc<Entry>>,
        change: Option<(&str, bool)>,
    ) -> Result<(), PluginError> {
        if let Some(catalog) = &self.catalog {
            let records = entries
                .iter()
                .map(|(id, entry)| Record {
                    config: entry.config.clone(),
                    enabled: change.filter(|(key, _)| *key == id).map_or_else(
                        || entry.enabled.load(Ordering::Acquire),
                        |(_, enabled)| enabled,
                    ),
                })
                .collect::<Vec<_>>();
            catalog.save(&records)?;
        }
        Ok(())
    }
    async fn launch(&self, entry: &Entry) -> Result<(), PluginError> {
        let package = PluginPackage::open(&entry.config.directory)?;
        let endpoint = if package
            .manifest()
            .capabilities
            .iter()
            .any(|c| c == "provider.openai")
        {
            Some(super::services::ServiceEndpoint::start(self.services.clone()).await?)
        } else {
            None
        };
        let process = PluginProcess::start_with_services(
            &package,
            &entry.config.data_directory,
            Duration::from_secs(5),
            endpoint.as_ref(),
        )
        .await?;
        *entry.process.lock().await = Some(process);
        *entry.services.lock().await = endpoint;
        entry.available.store(true, Ordering::Release);
        Ok(())
    }
    async fn stop_entry(entry: &Entry) {
        entry.available.store(false, Ordering::Release);
        // 调用最长五秒，停用先关闭入口，等待当前调用完成再回收进程与回调服务。
        if let Some(mut process) = entry.process.lock().await.take() {
            let _ = process.stop().await;
        }
        entry.services.lock().await.take();
    }
    pub fn attach_services(
        &self,
        services: Arc<dyn gateway_core::engine::extensions::ExtensionServices>,
    ) -> Result<(), PluginError> {
        self.services
            .set(services)
            .map_err(|_| PluginError::InvalidPackage)
    }
    pub fn attach_provider_services(
        &self,
        provider: Arc<dyn gateway_core::engine::extensions::ExtensionServices>,
        proxies: Arc<dyn gateway_admin::ports::proxy::ProxyStore>,
    ) -> Result<(), PluginError> {
        self.attach_services(Arc::new(super::services::ProviderServices {
            provider,
            proxies,
        }))
    }
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        let _guard = self.management.lock().await;
        let entries = self.snapshot().await;
        for entry in entries.values() {
            entry.available.store(false, Ordering::Release);
        }
        futures::future::join_all(entries.values().map(|entry| Self::stop_entry(entry))).await;
    }
    async fn apply(&self, operation: PluginManagement) -> Result<(), PluginOperationError> {
        let _guard = self
            .management
            .try_lock()
            .map_err(|_| PluginOperationError::Unavailable)?;
        if self.stopping.load(Ordering::Acquire) {
            return Err(PluginOperationError::Unavailable);
        }
        let catalog = self
            .catalog
            .as_ref()
            .ok_or(PluginOperationError::Unavailable)?;
        let mut entries = self.snapshot().await;
        match operation {
            PluginManagement::Install { url, sha256 } => {
                if entries.len() >= 16 {
                    return Err(PluginOperationError::Conflict);
                }
                let staged = super::management::download(catalog, &url, sha256.as_deref())
                    .await
                    .map_err(|_| PluginOperationError::Package)?;
                let package =
                    PluginPackage::open(&staged.path).map_err(|_| PluginOperationError::Package)?;
                let id = package.manifest().id.clone();
                if entries.contains_key(&id) {
                    return Err(PluginOperationError::Conflict);
                }
                let data_directory = catalog.root.join("data").join(&id);
                std::fs::create_dir_all(&data_directory)
                    .map_err(|_| PluginOperationError::Storage)?;
                let data_directory = data_directory
                    .canonicalize()
                    .map_err(|_| PluginOperationError::Storage)?;
                let packages = catalog.root.join("packages");
                std::fs::create_dir_all(&packages).map_err(|_| PluginOperationError::Storage)?;
                let packages = packages
                    .canonicalize()
                    .map_err(|_| PluginOperationError::Storage)?;
                let target = packages.join(format!("{}-{}", id, package.manifest().version));
                if overlaps(&data_directory, &target)
                    || entries.values().any(|entry| {
                        overlaps(&data_directory, &entry.config.data_directory)
                            || overlaps(&data_directory, &entry.config.directory)
                            || overlaps(&target, &entry.config.data_directory)
                    })
                {
                    return Err(PluginOperationError::Invalid);
                }
                let package = super::install_package(&staged.path, &packages)
                    .map_err(|_| PluginOperationError::Package)?;
                let entry = Arc::new(Entry {
                    config: PluginConfig {
                        directory: package.root().into(),
                        data_directory,
                    },
                    services: Mutex::new(None),
                    admission: Semaphore::new(16),
                    request_openai: package
                        .manifest()
                        .capabilities
                        .iter()
                        .any(|c| c == "request.openai"),
                    version: package.manifest().version.clone(),
                    menu_label: package.manifest().menu_label.clone(),
                    process: Mutex::new(None),
                    enabled: AtomicBool::new(false),
                    available: AtomicBool::new(false),
                });
                entries.insert(id.clone(), entry);
                if self.save(&entries, None).is_err() {
                    let _ = std::fs::remove_dir_all(package.root());
                    return Err(PluginOperationError::Storage);
                }
                *self.entries.write().await = entries;
                tracing::info!(plugin_id = %id, action = "install", "plugin management completed");
            }
            operation => {
                let (id, action) = match &operation {
                    PluginManagement::Enable { id } => (id, "enable"),
                    PluginManagement::Disable { id } => (id, "disable"),
                    PluginManagement::Uninstall { id } => (id, "uninstall"),
                    PluginManagement::Install { .. } => unreachable!(),
                };
                let entry = entries
                    .get(id)
                    .cloned()
                    .ok_or(PluginOperationError::NotFound)?;
                match operation {
                    PluginManagement::Enable { .. } => {
                        if !entry.available.load(Ordering::Acquire) {
                            Self::stop_entry(&entry).await;
                            self.launch(&entry)
                                .await
                                .map_err(|_| PluginOperationError::Unavailable)?;
                        }
                        if self.save(&entries, Some((id, true))).is_err() {
                            Self::stop_entry(&entry).await;
                            return Err(PluginOperationError::Storage);
                        }
                        entry.enabled.store(true, Ordering::Release);
                    }
                    PluginManagement::Disable { .. } => {
                        self.save(&entries, Some((id, false)))
                            .map_err(|_| PluginOperationError::Storage)?;
                        entry.enabled.store(false, Ordering::Release);
                        Self::stop_entry(&entry).await;
                    }
                    PluginManagement::Uninstall { .. } => {
                        if entry.enabled.load(Ordering::Acquire) {
                            return Err(PluginOperationError::Invalid);
                        }
                        entries.remove(id);
                        self.save(&entries, None)
                            .map_err(|_| PluginOperationError::Storage)?;
                        Self::stop_entry(&entry).await;
                        *self.entries.write().await = entries;
                        // 仅删除宿主管理目录中的程序；旧配置外部目录及业务数据保持原样。
                        if entry.config.directory.parent()
                            == Some(catalog.root.join("packages").as_path())
                        {
                            let _ = std::fs::remove_dir_all(&entry.config.directory);
                        }
                    }
                    PluginManagement::Install { .. } => unreachable!(),
                }
                tracing::info!(plugin_id = %id, action, "plugin management completed");
            }
        }
        Ok(())
    }
}
fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}
#[async_trait]
impl RequestExtension for PluginRegistry {
    async fn before_send(
        &self,
        request: ExtensionRequest,
    ) -> Result<ExtensionDecision, ExtensionUnavailable> {
        let mut decision = ExtensionDecision::default();
        let entries = self.snapshot().await;
        for entry in entries.values().filter(|e| {
            e.request_openai && e.enabled.load(Ordering::Acquire) && request.provider == "openai"
        }) {
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
            if !entry.available.load(Ordering::Acquire) {
                return Err(ExtensionUnavailable);
            }
            let process = process.as_mut().ok_or(ExtensionUnavailable)?;
            let result = process
                .call(
                    "request.before_send",
                    serde_json::to_value(&request).map_err(|_| ExtensionUnavailable)?,
                    Duration::from_millis(500),
                )
                .await;
            let result = result.map_err(|_| {
                if !process.is_available() {
                    entry.available.store(false, Ordering::Release);
                }
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
    async fn manage(&self, operation: PluginManagement) -> Result<(), PluginOperationError> {
        // HTTP 断连不会取消已经接受的状态修改，避免磁盘和内存状态分离。
        let registry = self.clone();
        tokio::spawn(async move { registry.apply(operation).await })
            .await
            .map_err(|_| PluginOperationError::Unavailable)?
    }
    async fn list(&self) -> Vec<PluginStatus> {
        self.snapshot()
            .await
            .iter()
            .map(|(id, entry)| {
                if let Ok(mut process) = entry.process.try_lock()
                    && !process.as_mut().is_some_and(PluginProcess::is_available)
                {
                    entry.available.store(false, Ordering::Release);
                }
                PluginStatus {
                    id: id.clone(),
                    version: entry.version.clone(),
                    menu_label: entry.menu_label.clone(),
                    available: entry.available.load(Ordering::Acquire),
                    enabled: entry.enabled.load(Ordering::Acquire),
                }
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
        let entries = self.snapshot().await;
        let entry = entries.get(id).ok_or(PluginOperationError::NotFound)?;
        if !entry.available.load(Ordering::Acquire) {
            return Err(PluginOperationError::Unavailable);
        }
        // 不建立无限等待队列；同一插件忙时立即返回，避免拖慢整个管理面。
        let mut process = entry
            .process
            .try_lock()
            .map_err(|_| PluginOperationError::Unavailable)?;
        if !entry.available.load(Ordering::Acquire) {
            return Err(PluginOperationError::Unavailable);
        }
        let process = process.as_mut().ok_or(PluginOperationError::Unavailable)?;
        let result = process.call(method, input, Duration::from_secs(5)).await;
        if result.is_err() && !process.is_available() {
            entry.available.store(false, Ordering::Release);
        }
        result.map_err(|error| match error {
            PluginError::Rejected => PluginOperationError::Rejected,
            _ => PluginOperationError::Unavailable,
        })
    }
}
