//! Provider-owned 打标：后台探测、私有持久化及按账号/模型隔离的注入。

use crate::{
    credential::{CodexCredentialRepository, CodexRuntimeAuthentication},
    transport::{CODEX_RESPONSES_PATH, endpoint_url, profile::CodexWireProfileState},
};
use futures::future::BoxFuture;
use gateway_admin::{
    model::tickets::*,
    ports::provider::{ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::{
    account::{CredentialState, ProviderAccount, ProviderAccountId},
    task::{ScheduledTask, WorkerCycleContext, WorkerTaskError},
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, Semaphore};

const HEADER: &str = "x-codex-turn-state";
const LOG_LIMIT: usize = 1000;
const MAX_PROBE_CONCURRENCY: usize = 12;

#[derive(Clone, Serialize, Deserialize)]
struct Ticket {
    account_id: String,
    credential_revision: u64,
    #[serde(default)]
    auth_binding: Option<String>,
    model: String,
    state: String,
    expires_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Persisted {
    revision: u64,
    settings: TicketSettings,
    #[serde(default)]
    proxy_url: String,
    #[serde(default)]
    proxy_pool: Vec<String>,
    #[serde(default)]
    proxy_names: Vec<String>,
    #[serde(default)]
    proxy_enabled: Vec<bool>,
    #[serde(default)]
    proxy_concurrency: Vec<usize>,
    tickets: Vec<Ticket>,
    #[serde(default)]
    logs: Vec<TicketLog>,
}

impl Default for Persisted {
    fn default() -> Self {
        Self {
            revision: 1,
            settings: TicketSettings::default(),
            proxy_url: String::new(),
            proxy_pool: Vec::new(),
            proxy_names: Vec::new(),
            proxy_enabled: Vec::new(),
            proxy_concurrency: Vec::new(),
            tickets: Vec::new(),
            logs: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Observations {
    last: BTreeMap<(String, String), TicketResult>,
    retry: BTreeMap<String, u64>,
    manual_retry: BTreeMap<String, u64>,
    busy: BTreeMap<(String, String), usize>,
    proxy_busy: BTreeMap<String, usize>,
    proxy_cursor: usize,
    worker_checked_at: Option<u64>,
}

impl Observations {
    fn retry_at(&self, id: &str, interval: u64, manual: bool) -> Option<u64> {
        let ordinary = self
            .last
            .iter()
            .filter(|((account, _), _)| account == id)
            .map(|(_, result)| result.checked_at.saturating_add(interval))
            .max()
            .unwrap_or(0);
        let retries = if manual {
            &self.manual_retry
        } else {
            &self.retry
        };
        Some(ordinary.max(retries.get(id).copied().unwrap_or(0))).filter(|time| *time > now())
    }
}

pub(crate) struct TicketService {
    repository: CodexCredentialRepository,
    profile: CodexWireProfileState,
    url: String,
    path: PathBuf,
    data: Arc<Mutex<Persisted>>,
    observations: Mutex<Observations>,
    persistence: Arc<AsyncMutex<()>>,
    request_slot: Semaphore,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}
fn error(kind: ProviderAdminErrorKind) -> ProviderAdminError {
    ProviderAdminError::new(kind)
}
fn valid_state(state: &str, length: usize) -> bool {
    state.len() == length
        && state.starts_with("gAAAAA")
        && state
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-=".contains(&c))
}
fn eligible(account: &ProviderAccount) -> bool {
    account.enabled()
        && account.authentication_kind() == "oauth"
        && account.credential_state() == CredentialState::Ready
}

// Cookie 和调度配置会推进 revision；票只绑定真正用于上游认证的身份材料。
fn auth_binding(
    account: &ProviderAccount,
    authentication: &CodexRuntimeAuthentication,
) -> Option<String> {
    let oauth = authentication.oauth()?;
    let mut hash = Sha256::new();
    for value in [
        account.id().as_str(),
        account.upstream_account_id().unwrap_or(""),
        account.upstream_user_id().unwrap_or(""),
        oauth.access_token.expose_secret(),
        oauth
            .refresh_token
            .as_ref()
            .map_or("", |v| v.expose_secret()),
        oauth.id_token.as_ref().map_or("", |v| v.expose_secret()),
    ] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    Some(hex::encode(hash.finalize()))
}

fn binding_matches(ticket: &Ticket, account: &ProviderAccount, binding: Option<&str>) -> bool {
    match ticket.auth_binding.as_deref() {
        Some(expected) => binding == Some(expected),
        // 旧票没有身份摘要，仅在原 revision 完全一致时允许兼容使用。
        None => ticket.credential_revision == account.revision().get(),
    }
}

impl TicketService {
    pub(crate) fn new(
        repository: CodexCredentialRepository,
        profile: CodexWireProfileState,
        base_url: &str,
        path: PathBuf,
    ) -> Result<Self, ProviderAdminError> {
        let mut data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Persisted>(&bytes)
                .map_err(|_| error(ProviderAdminErrorKind::Invalid))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Persisted::default(),
            Err(_) => return Err(error(ProviderAdminErrorKind::Internal)),
        };
        if data.proxy_pool.is_empty() && !data.proxy_url.is_empty() {
            data.proxy_pool = split_proxy_pool(&data.proxy_url);
        }
        if !data.settings.validate()
            || data
                .proxy_concurrency
                .iter()
                .any(|limit| !(1..=3).contains(limit))
            || data.proxy_pool.len() > 64
            || !data
                .proxy_pool
                .iter()
                .all(|p| !p.is_empty() && valid_proxy(p))
        {
            return Err(error(ProviderAdminErrorKind::Invalid));
        }
        data.tickets.retain(|t| {
            t.expires_at > now()
                && t.expires_at <= now() + 3600
                && valid_state(&t.state, t.state.len())
                && t.state.len() <= 4096
        });
        if data.logs.len() > LOG_LIMIT {
            data.logs.drain(..data.logs.len() - LOG_LIMIT);
        }
        let mut observations = Observations::default();
        for log in &data.logs {
            observations.last.insert(
                (log.result.account_id.clone(), log.result.model.clone()),
                log.result.clone(),
            );
            if let Some(retry_at) = log.retry_at.filter(|time| *time > now()) {
                if matches!(log.result.http_status, 429 | 401 | 403) {
                    observations
                        .manual_retry
                        .entry(log.result.account_id.clone())
                        .and_modify(|time| *time = (*time).max(retry_at))
                        .or_insert(retry_at);
                }
                observations
                    .retry
                    .entry(log.result.account_id.clone())
                    .and_modify(|time| *time = (*time).max(retry_at))
                    .or_insert(retry_at);
            }
        }
        Ok(Self {
            repository,
            profile,
            url: endpoint_url(base_url, CODEX_RESPONSES_PATH),
            path,
            data: Arc::new(Mutex::new(data)),
            observations: Mutex::new(observations),
            persistence: Arc::new(AsyncMutex::new(())),
            request_slot: Semaphore::new(MAX_PROBE_CONCURRENCY),
        })
    }

    // 写成功后才发布内存状态；锁覆盖整个替换，防止旧快照覆盖新配置。
    async fn commit(
        &self,
        next: Persisted,
        guard: OwnedMutexGuard<()>,
    ) -> Result<(), ProviderAdminError> {
        let path = self.path.clone();
        let data = Arc::clone(&self.data);
        // fsync 不占用异步执行线程；锁由写盘任务持有，即使请求取消也保证写盘与发布有序。
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let bytes =
                serde_json::to_vec(&next).map_err(|_| error(ProviderAdminErrorKind::Internal))?;
            atomic_write(&path, &bytes).map_err(|_| error(ProviderAdminErrorKind::Internal))?;
            *data
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
            Ok(())
        })
        .await
        .map_err(|_| error(ProviderAdminErrorKind::Internal))?
    }

    pub(crate) async fn panel(&self) -> Result<TicketPanel, ProviderAdminError> {
        let accounts = self
            .repository
            .list_for_provider()
            .await
            .map_err(|_| error(ProviderAdminErrorKind::Unavailable))?;
        let mut bindings = BTreeMap::new();
        for account in &accounts {
            if eligible(account)
                && let Ok(credential) = self.repository.load_runtime_credential(account).await
                && let Some(binding) = auth_binding(account, &credential.authentication)
            {
                bindings.insert(account.id().as_str().to_owned(), binding);
            }
        }
        let d = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let o = self
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rows = accounts
            .iter()
            .filter(|a| a.authentication_kind() == "oauth")
            .map(|a| {
                let id = a.id().as_str();
                let target = d.settings.target_length(id, a.plan_type());
                TicketAccountStatus {
                    id: id.into(),
                    name: a.name().into(),
                    plan: a.plan_type().map(str::to_owned),
                    eligible: eligible(a),
                    target_length: target,
                    policy: d.settings.accounts.get(id).cloned().unwrap_or_default(),
                    models: d
                        .settings
                        .models
                        .iter()
                        .map(|model| {
                            let ticket = d.tickets.iter().find(|t| {
                                t.account_id == id
                                    && eligible(a)
                                    && binding_matches(t, a, bindings.get(id).map(String::as_str))
                                    && t.model == *model
                                    && t.expires_at > now()
                                    && valid_state(&t.state, target)
                            });
                            let key = (id.to_owned(), model.clone());
                            TicketModelStatus {
                                model: model.clone(),
                                ready: ticket.is_some(),
                                expires_at: ticket.map(|t| t.expires_at),
                                last_result: o.last.get(&key).cloned(),
                                busy: o.busy.contains_key(&key),
                                retry_at: o.retry_at(id, d.settings.interval_seconds, false),
                                manual_retry_at: o.retry_at(
                                    id,
                                    d.settings.manual_interval_seconds,
                                    true,
                                ),
                            }
                        })
                        .collect(),
                }
            })
            .collect();
        let mut settings = d.settings.clone();
        settings.accounts.retain(|id, _| {
            accounts
                .iter()
                .any(|a| a.id().as_str() == id && a.authentication_kind() == "oauth")
        });
        Ok(TicketPanel {
            revision: d.revision,
            settings,
            proxy_configured: !d.proxy_pool.is_empty(),
            proxy_count: d.proxy_pool.len(),
            proxies: d
                .proxy_pool
                .iter()
                .enumerate()
                .map(|(index, proxy)| TicketProxyView {
                    enabled: d.proxy_enabled.get(index).copied().unwrap_or(true),
                    concurrency: d.proxy_concurrency.get(index).copied().unwrap_or(1),
                    in_flight: o
                        .proxy_busy
                        .get(&proxy_endpoint(proxy))
                        .copied()
                        .unwrap_or(0),
                    id: index.to_string(),
                    name: d
                        .proxy_names
                        .get(index)
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .unwrap_or_else(|| format!("代理 {}", index + 1)),
                    endpoint: proxy_endpoint(proxy),
                    has_authentication: url::Url::parse(proxy)
                        .is_ok_and(|u| !u.username().is_empty() || u.password().is_some()),
                })
                .collect(),
            accounts: rows,
            logs: d.logs.iter().rev().cloned().collect(),
            log_limit: LOG_LIMIT,
            worker_checked_at: o.worker_checked_at,
        })
    }

    pub(crate) async fn update(
        &self,
        update: TicketUpdate,
    ) -> Result<TicketPanel, ProviderAdminError> {
        if !(10..=86400).contains(&update.settings.interval_seconds) {
            return Err(error(ProviderAdminErrorKind::Invalid).with_public_message(
                "自动探测间隔应为 10–86400 秒；手动立即重试请将手动间隔设为 0",
            ));
        }
        if update.settings.manual_interval_seconds > 86400 {
            return Err(error(ProviderAdminErrorKind::Invalid)
                .with_public_message("手动探测间隔应为 0–86400 秒"));
        }
        if !(60..=3600).contains(&update.settings.ttl_seconds)
            || update.settings.refresh_before_seconds >= update.settings.ttl_seconds
        {
            return Err(error(ProviderAdminErrorKind::Invalid)
                .with_public_message("有效期应为 60–3600 秒，提前刷新时间必须小于有效期"));
        }
        if [
            update.proxy_pool.is_some(),
            update.proxy_url.is_some(),
            update.proxies.is_some(),
        ]
        .into_iter()
        .filter(|v| *v)
        .count()
            > 1
        {
            return Err(error(ProviderAdminErrorKind::Invalid)
                .with_public_message("请只提交代理池或旧版单代理字段之一"));
        }
        let requested_pool = update
            .proxy_pool
            .as_ref()
            .map(|pool| {
                pool.iter()
                    .map(String::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .or_else(|| {
                update
                    .proxy_url
                    .as_ref()
                    .map(|proxy| split_proxy_pool(proxy))
            });
        if !update.settings.validate() {
            return Err(error(ProviderAdminErrorKind::Invalid)
                .with_public_message("请检查目标长度、模型列表和账号策略；模型不能重复"));
        }
        if requested_pool
            .as_ref()
            .is_some_and(|pool| pool.len() > 64 || pool.iter().any(|item| !valid_proxy(item)))
        {
            return Err(error(ProviderAdminErrorKind::Invalid).with_public_message(
                "代理池最多 64 个，每行填写一个有效的 HTTP、HTTPS、SOCKS5 或 SOCKS5H 代理地址",
            ));
        }
        let accounts = self
            .repository
            .list_for_provider()
            .await
            .map_err(|_| error(ProviderAdminErrorKind::Unavailable))?;
        if update.settings.accounts.keys().any(|id| {
            !accounts
                .iter()
                .any(|a| a.id().as_str() == id && a.authentication_kind() == "oauth")
        }) {
            return Err(error(ProviderAdminErrorKind::Invalid)
                .with_public_message("账号列表已变化，请刷新页面后重新保存"));
        }
        let guard = Arc::clone(&self.persistence).lock_owned().await;
        let mut next = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if next.revision != update.revision {
            return Err(error(ProviderAdminErrorKind::Conflict));
        }
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| error(ProviderAdminErrorKind::Conflict))?;
        let old_settings = next.settings.clone();
        next.settings = update.settings;
        if let Some(entries) = update.proxies {
            if entries.len() > 64 {
                return Err(error(ProviderAdminErrorKind::Invalid)
                    .with_public_message("代理池最多 64 个代理"));
            }
            let mut urls = Vec::new();
            let mut names = Vec::new();
            let mut enabled = Vec::new();
            let mut concurrency = Vec::new();
            for entry in entries {
                let previous_index = entry.id.as_deref().and_then(|id| id.parse::<usize>().ok());
                let limit = entry
                    .concurrency
                    .or_else(|| previous_index.and_then(|i| next.proxy_concurrency.get(i).copied()))
                    .unwrap_or(1);
                if !(1..=3).contains(&limit) {
                    return Err(error(ProviderAdminErrorKind::Invalid)
                        .with_public_message("每个代理入口并发应为 1–3"));
                }
                enabled.push(
                    entry
                        .enabled
                        .or_else(|| previous_index.and_then(|i| next.proxy_enabled.get(i).copied()))
                        .unwrap_or(true),
                );
                concurrency.push(limit);
                let name = entry.name.trim();
                if name.is_empty()
                    || name.len() > 128
                    || name.chars().any(char::is_control)
                    || entry.saved_proxy_id.is_some()
                {
                    return Err(error(ProviderAdminErrorKind::Invalid)
                        .with_public_message("代理名称不能为空、不能包含控制字符且最多 128 字节"));
                }
                let url = match entry.url.filter(|s| !s.trim().is_empty()) {
                    Some(url) => url.trim().to_owned(),
                    None => entry
                        .id
                        .as_deref()
                        .and_then(|id| id.parse::<usize>().ok())
                        .and_then(|i| next.proxy_pool.get(i))
                        .cloned()
                        .ok_or_else(|| {
                            error(ProviderAdminErrorKind::Invalid)
                                .with_public_message("新代理需要完整地址；已有代理请刷新后再保存")
                        })?,
                };
                if !valid_proxy(&url) {
                    return Err(error(ProviderAdminErrorKind::Invalid)
                        .with_public_message("代理地址格式不合法"));
                }
                urls.push(url);
                names.push(name.to_owned());
            }
            next.proxy_pool = urls;
            next.proxy_names = names;
            next.proxy_enabled = enabled;
            next.proxy_concurrency = concurrency;
            next.proxy_url = next.proxy_pool.join("\n");
        }
        if let Some(pool) = requested_pool {
            next.proxy_enabled = vec![true; pool.len()];
            next.proxy_concurrency = vec![1; pool.len()];
            next.proxy_names = (1..=pool.len()).map(|i| format!("代理 {i}")).collect();
            next.proxy_url = pool.join("\n");
            next.proxy_pool = pool;
        }
        if next.settings.enabled && next.proxy_pool.is_empty() {
            return Err(error(ProviderAdminErrorKind::Invalid)
                .with_public_message("启用打标需要先配置代理池；清除代理池前请关闭打标"));
        }
        // 间隔、代理名称及手动/自动切换不改变已获票的身份和有效期。
        // 只淘汰被关闭、移除、目标长度不符或过期的票；缩短 TTL 只能收紧已有期限。
        next.tickets.retain_mut(|ticket| {
            let Some(account) = accounts
                .iter()
                .find(|a| a.id().as_str() == ticket.account_id)
            else {
                return false;
            };
            if next.settings.ttl_seconds < old_settings.ttl_seconds {
                ticket.expires_at = ticket
                    .expires_at
                    .saturating_sub(old_settings.ttl_seconds - next.settings.ttl_seconds);
            }
            next.settings.enabled
                && eligible(account)
                && next
                    .settings
                    .accounts
                    .get(&ticket.account_id)
                    .is_some_and(|p| p.mode != TicketMode::Off)
                && next.settings.models.contains(&ticket.model)
                && ticket.expires_at > now()
                && valid_state(
                    &ticket.state,
                    next.settings
                        .target_length(&ticket.account_id, account.plan_type()),
                )
        });
        self.commit(next, guard).await?;
        self.panel().await
    }

    pub(crate) fn get(
        &self,
        account: &ProviderAccount,
        authentication: &CodexRuntimeAuthentication,
        model: &str,
    ) -> Option<String> {
        let binding = auth_binding(account, authentication);
        let d = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = account.id().as_str();
        if !eligible(account)
            || !d.settings.enabled
            || !d.settings.inject
            || !d.settings.models.iter().any(|m| m == model)
            || d.settings
                .accounts
                .get(id)
                .is_none_or(|p| p.mode == TicketMode::Off)
        {
            return None;
        }
        let target = d.settings.target_length(id, account.plan_type());
        d.tickets
            .iter()
            .find(|t| {
                t.account_id == id
                    && binding_matches(t, account, binding.as_deref())
                    && t.model == model
                    && t.expires_at > now()
                    && valid_state(&t.state, target)
            })
            .map(|t| t.state.clone())
    }

    pub(crate) async fn probe(
        &self,
        input: TicketProbe,
    ) -> Result<TicketResult, ProviderAdminError> {
        self.probe_with_mode(input, false).await
    }

    async fn probe_with_mode(
        &self,
        input: TicketProbe,
        automatic: bool,
    ) -> Result<TicketResult, ProviderAdminError> {
        self.probe_on_proxy(input, automatic, None).await
    }

    async fn probe_on_proxy(
        &self,
        input: TicketProbe,
        automatic: bool,
        forced_proxy: Option<usize>,
    ) -> Result<TicketResult, ProviderAdminError> {
        let _permit = self
            .request_slot
            .try_acquire()
            .map_err(|_| error(ProviderAdminErrorKind::Conflict))?;
        let account_id = ProviderAccountId::new(input.account_id.clone())
            .map_err(|_| error(ProviderAdminErrorKind::Invalid))?;
        let account = self
            .repository
            .store()
            .get_account(&account_id)
            .await
            .map_err(|_| error(ProviderAdminErrorKind::Unavailable))?
            .filter(|a| eligible(a) && a.provider().as_str() == "openai")
            .ok_or_else(|| error(ProviderAdminErrorKind::Invalid))?;
        let snapshot = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if !snapshot.settings.enabled
            || !snapshot.settings.proxy_pool_enabled
            || snapshot.proxy_pool.is_empty()
            || !snapshot.settings.models.contains(&input.model)
            || snapshot
                .settings
                .accounts
                .get(&input.account_id)
                .is_none_or(|p| p.mode == TicketMode::Off)
            || (automatic
                && snapshot
                    .settings
                    .accounts
                    .get(&input.account_id)
                    .is_none_or(|p| p.mode != TicketMode::Auto))
        {
            return Err(error(ProviderAdminErrorKind::Invalid));
        }
        let (proxy, proxy_name, proxy_key) = {
            let mut o = self
                .observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let interval = if automatic {
                snapshot.settings.interval_seconds
            } else {
                snapshot.settings.manual_interval_seconds
            };
            if o.retry_at(&input.account_id, interval, !automatic)
                .is_some()
            {
                return Err(error(ProviderAdminErrorKind::Conflict)
                    .with_public_message("账号仍在探测间隔或上游错误退避中，请稍后重试"));
            }
            let key = (input.account_id.clone(), input.model.clone());
            if !automatic && o.busy.contains_key(&key) {
                return Err(error(ProviderAdminErrorKind::Conflict)
                    .with_public_message("该账号模型已有探测正在执行"));
            }
            let start = o.proxy_cursor;
            let index = (0..snapshot.proxy_pool.len())
                .map(|offset| (start + offset) % snapshot.proxy_pool.len())
                .find(|index| {
                    if forced_proxy.is_some_and(|forced| forced != *index)
                        || !snapshot.proxy_enabled.get(*index).copied().unwrap_or(true)
                    {
                        return false;
                    }
                    let endpoint = proxy_endpoint(&snapshot.proxy_pool[*index]);
                    // 重复入口共用限额，不因重命名、认证别名或列表重排而绕过并发约束。
                    let limit = snapshot
                        .proxy_pool
                        .iter()
                        .enumerate()
                        .filter(|(i, p)| {
                            snapshot.proxy_enabled.get(*i).copied().unwrap_or(true)
                                && proxy_endpoint(p) == endpoint
                        })
                        .map(|(i, _)| snapshot.proxy_concurrency.get(i).copied().unwrap_or(1))
                        .min()
                        .unwrap_or(1);
                    o.proxy_busy.get(&endpoint).copied().unwrap_or(0) < limit
                })
                .ok_or_else(|| {
                    error(ProviderAdminErrorKind::Conflict)
                        .with_public_message("代理池已暂停、全部代理已禁用或并发已满")
                })?;
            o.proxy_cursor = index.wrapping_add(1);
            let proxy = snapshot.proxy_pool[index].clone();
            let endpoint = proxy_endpoint(&proxy);
            *o.proxy_busy.entry(endpoint.clone()).or_default() += 1;
            *o.busy.entry(key).or_default() += 1;
            (
                proxy,
                snapshot
                    .proxy_names
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("代理 {}", index + 1)),
                endpoint,
            )
        };
        let _busy = BusyGuard {
            observations: &self.observations,
            key: (input.account_id.clone(), input.model.clone()),
            proxy: proxy_key,
        };
        let target = snapshot
            .settings
            .target_length(&input.account_id, account.plan_type());
        let started_at = now();
        let started = Instant::now();
        let credential = self
            .repository
            .load_runtime_credential(&account)
            .await
            .map_err(|_| error(ProviderAdminErrorKind::Unavailable))?;
        let binding = auth_binding(&account, &credential.authentication);
        let outcome = self
            .send(&account, &credential.authentication, &input.model, &proxy)
            .await;
        let (status, state, retry, message) =
            outcome.unwrap_or_else(|message| (0, String::new(), 60, message.into()));
        let current = self.repository.store().get_account(&account_id).await;
        let account_unchanged = if let Some(a) = current.as_ref().ok().and_then(|a| a.as_ref()) {
            if eligible(a) && a.plan_type() == account.plan_type() {
                self.repository
                    .load_runtime_credential(a)
                    .await
                    .is_ok_and(|c| {
                        binding.is_some() && auth_binding(a, &c.authentication) == binding
                    })
            } else {
                false
            }
        } else {
            false
        };
        let mut matched = status == 200 && valid_state(&state, target) && account_unchanged;
        let guard = Arc::clone(&self.persistence).lock_owned().await;
        let mut next = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if next.revision != snapshot.revision {
            matched = false;
        }
        let result = TicketResult {
            account_id: input.account_id.clone(),
            model: input.model.clone(),
            http_status: status,
            length: state.len(),
            matched,
            checked_at: now(),
            message: if next.revision != snapshot.revision || !account_unchanged {
                "策略或账号已改变，或账号状态无法复核，结果未采纳".into()
            } else if matched {
                "已匹配目标规则".into()
            } else {
                message
            },
        };
        if matched {
            next.tickets.retain(|t| {
                t.expires_at > now()
                    && !(t.account_id == input.account_id && t.model == input.model)
            });
            next.tickets.push(Ticket {
                account_id: input.account_id.clone(),
                credential_revision: account.revision().get(),
                auth_binding: binding,
                model: input.model.clone(),
                state,
                expires_at: now() + snapshot.settings.ttl_seconds,
            });
        }
        let delay = match status {
            429 => retry.max(60),
            401 | 403 => 900,
            0 => 60,
            _ => 0,
        };
        let retry_at = result.checked_at.saturating_add(delay);
        next.logs.push(TicketLog {
            id: uuid::Uuid::new_v4().to_string(),
            account_name: account.name().into(),
            trigger: if automatic {
                TicketMode::Auto
            } else {
                TicketMode::Manual
            },
            proxy_endpoint: proxy_endpoint(&proxy),
            proxy_name,
            target_length: target,
            started_at,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            retry_at: (delay > 0).then_some(retry_at),
            result: result.clone(),
        });
        if next.logs.len() > LOG_LIMIT {
            next.logs.drain(..next.logs.len() - LOG_LIMIT);
        }
        // 失败也落盘；不保存票原文、代理认证或上游正文。
        let committed = self.commit(next, guard).await;
        let mut o = self
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        o.retry
            .entry(input.account_id.clone())
            .and_modify(|t| *t = (*t).max(retry_at))
            .or_insert(retry_at);
        if matches!(status, 429 | 401 | 403) {
            o.manual_retry
                .entry(input.account_id.clone())
                .and_modify(|t| *t = (*t).max(retry_at))
                .or_insert(retry_at);
        }
        o.last
            .insert((input.account_id, input.model), result.clone());
        committed.map_err(|_| {
            error(ProviderAdminErrorKind::Internal)
                .with_public_message("探测已执行，但日志或票保存失败，请检查磁盘状态")
        })?;
        Ok(result)
    }

    async fn send(
        &self,
        account: &ProviderAccount,
        authentication: &CodexRuntimeAuthentication,
        model: &str,
        proxy: &str,
    ) -> Result<(u16, String, u64, String), &'static str> {
        let auth = authentication.oauth().ok_or("账号没有 OAuth 凭据")?;
        let builder = reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .pool_max_idle_per_host(0)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(25))
            .proxy(reqwest::Proxy::all(proxy).map_err(|_| "代理配置无效")?);
        let client = crate::transport::tls::build_reqwest_client_with_custom_ca(builder)
            .map_err(|_| "请求客户端初始化失败")?;
        let profile = self.profile.snapshot();
        // 复用 RS 已核验画像；仅在 Astra 最低版本约束下提升版本，避免身份字段不一致。
        let mut version = profile.codex_version.clone();
        if (model.contains("astra") || model.contains("gpt-6"))
            && semver::Version::parse(&version)
                .map_or(true, |v| v < semver::Version::new(0, 153, 4))
        {
            version = "0.153.4".into();
        }
        let mut identity = profile;
        identity.codex_version = version.clone();
        let mut request = client.post(&self.url).bearer_auth(auth.access_token.expose_secret())
            .header("user-agent", identity.user_agent()).header("originator", identity.originator)
            .header("version", version).header("accept", "text/event-stream")
            .header("openai-beta", "responses=experimental").header("connection", "close")
            .header("session_id", uuid::Uuid::new_v4().to_string())
            .json(&json!({"model":model,"store":false,"stream":true,"instructions":"Reply with exactly: pong",
                "input":[{"role":"user","content":[{"type":"input_text","text":"ping"}]}]}));
        if let Some(id) = account.upstream_account_id() {
            request = request.header("chatgpt-account-id", id);
        }
        let response = request.send().await.map_err(|err| {
            let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&err);
            while let Some(cause) = source {
                let text = cause.to_string().to_ascii_lowercase();
                if text.contains("password")
                    || text.contains("authentication")
                    || text.contains("auth failure")
                {
                    return "代理认证失败，请检查用户名、密码和服务商授权";
                }
                if text.contains("connection refused") {
                    return "代理端口拒绝连接";
                }
                source = cause.source();
            }
            if err.is_timeout() {
                "请求超时"
            } else if err.is_connect() {
                "代理连接或 TLS 握手失败"
            } else {
                "网络请求失败"
            }
        })?;
        let status = response.status().as_u16();
        let state = response
            .headers()
            .get(HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim()
            .to_owned();
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map_or(0, |v| {
                v.parse::<u64>().unwrap_or_else(|_| {
                    chrono::DateTime::parse_from_rfc2822(v).map_or(0, |t| {
                        u64::try_from(
                            (t.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds(),
                        )
                        .unwrap_or(0)
                    })
                })
            });
        // 仅取响应头；不消费生成正文，不复用此连接。
        let message = if status == 429 {
            "上游限流，已退避"
        } else if status == 401 || status == 403 {
            "上游拒绝认证或访问，已退避"
        } else if status != 200 {
            "上游返回非 200 状态"
        } else if state.is_empty() {
            "响应未包含 Turn-State"
        } else {
            "未匹配目标规则"
        };
        Ok((status, state, retry, message.into()))
    }
}

fn proxy_endpoint(raw: &str) -> String {
    let Ok(url) = url::Url::parse(raw) else {
        return "未知代理".into();
    };
    // 只组合白名单字段，不在 URL 原文上做字符串脱敏。
    let host = url
        .host()
        .map_or_else(|| "未知主机".into(), |host| host.to_string());
    let port = url
        .port_or_known_default()
        .or_else(|| matches!(url.scheme(), "socks5" | "socks5h").then_some(1080));
    match port {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

struct BusyGuard<'a> {
    observations: &'a Mutex<Observations>,
    key: (String, String),
    proxy: String,
}
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        let mut observations = self
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = observations.busy.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                observations.busy.remove(&self.key);
            }
        }
        if let Some(count) = observations.proxy_busy.get_mut(&self.proxy) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                observations.proxy_busy.remove(&self.proxy);
            }
        }
    }
}

fn valid_proxy(raw: &str) -> bool {
    if raw.is_empty() {
        return true;
    }
    if raw.len() > 4096 {
        return false;
    }
    url::Url::parse(raw).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https" | "socks5" | "socks5h")
            && u.host_str().is_some()
            && u.query().is_none()
            && u.fragment().is_none()
            && matches!(u.path(), "" | "/")
    })
}

fn split_proxy_pool(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing parent"))?;
    std::fs::create_dir_all(parent)?;
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

pub(crate) struct TicketTask(pub(crate) Arc<TicketService>);
impl ScheduledTask for TicketTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            self.0
                .observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .worker_checked_at = Some(now());
            let panel = self
                .0
                .panel()
                .await
                .map_err(|_| WorkerTaskError::safe("打标目录读取失败"))?;
            if !panel.settings.enabled
                || !panel.settings.proxy_pool_enabled
                || !panel.proxy_configured
            {
                return Ok(());
            }
            let candidates: Vec<_> = panel
                .accounts
                .iter()
                .filter(|a| a.eligible && a.policy.mode == TicketMode::Auto)
                .flat_map(|a| {
                    a.models
                        .iter()
                        .filter(|m| {
                            !m.busy
                                && m.retry_at.is_none()
                                && m.expires_at.is_none_or(|t| {
                                    t <= now() + panel.settings.refresh_before_seconds
                                })
                        })
                        .map(|m| TicketProbe {
                            account_id: a.id.clone(),
                            model: m.model.clone(),
                        })
                })
                .collect();
            if candidates.is_empty() {
                return Ok(());
            }
            let input = {
                let o = self
                    .0
                    .observations
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                candidates
                    .into_iter()
                    .min_by_key(|p| {
                        o.last
                            .get(&(p.account_id.clone(), p.model.clone()))
                            .map_or(0, |r| r.checked_at)
                    })
                    .expect("候选非空")
            };
            // 同一账号模型按各入口配额并发尝试；达到全局上限后从轮换游标公平选择。
            let snapshot = self
                .0
                .data
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let start = self
                .0
                .observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .proxy_cursor;
            let mut slots = Vec::new();
            for offset in 0..snapshot.proxy_pool.len() {
                let index = (start + offset) % snapshot.proxy_pool.len();
                if snapshot.proxy_enabled.get(index).copied().unwrap_or(true) {
                    for _ in 0..snapshot.proxy_concurrency.get(index).copied().unwrap_or(1) {
                        if slots.len() < MAX_PROBE_CONCURRENCY {
                            slots.push(index);
                        }
                    }
                }
            }
            let probes = futures::future::join_all(slots.into_iter().map(|index| {
                self.0.probe_on_proxy(
                    TicketProbe {
                        account_id: input.account_id.clone(),
                        model: input.model.clone(),
                    },
                    true,
                    Some(index),
                )
            }));
            tokio::select! {
                () = context.cancellation().cancelled() => {},
                results = probes => {
                    for result in results {
                        match result {
                            Ok(result) => tracing::info!(account_id = %result.account_id, model = %result.model,
                                http = result.http_status, length = result.length, matched = result.matched, "后台打标完成"),
                            Err(error) => tracing::warn!(kind = ?error.kind(), "后台打标未执行"),
                        }
                    }
                }
            }
            Ok(())
        })
    }
}
