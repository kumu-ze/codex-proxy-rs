use super::{AdminTestFixture, AdminTestState};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use gateway_admin::ports::plugins::{
    PluginManagement, PluginOperationError, PluginOperations, PluginStatus,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt as _;

struct Echo;
#[async_trait]
impl PluginOperations for Echo {
    async fn manage(&self, _: PluginManagement) -> Result<(), PluginOperationError> {
        Ok(())
    }
    async fn list(&self) -> Vec<PluginStatus> {
        vec![]
    }
    async fn invoke(&self, _: &str, _: &str, input: Value) -> Result<Value, PluginOperationError> {
        Ok(input)
    }
}

#[tokio::test]
async fn plugin_routes_require_admin_and_preserve_authenticated_rpc() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let state = AdminTestState(fixture.services.with_plugins(Arc::new(Echo)));
    let app = gateway_api::admin::router().with_state(state);
    for authorized in [false, true] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/admin/plugins/invoke")
            .header("x-request-id", "plugin-test")
            .header(header::CONTENT_TYPE, "application/json");
        if authorized {
            request = request.header(header::COOKIE, "cpr_session=valid-session");
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(
                        json!({"id":"example","method":"admin.echo","input":{"value":42}})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if authorized {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/plugins")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn management_requires_admin_and_accepts_typed_actions() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let app = gateway_api::admin::router().with_state(AdminTestState(
        fixture.services.with_plugins(Arc::new(Echo)),
    ));
    for (authorized, body, expected) in [
        (
            false,
            json!({"action":"disable","id":"example"}),
            StatusCode::UNAUTHORIZED,
        ),
        (
            true,
            json!({"action":"disable","id":"example"}),
            StatusCode::OK,
        ),
        (
            true,
            json!({"action":"install","url":"https://example.com/package.tar.gz"}),
            StatusCode::OK,
        ),
        (
            true,
            json!({"action":"execute","id":"example"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            true,
            json!({"action":"disable","id":"example","unexpected":true}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/admin/plugins/manage")
            .header("x-request-id", "plugin-manage")
            .header(header::CONTENT_TYPE, "application/json");
        if authorized {
            request = request.header(header::COOKIE, "cpr_session=valid-session");
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}
