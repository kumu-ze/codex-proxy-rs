//! Provider-owned 受限账号探测；不接受任意目标 URL、认证头或请求正文。
use crate::{
    credential::{CodexCredentialRepository, CodexRuntimeAuthentication},
    transport::{CODEX_RESPONSES_PATH, endpoint_url, profile::CodexWireProfileState},
};
use async_trait::async_trait;
use gateway_core::{
    account::{CredentialState, ProviderAccount, ProviderAccountId},
    engine::extensions::{ExtensionServices, ExtensionUnavailable},
};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
const HEADER: &str = "x-codex-turn-state";
pub(crate) struct OpenAiExtensionServices {
    repository: CodexCredentialRepository,
    profile: CodexWireProfileState,
    url: String,
}
impl OpenAiExtensionServices {
    pub(crate) fn new(
        repository: CodexCredentialRepository,
        profile: CodexWireProfileState,
        base: &str,
    ) -> Arc<Self> {
        Arc::new(Self {
            repository,
            profile,
            url: endpoint_url(base, CODEX_RESPONSES_PATH),
        })
    }
    async fn account_view(&self, account: &ProviderAccount) -> Value {
        let eligible = account.enabled()
            && account.authentication_kind() == "oauth"
            && account.credential_state() == CredentialState::Ready;
        let binding = if eligible {
            self.repository
                .load_runtime_credential(account)
                .await
                .ok()
                .map(|c| c.authentication.extension_scope(account))
        } else {
            None
        };
        json!({"id":account.id().as_str(),"name":account.name(),"plan":account.plan_type(),"eligible":eligible&&binding.is_some(),"binding":binding,"revision":account.revision().get(),"authenticationKind":account.authentication_kind()})
    }
    async fn send(
        &self,
        account: &ProviderAccount,
        authentication: &CodexRuntimeAuthentication,
        model: &str,
        proxy: &str,
    ) -> Result<(u16, String, u64, String), &'static str> {
        let auth = authentication.oauth().ok_or("账号没有 OAuth 凭据")?;
        let client = probe_client(proxy, Duration::from_secs(25))?;
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
    async fn measure_exit(&self, proxy: &str) -> Result<std::net::IpAddr, &'static str> {
        let mut url = url::Url::parse(&self.url).map_err(|_| "检测地址无效")?;
        url.set_path("/cdn-cgi/trace");
        url.set_query(None);
        url.set_fragment(None);
        // 与打标目标使用同一域名，避免域名分流；不附带账号凭据，也不跟随重定向。
        let client = probe_client(proxy, Duration::from_secs(5))?;
        let mut response = client
            .get(url)
            .header("user-agent", self.profile.snapshot().user_agent())
            .send()
            .await
            .map_err(|_| "出口检测连接失败或超时")?;
        if !response.status().is_success() {
            return Err("出口检测服务返回错误状态");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "出口检测响应读取失败")?
        {
            if body.len() + chunk.len() > 1024 {
                return Err("出口检测响应过大");
            }
            body.extend_from_slice(&chunk);
        }
        std::str::from_utf8(&body)
            .ok()
            .and_then(|s| {
                s.lines().find_map(|line| {
                    line.strip_prefix("ip=")
                        .and_then(|ip| ip.trim().parse().ok())
                })
            })
            .ok_or("出口检测未返回有效 IP")
    }
}
#[async_trait]
impl ExtensionServices for OpenAiExtensionServices {
    async fn invoke(&self, method: &str, input: Value) -> Result<Value, ExtensionUnavailable> {
        match method {
            "accounts.list" => {
                let accounts = self
                    .repository
                    .list_for_provider()
                    .await
                    .map_err(|_| ExtensionUnavailable)?;
                let mut output = Vec::new();
                for account in accounts {
                    output.push(self.account_view(&account).await)
                }
                Ok(Value::Array(output))
            }
            "accounts.get" => {
                let id = ProviderAccountId::new(
                    input["accountId"].as_str().ok_or(ExtensionUnavailable)?,
                )
                .map_err(|_| ExtensionUnavailable)?;
                let account = self
                    .repository
                    .store()
                    .get_account(&id)
                    .await
                    .map_err(|_| ExtensionUnavailable)?;
                match account {
                    Some(account) if account.provider().as_str() == "openai" => {
                        Ok(self.account_view(&account).await)
                    }
                    _ => Ok(Value::Null),
                }
            }
            "responses.probe" => {
                let id = ProviderAccountId::new(
                    input["accountId"].as_str().ok_or(ExtensionUnavailable)?,
                )
                .map_err(|_| ExtensionUnavailable)?;
                let model = input["model"]
                    .as_str()
                    .filter(|m| {
                        !m.is_empty()
                            && m.len() <= 128
                            && m.bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                    })
                    .ok_or(ExtensionUnavailable)?;
                let proxy = input["proxy"]
                    .as_str()
                    .filter(|p| valid_proxy(p))
                    .ok_or(ExtensionUnavailable)?;
                let account = self
                    .repository
                    .store()
                    .get_account(&id)
                    .await
                    .map_err(|_| ExtensionUnavailable)?
                    .filter(|a| {
                        a.provider().as_str() == "openai"
                            && a.enabled()
                            && a.authentication_kind() == "oauth"
                            && a.credential_state() == CredentialState::Ready
                    })
                    .ok_or(ExtensionUnavailable)?;
                let authentication = self
                    .repository
                    .load_runtime_credential(&account)
                    .await
                    .map_err(|_| ExtensionUnavailable)?
                    .authentication;
                if input["credentialScope"].as_str()
                    != Some(authentication.extension_scope(&account).as_str())
                {
                    return Err(ExtensionUnavailable);
                }
                let result = self
                    .send(&account, &authentication, model, proxy)
                    .await
                    .map_err(|_| ExtensionUnavailable)?;
                serde_json::to_value(result).map_err(|_| ExtensionUnavailable)
            }
            "network.exit" => {
                let proxy = input["proxy"]
                    .as_str()
                    .filter(|p| valid_proxy(p))
                    .ok_or(ExtensionUnavailable)?;
                serde_json::to_value(
                    self.measure_exit(proxy)
                        .await
                        .map_err(|_| ExtensionUnavailable)?,
                )
                .map_err(|_| ExtensionUnavailable)
            }
            _ => Err(ExtensionUnavailable),
        }
    }
}
fn valid_proxy(raw: &str) -> bool {
    raw.len() <= 4096
        && url::Url::parse(raw).is_ok_and(|u| {
            matches!(u.scheme(), "http" | "https" | "socks5" | "socks5h")
                && u.host_str().is_some()
                && u.query().is_none()
                && u.fragment().is_none()
                && matches!(u.path(), "" | "/")
        })
}
fn probe_client(proxy: &str, timeout: Duration) -> Result<reqwest::Client, &'static str> {
    let builder = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .timeout(timeout)
        .proxy(reqwest::Proxy::all(proxy).map_err(|_| "代理配置无效")?);
    crate::transport::tls::build_reqwest_client_with_custom_ca(builder)
        .map_err(|_| "请求客户端初始化失败")
}
