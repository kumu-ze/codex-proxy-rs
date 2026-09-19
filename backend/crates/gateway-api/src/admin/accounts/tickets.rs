//! 独立打标页面，复用管理会话和统一错误合同。

use super::*;
use crate::auth::SessionState;
use gateway_admin::model::tickets::{
    TicketContinuousInput, TicketExitProbe, TicketProbe, TicketUpdate,
};

pub(super) fn router<S>() -> Router<S>
where
    S: SessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/tickets", get(panel::<S>).post(update::<S>))
        .route("/api/admin/tickets/probe", post(probe::<S>))
        .route("/api/admin/tickets/continuous", post(continuous::<S>))
        .route("/api/admin/tickets/logs/clear", post(clear_logs::<S>))
        .route("/api/admin/tickets/proxy-exit", post(exit_sample::<S>))
}

async fn panel<S>(_auth: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .accounts()
        .ticket_panel()
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}

async fn clear_logs<S>(
    auth: AdminAuth,
    State(state): State<S>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .accounts()
        .clear_ticket_logs(&auth.context().mutation_context())
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}

async fn exit_sample<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminJson(input): AdminJson<TicketExitProbe>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .accounts()
        .ticket_exit_sample(input)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}

async fn continuous<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(input): AdminJson<TicketContinuousInput>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .accounts()
        .continuous_ticket(&auth.context().mutation_context(), input)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}

async fn update<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(input): AdminJson<TicketUpdate>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .accounts()
        .update_tickets(&auth.context().mutation_context(), input)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}

async fn probe<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(input): AdminJson<TicketProbe>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let data = state
        .admin_services()
        .accounts()
        .probe_ticket(&auth.context().mutation_context(), input)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}
