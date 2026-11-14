//! recipe-box の Worker。
//!
//! Cloudflare Workers から届いたリクエストを axum の `Router` に渡して処理する。

use askama::Template;
use axum::{
    Json, Router,
    http::{StatusCode, header},
    response::{Html, IntoResponse},
    routing::get,
};
use serde::Serialize;
use tower_service::Service;
use worker::{Context, Env, HttpRequest, event};

/// トップページのテンプレート（`templates/index.html`）。
#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate;

/// `GET /api/health` の応答の本文。
#[derive(Serialize)]
struct Health {
    status: &'static str,
}

/// URL のパスと処理する関数の対応を作る。
pub fn router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/health", get(health))
}

/// Workers がリクエストを受け取るたびに呼ぶ入口。
#[event(fetch)]
async fn fetch(
    req: HttpRequest,
    _env: Env,
    _ctx: Context,
) -> worker::Result<axum::http::Response<axum::body::Body>> {
    // panic したときに、原因をログに出す。
    console_error_panic_hook::set_once();
    Ok(router().call(req).await?)
}

async fn index() -> impl IntoResponse {
    match IndexTemplate.render() {
        Ok(html) => Html(html).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "画面を表示できませんでした",
        )
            .into_response(),
    }
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

#[cfg(test)]
mod tests {
    use super::router;
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// `GET <path>` を router に送り、状態コード・Content-Type・本文を返す。
    async fn get(path: &str) -> (StatusCode, String, String) {
        let request = Request::get(path).body(Body::empty()).unwrap();
        let response = router().oneshot(request).await.unwrap();
        let status = response.status();
        // 404 などでは Content-Type が付かないので、無ければ空にする。
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|value| value.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            content_type,
            String::from_utf8(bytes.to_vec()).unwrap(),
        )
    }

    #[tokio::test]
    async fn index_returns_html() {
        let (status, content_type, body) = get("/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type.starts_with("text/html"));
        assert!(body.contains("<h1>recipe-box</h1>"));
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (status, content_type, body) = get("/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type, "application/json");
        assert_eq!(body, r#"{"status":"ok"}"#);
    }

    #[tokio::test]
    async fn health_rejects_post() {
        let request = Request::post("/api/health").body(Body::empty()).unwrap();
        let response = router().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn unknown_path_returns_not_found() {
        let (status, _, _) = get("/no-such-page").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
