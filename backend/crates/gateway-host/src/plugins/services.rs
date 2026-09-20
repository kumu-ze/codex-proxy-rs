use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use gateway_core::engine::extensions::ExtensionServices;
use serde::Deserialize;
use serde_json::Value;
use std::sync::{Arc, OnceLock};

#[derive(Clone)]
struct ServiceState {
    token: String,
    provider: Arc<OnceLock<Arc<dyn ExtensionServices>>>,
    slots: Arc<tokio::sync::Semaphore>,
}

#[derive(Deserialize)]
struct Invocation {
    method: String,
    input: Value,
}

async fn invoke(
    State(state): State<ServiceState>,
    headers: HeaderMap,
    Json(body): Json<Invocation>,
) -> Result<Json<Value>, StatusCode> {
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(state.token.as_str()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let _slot = state
        .slots
        .try_acquire()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let provider = state
        .provider
        .get()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        provider.invoke(&body.method, body.input),
    )
    .await
    .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
    .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(output))
}

/// 仅回环监听；每个受信插件独享随机凭证及并发预算。凭证不进入配置和日志。
pub(super) struct ServiceEndpoint {
    pub url: String,
    pub token: String,
    task: tokio::task::JoinHandle<()>,
}
impl ServiceEndpoint {
    pub async fn start(
        provider: Arc<OnceLock<Arc<dyn ExtensionServices>>>,
    ) -> Result<Self, super::PluginError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| super::PluginError::Io)?;
        let url = format!(
            "http://{}/invoke",
            listener.local_addr().map_err(|_| super::PluginError::Io)?
        );
        let token = format!("Bearer {}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let router = Router::new()
            .route("/invoke", post(invoke))
            .with_state(ServiceState {
                token: token.clone(),
                provider,
                slots: Arc::new(tokio::sync::Semaphore::new(16)),
            })
            .layer(axum::extract::DefaultBodyLimit::max(65536));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self { url, token, task })
    }
}
impl Drop for ServiceEndpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) struct ProviderServices {
    pub provider: Arc<dyn ExtensionServices>,
    pub proxies: Arc<dyn gateway_admin::ports::proxy::ProxyStore>,
}
#[async_trait::async_trait]
impl ExtensionServices for ProviderServices {
    async fn invoke(
        &self,
        method: &str,
        input: Value,
    ) -> Result<Value, gateway_core::engine::extensions::ExtensionUnavailable> {
        use gateway_core::engine::extensions::ExtensionUnavailable;
        match method {
            "proxies.resolve" => {
                let id = input["id"].as_str().ok_or(ExtensionUnavailable)?;
                let proxy = self
                    .proxies
                    .get(id)
                    .await
                    .map_err(|_| ExtensionUnavailable)?;
                Ok(Value::String(proxy.proxy.expose_url().into()))
            }
            "proxies.list" => {
                let page = input["page"].as_u64().unwrap_or(1).clamp(1, 100000) as u32;
                let result = self
                    .proxies
                    .list(gateway_admin::model::proxies::ProxyListQuery {
                        page,
                        page_size: gateway_admin::model::PageSize::new(100)
                            .map_err(|_| ExtensionUnavailable)?,
                        search: String::new(),
                    })
                    .await
                    .map_err(|_| ExtensionUnavailable)?;
                Ok(
                    serde_json::json!({"items":result.items.iter().map(|p|serde_json::json!({"id":p.id,"name":p.name,"endpoint":p.proxy.endpoint(),"hasAuthentication":p.proxy.expose_url().contains('@')})).collect::<Vec<_>>(),"page":{"totalPages":result.total.div_ceil(100)}}),
                )
            }
            _ => self.provider.invoke(method, input).await,
        }
    }
}
