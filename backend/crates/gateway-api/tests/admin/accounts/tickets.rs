use super::super::{AdminTestFixture, AdminTestState};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use gateway_api::admin;
use tower::ServiceExt as _;

#[tokio::test]
async fn ticket_endpoints_require_admin_session() {
    let fixture = AdminTestFixture::new().await;
    for (uri, method, body) in [
        ("/api/admin/tickets", "GET", ""),
        ("/api/admin/tickets", "POST", "{}"),
        (
            "/api/admin/tickets/continuous",
            "POST",
            "{\"accountId\":\"acct_test\",\"model\":\"gpt-6-astra\",\"intervalSeconds\":10}",
        ),
        (
            "/api/admin/tickets/probe",
            "POST",
            "{\"accountId\":\"acct_test\",\"model\":\"gpt-6-astra\"}",
        ),
    ] {
        let response = admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
