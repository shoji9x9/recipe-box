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
fn router() -> Router {
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
