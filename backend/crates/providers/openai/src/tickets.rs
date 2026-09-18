//! Provider-owned 打标：后台探测、私有持久化及按账号/模型隔离的注入。

use crate::{
    credential::CodexCredentialRepository,
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
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

const HEADER: &str = "x-codex-turn-state";

#[derive(Clone, Serialize, Deserialize)]
struct Ticket {
    account_id: String,
    credential_revision: u64,
    model: String,
    state: String,
    expires_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Persisted {
    revision: u64,
    settings: TicketSettings,
    proxy_url: String,
    tickets: Vec<Ticket>,
}

impl Default for Persisted {
    fn default() -> Self {
        Self {
            revision: 1,
            settings: TicketSettings::default(),
            proxy_url: String::new(),
            tickets: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Observations {
    last: BTreeMap<(String, String), TicketResult>,
    retry: BTreeMap<String, u64>,
    busy: Option<(String, String)>,
}

pub(crate) struct TicketService {
    repository: CodexCredentialRepository,
    profile: CodexWireProfileState,
    url: String,
    path: PathBuf,
    data: Mutex<Persisted>,
    observations: Mutex<Observations>,
    persistence: AsyncMutex<()>,
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
        if !data.settings.validate() || !valid_proxy(&data.proxy_url) {
            return Err(error(ProviderAdminErrorKind::Invalid));
        }
        data.tickets.retain(|t| {
            t.expires_at > now()
                && t.expires_at <= now() + 3600
                && valid_state(&t.state, t.state.len())
                && t.state.len() <= 4096
        });
        Ok(Self {
            repository,
            profile,
            url: endpoint_url(base_url, CODEX_RESPONSES_PATH),
            path,
            data: Mutex::new(data),
            observations: Mutex::new(Observations::default()),
            persistence: AsyncMutex::new(()),
            request_slot: Semaphore::new(1),
        })
    }

    // 写成功后才发布内存状态；锁覆盖整个替换，防止旧快照覆盖新配置。
    async fn commit(&self, next: Persisted) -> Result<(), ProviderAdminError> {
        let bytes =
            serde_json::to_vec(&next).map_err(|_| error(ProviderAdminErrorKind::Internal))?;
        // 此小型本地状态文件同步原子替换，替换与内存发布之间不留取消点。
        // 避免脱离请求生命周期的后台写盘在新策略之后覆盖旧文件。
        atomic_write(&self.path, &bytes).map_err(|_| error(ProviderAdminErrorKind::Internal))?;
        *self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
        Ok(())
    }

    pub(crate) async fn panel(&self) -> Result<TicketPanel, ProviderAdminError> {
        let accounts = self
            .repository
            .list_for_provider()
            .await
            .map_err(|_| error(ProviderAdminErrorKind::Unavailable))?;
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
                                    && t.credential_revision == a.revision().get()
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
                                busy: o.busy.as_ref() == Some(&key),
                                retry_at: o.retry.get(id).copied().filter(|t| *t > now()),
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
            proxy_configured: !d.proxy_url.is_empty(),
            accounts: rows,
        })
    }

    pub(crate) async fn update(
        &self,
        update: TicketUpdate,
    ) -> Result<TicketPanel, ProviderAdminError> {
        if !update.settings.validate() || update.proxy_url.as_ref().is_some_and(|p| !valid_proxy(p))
        {
            return Err(error(ProviderAdminErrorKind::Invalid));
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
            return Err(error(ProviderAdminErrorKind::Invalid));
        }
        let guard = self.persistence.lock().await;
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
        next.settings = update.settings;
        if let Some(proxy) = update.proxy_url {
            next.proxy_url = proxy.trim().into();
        }
        if next.settings.enabled && next.proxy_url.is_empty() {
            return Err(error(ProviderAdminErrorKind::Invalid));
        }
        // 策略改变后旧票全部失效，进行中的旧策略探测也不能重新写回。
        next.tickets.clear();
        self.commit(next).await?;
        drop(guard);
        self.panel().await
    }

    pub(crate) fn get(&self, account: &ProviderAccount, model: &str) -> Option<String> {
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
                    && t.credential_revision == account.revision().get()
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
            || snapshot.proxy_url.is_empty()
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
        {
            let mut o = self
                .observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if o.retry.get(&input.account_id).is_some_and(|t| *t > now()) {
                return Err(error(ProviderAdminErrorKind::Conflict));
            }
            o.busy = Some((input.account_id.clone(), input.model.clone()));
        }
        let _busy = BusyGuard(&self.observations);
        let target = snapshot
            .settings
            .target_length(&input.account_id, account.plan_type());
        let outcome = self.send(&account, &input.model, &snapshot.proxy_url).await;
        let (status, state, retry, message) =
            outcome.unwrap_or_else(|_| (0, String::new(), 60, "请求失败或超时".into()));
        let current = self
            .repository
            .store()
            .get_account(&account_id)
            .await
            .map_err(|_| error(ProviderAdminErrorKind::Unavailable))?;
        let account_unchanged = current.as_ref().is_some_and(|a| {
            eligible(a)
                && a.revision() == account.revision()
                && a.upstream_account_id() == account.upstream_account_id()
                && a.plan_type() == account.plan_type()
        });
        let mut matched = status == 200 && valid_state(&state, target) && account_unchanged;
        let guard = self.persistence.lock().await;
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
                "策略或账号已改变，结果未采纳".into()
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
                model: input.model.clone(),
                state,
                expires_at: now() + snapshot.settings.ttl_seconds,
            });
            self.commit(next).await?;
        }
        drop(guard);
        let mut o = self
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let delay = match status {
            429 => retry.max(60),
            401 | 403 => 900,
            0 => 60,
            _ => snapshot.settings.interval_seconds,
        };
        o.retry
            .insert(input.account_id.clone(), now().saturating_add(delay));
        o.last
            .insert((input.account_id, input.model), result.clone());
        Ok(result)
    }

    async fn send(
        &self,
        account: &ProviderAccount,
        model: &str,
        proxy: &str,
    ) -> Result<(u16, String, u64, String), ()> {
        let credential = self
            .repository
            .load_runtime_credential(account)
            .await
            .map_err(|_| ())?;
        let auth = credential.authentication.oauth().ok_or(())?;
        let builder = reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .pool_max_idle_per_host(0)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(25))
            .proxy(reqwest::Proxy::all(proxy).map_err(|_| ())?);
        let client =
            crate::transport::tls::build_reqwest_client_with_custom_ca(builder).map_err(|_| ())?;
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
        let response = request.send().await.map_err(|_| ())?;
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
        Ok((
            status,
            state,
            retry,
            if status == 429 {
                "上游限流，已退避"
            } else {
                "未匹配目标规则"
            }
            .into(),
        ))
    }
}

struct BusyGuard<'a>(&'a Mutex<Observations>);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .busy = None;
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
            let panel = self
                .0
                .panel()
                .await
                .map_err(|_| WorkerTaskError::safe("打标目录读取失败"))?;
            if !panel.settings.enabled || !panel.proxy_configured {
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
            // 每周期最多一发，与手动任务共享并发槽；取消直接丢弃请求 future。
            tokio::select! {
                () = context.cancellation().cancelled() => {},
                result = self.0.probe_with_mode(input, true) => {
                    if let Ok(result) = result {
                        tracing::info!(account_id = %result.account_id, model = %result.model,
                            http = result.http_status, length = result.length, matched = result.matched, "后台打标完成");
                    }
                }
            }
            Ok(())
        })
    }
}
