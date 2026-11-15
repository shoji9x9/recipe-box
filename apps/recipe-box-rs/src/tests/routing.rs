//! 画面・ヘルスチェック・無いパスとメソッドのテスト。

use axum::{
    body::Body,
    http::{Request, StatusCode},
};

use super::{fake::FakeBackend, get, send};
use crate::router;

#[tokio::test]
async fn health_returns_ok() {
    let response = get(&FakeBackend::default(), "/api/health").await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.content_type, "application/json");
    assert_eq!(response.body, r#"{"status":"ok"}"#);
}

#[tokio::test]
async fn health_rejects_post() {
    let request = Request::post("/api/health").body(Body::empty()).unwrap();
    let response = send(router(FakeBackend::default()), request).await;
    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.content_type, "application/json");
    assert_eq!(response.json()["error"], "method_not_allowed");
    // 受け付けるメソッドを、Allow ヘッダーで知らせる。
    assert!(response.allow.contains("GET"), "{}", response.allow);
}

#[tokio::test]
async fn unknown_api_path_returns_json_not_found() {
    let response = get(&FakeBackend::default(), "/api/no-such-api").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.content_type, "application/json");
    assert_eq!(response.json()["error"], "not_found");
}

#[tokio::test]
async fn api_root_and_trailing_slash_return_json_not_found() {
    for path in ["/api", "/api/", "/api/workspaces/", "/api/recipes/"] {
        let response = get(&FakeBackend::default(), path).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(response.content_type, "application/json", "{path}");
        assert_eq!(response.json()["error"], "not_found", "{path}");
    }
}

#[tokio::test]
async fn workspaces_rejects_get() {
    let response = get(&FakeBackend::default(), "/api/workspaces").await;
    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.json()["error"], "method_not_allowed");
    assert!(response.allow.contains("POST"), "{}", response.allow);
}

#[tokio::test]
async fn unknown_page_returns_not_found() {
    let response = get(&FakeBackend::default(), "/no-such-page").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
}
