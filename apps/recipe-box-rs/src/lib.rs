//! recipe-box の Worker。
//!
//! Cloudflare Workers から届いたリクエストを axum の `Router` に渡して処理する。

mod backend;
mod cloudflare;
mod error;
mod id;
mod recipe;
mod workspace;

use askama::Template;
use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    http::{StatusCode, Uri, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use tower_service::Service;
use worker::{Context, Env, HttpRequest, event};

use crate::{backend::Backend, cloudflare::CloudflareBackend, error::ApiError};

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
///
/// `backend` は、ハンドラーが DB などを使うときの窓口。本番では Cloudflare の機能を、テストでは偽物を渡す。
pub fn router<B: Backend>(backend: B) -> Router {
    // API は、受け付けないメソッドにも、ほかの失敗と同じ JSON の形で答える。
    let api = Router::new()
        .route("/health", get(health))
        .route("/workspaces", post(workspace::create::<B>))
        .route("/recipes", get(recipe::list::<B>).post(recipe::create::<B>))
        .route(
            "/recipes/{id}",
            get(recipe::get::<B>)
                .put(recipe::update::<B>)
                .delete(recipe::delete::<B>),
        )
        // 本文の大きさの上限。超えると、本文を読む前に 413 になる。
        .layer(DefaultBodyLimit::max(recipe::MAX_BODY_BYTES))
        .method_not_allowed_fallback(api_method_not_allowed);
    Router::new()
        .route("/", get(index))
        .nest("/api", api)
        // 無いパスは、ここでまとめて扱う。`nest` した router の fallback では
        // `/api/`（末尾のスラッシュ）に届かないので、パスを見て分ける。
        .fallback(not_found)
        .with_state(backend)
}

/// 無いパスへの応答。API（`/api` とその下）には JSON の形で、画面には本文の無い 404 で答える。
async fn not_found(uri: Uri) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") {
        ApiError::NotFound.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn api_method_not_allowed() -> ApiError {
    ApiError::MethodNotAllowed
}

/// Workers がリクエストを受け取るたびに呼ぶ入口。
#[event(fetch)]
async fn fetch(
    req: HttpRequest,
    env: Env,
    _ctx: Context,
) -> worker::Result<axum::http::Response<axum::body::Body>> {
    // panic したときに、原因をログに出す。
    console_error_panic_hook::set_once();
    Ok(router(CloudflareBackend::new(env)).call(req).await?)
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
mod tests;
