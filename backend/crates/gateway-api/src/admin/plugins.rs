//! 固定的插件管理入口；插件不能注册或覆盖宿主路由。

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use gateway_admin::ports::plugins::{PluginManagement, PluginOperationError};
use serde::Deserialize;
use serde_json::Value;

use super::{AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminResponse};
use crate::auth::SessionState;

pub(super) fn router<S: SessionState + Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/admin/plugins", get(list::<S>))
        .route("/api/admin/plugins/invoke", post(invoke::<S>))
        .route("/api/admin/plugins/manage", post(manage::<S>))
}

async fn list<S: SessionState + Send + Sync>(
    _auth: AdminAuth,
    State(state): State<S>,
) -> impl IntoResponse {
    let services = state.admin_services();
    let data = match services.plugins() {
        Some(plugins) => plugins.list().await,
        None => Vec::new(),
    };
    AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invocation {
    id: String,
    method: String,
    input: Value,
}

async fn invoke<S: SessionState + Send + Sync>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<Invocation>,
) -> Result<impl IntoResponse, AdminError> {
    let services = state.admin_services();
    let plugins = services
        .plugins()
        .ok_or_else(AdminError::service_unavailable)?;
    let data = plugins
        .invoke(&body.id, &body.method, body.input)
        .await
        .map_err(operation_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}

fn operation_error(error: PluginOperationError) -> AdminError {
    match error {
        PluginOperationError::NotFound => AdminError::not_found("插件不存在"),
        PluginOperationError::Invalid => AdminError::bad_request("插件操作无效"),
        PluginOperationError::Rejected => AdminError::bad_request("插件拒绝了本次操作"),
        PluginOperationError::Unavailable => AdminError::service_unavailable(),
        PluginOperationError::Package => {
            AdminError::bad_request("插件包格式或内容校验失败：请提供兼容 API 1 的 tar.gz 插件包")
        }
        PluginOperationError::Download => {
            AdminError::bad_request("下载连接失败：请检查网络或选择可用的下载代理")
        }
        PluginOperationError::DownloadTimeout => {
            AdminError::bad_request("下载连接超时：请重试或选择下载代理")
        }
        PluginOperationError::DownloadHttp(status) => AdminError::bad_request(format!(
            "下载服务器返回 HTTP {status}，请检查直链或下载代理"
        )),
        PluginOperationError::Checksum => {
            AdminError::bad_request("SHA256 不匹配：请核对发布者提供的整包校验值")
        }
        PluginOperationError::Proxy => {
            AdminError::bad_request("下载代理不存在或不可用，请重新选择")
        }
        PluginOperationError::Conflict => {
            AdminError::bad_request("该插件已安装，或插件数量已达到 16 个上限")
        }
        PluginOperationError::Storage => {
            AdminError::bad_request("插件状态保存失败，请检查运行目录权限和磁盘空间")
        }
    }
}

async fn manage<S: SessionState + Send + Sync>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminJson(body): AdminJson<PluginManagement>,
) -> Result<impl IntoResponse, AdminError> {
    let services = state.admin_services();
    let plugins = services
        .plugins()
        .ok_or_else(AdminError::service_unavailable)?;
    plugins.manage(body).await.map_err(operation_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(plugins.list().await),
    ))
}
