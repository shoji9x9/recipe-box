//! router を通した API のテスト。D1 などの代わりに [`fake::FakeBackend`] を使う。

mod cleanup;
mod fake;
mod photos;
mod recipes;
mod routing;
mod screens;
mod thumbnails;
mod workspaces;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

use self::fake::FakeBackend;
use crate::router;

/// 応答の状態コード・ヘッダーの一部・本文。
pub struct TestResponse {
    pub status: StatusCode,
    pub content_type: String,
    /// Allow ヘッダー。無ければ空。
    pub allow: String,
    /// Location ヘッダー。無ければ空。
    pub location: String,
    /// すべてのヘッダー。
    pub headers: axum::http::HeaderMap,
    pub body: String,
    /// 本文のバイト列（写真など、UTF-8 でない本文を確かめるときに使う）。
    pub bytes: Vec<u8>,
}

impl TestResponse {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

/// `request` を `app` に送り、応答を返す。
pub async fn send(app: Router, request: Request<Body>) -> TestResponse {
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    // 無いヘッダーは空にする（404 などでは Content-Type が付かない）。
    let header_text = |name| {
        response
            .headers()
            .get(name)
            .map(|value| value.to_str().unwrap().to_owned())
            .unwrap_or_default()
    };
    let content_type = header_text(header::CONTENT_TYPE);
    let allow = header_text(header::ALLOW);
    let location = header_text(header::LOCATION);
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    TestResponse {
        status,
        content_type,
        allow,
        location,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
        bytes: bytes.to_vec(),
    }
}

/// `backend` を使う router に、`GET <path>` を本文なしで送る。
pub async fn get(backend: &FakeBackend, path: &str) -> TestResponse {
    let request = Request::get(path).body(Body::empty()).unwrap();
    send(router(backend.clone()), request).await
}

/// API へのリクエストを作る。`workspace` があれば `X-Workspace-Id` に、`body` があれば JSON の本文にする。
pub fn api_request(
    method: Method,
    path: &str,
    workspace: Option<&str>,
    body: Option<&serde_json::Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(id) = workspace {
        builder = builder.header("x-workspace-id", id);
    }
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

/// `backend` を使う router に、API へのリクエストを送る。
pub async fn call(
    backend: &FakeBackend,
    method: Method,
    path: &str,
    workspace: Option<&str>,
    body: Option<&serde_json::Value>,
) -> TestResponse {
    send(
        router(backend.clone()),
        api_request(method, path, workspace, body),
    )
    .await
}

/// テストでよく使うワークスペースの ID。
pub const WORKSPACE_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const WORKSPACE_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
