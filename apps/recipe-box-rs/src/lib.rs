//! recipe-box の Worker。
//!
//! Cloudflare Workers から届いたリクエストを axum の `Router` に渡して処理する。

mod backend;
mod cloudflare;
mod error;
mod id;
mod limits;
mod photo;
mod recipe;
mod screen;
mod workspace;

use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    http::{StatusCode, Uri},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Serialize;
use tower_service::Service;
use worker::{Context, Env, HttpRequest, ScheduleContext, ScheduledEvent, event};

use crate::{backend::Backend, cloudflare::CloudflareBackend, error::ApiError};

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
        .route(
            "/workspaces",
            // ワークスペースを作る回数は、専用のレート制限で確かめる（[`workspace::create_new`]）。
            // 消すのは、ほかの書き込む API と同じ回数の制限を通す。
            post(workspace::create::<B>).merge(delete(workspace::delete::<B>).route_layer(
                middleware::from_fn_with_state(backend.clone(), limits::limit_writes::<B>),
            )),
        )
        .merge(recipe_routes(backend.clone()))
        // 本文の大きさの上限。超えると、本文を読む前に 413 になる。
        .layer(DefaultBodyLimit::max(recipe::MAX_BODY_BYTES))
        // 写真の API は本文の上限が違うので、上の上限の外に置く。
        .merge(photo_routes(backend.clone()))
        .method_not_allowed_fallback(api_method_not_allowed);
    Router::new()
        .merge(screen_routes(backend.clone()))
        // 写真のフォームは本文の上限が違うので、上の画面の上限の外に置く。
        .merge(screen_photo_routes(backend.clone()))
        .nest("/api", api)
        // 無いパスは、ここでまとめて扱う。`nest` した router の fallback では
        // `/api/`（末尾のスラッシュ）に届かないので、パスを見て分ける。
        .fallback(not_found)
        .with_state(backend)
}

/// レシピの API。書き込む API の前に、回数と本文の大きさを確かめる（[`limits::limit_writes`]）。
///
/// `route_layer` にするので、無いパス（404）には効かない。
fn recipe_routes<B: Backend>(backend: B) -> Router<B> {
    Router::new()
        .route("/recipes", get(recipe::list::<B>).post(recipe::create::<B>))
        .route(
            "/recipes/{id}",
            get(recipe::get::<B>)
                .put(recipe::update::<B>)
                .delete(recipe::delete::<B>),
        )
        .route_layer(middleware::from_fn_with_state(
            backend,
            limits::limit_writes::<B>,
        ))
}

/// 写真の API。書き込む API の前に、回数と本文の大きさ（1 枚の写真の上限）を確かめる（[`limits::limit_photo_writes`]）。
fn photo_routes<B: Backend>(backend: B) -> Router<B> {
    Router::new()
        .route(
            "/recipes/{id}/photos",
            get(photo::list::<B>).post(photo::create::<B>),
        )
        .route(
            "/recipes/{id}/photos/{photo_id}",
            get(photo::get::<B>).delete(photo::delete::<B>),
        )
        .route_layer(middleware::from_fn_with_state(
            backend,
            limits::limit_photo_writes::<B>,
        ))
        .layer(DefaultBodyLimit::max(photo::MAX_PHOTO_BYTES))
}

/// 画面。次の順に middleware を通る（[`screen`]）。
///
/// 1. [`screen::security_headers`] — すべての応答に、セキュリティのためのヘッダーを付ける。
/// 2. [`screen::guard_writes`] — 書き込み（POST）の送り元・回数・本文の大きさを確かめる。D1 を読む前に断るため、先に置く。
/// 3. [`screen::identify`] — Cookie から、画面を見ている人のワークスペースを決める。
///
/// 本文の大きさは API と同じ上限にする。`Content-Length` の無い本文（chunked）も、ここで止める。
/// CSS・htmx・アイコンは静的アセット（`public/`）として Worker の外で配るので、ここには無い。
fn screen_routes<B: Backend>(backend: B) -> Router<B> {
    let check_routes = screen_check_routes();
    Router::new()
        .route("/", get(screen::index::<B>))
        .route("/recipes", post(screen::create::<B>))
        .route("/recipes/new", get(screen::new_form::<B>))
        .route(
            "/recipes/{id}",
            get(screen::detail::<B>).post(screen::update::<B>),
        )
        .route("/recipes/{id}/edit", get(screen::edit_form::<B>))
        .route("/recipes/{id}/delete", post(screen::delete::<B>))
        .route(
            "/workspace",
            get(screen::workspace_page::<B>).post(screen::create_workspace::<B>),
        )
        .route("/workspace/switch", post(screen::switch_workspace::<B>))
        .route("/workspace/delete", post(screen::delete_workspace::<B>))
        .route("/workspace/demo", post(screen::view_demo::<B>))
        .route("/workspace/own", post(screen::view_own))
        .route_layer(middleware::from_fn_with_state(
            backend.clone(),
            screen::identify::<B>,
        ))
        .route_layer(middleware::from_fn_with_state(
            backend,
            screen::guard_writes::<B>,
        ))
        .route_layer(middleware::from_fn(screen::security_headers))
        .merge(check_routes)
        .layer(DefaultBodyLimit::max(recipe::MAX_BODY_BYTES))
}

/// 画面の写真。middleware は [`screen_routes`] と同じ順に通るが、書き込みの本文の上限が違う
/// （[`screen::guard_photo_writes`]）。
fn screen_photo_routes<B: Backend>(backend: B) -> Router<B> {
    Router::new()
        .route("/recipes/{id}/photos", post(screen::add_photo::<B>))
        .route(
            "/recipes/{id}/photos/{photo_id}",
            get(screen::photo_file::<B>),
        )
        .route(
            "/recipes/{id}/photos/{photo_id}/delete",
            post(screen::delete_photo::<B>),
        )
        .route_layer(middleware::from_fn_with_state(
            backend.clone(),
            screen::identify::<B>,
        ))
        .route_layer(middleware::from_fn_with_state(
            backend,
            screen::guard_photo_writes::<B>,
        ))
        .route_layer(middleware::from_fn(screen::security_headers))
        .layer(DefaultBodyLimit::max(screen::PHOTO_FORM_MAX_BYTES))
}

/// 画面のフォームを確かめるだけのルート。ワークスペースを読まないので、[`screen::identify`] を通さない。
/// 送り元と本文の大きさは確かめるが、書き込みの回数の制限は使わない（[`screen::guard_check`]）。
fn screen_check_routes<B: Backend>() -> Router<B> {
    Router::new()
        .route("/recipes/check", post(screen::check_form))
        .route_layer(middleware::from_fn(screen::guard_check))
        .route_layer(middleware::from_fn(screen::security_headers))
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

/// 定期実行（wrangler.toml の Cron Trigger）の入口。使われていないワークスペースを消す（[`workspace::delete_unused`]）。
#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    console_error_panic_hook::set_once();
    let summary = workspace::delete_unused(&CloudflareBackend::new(env)).await;
    // 失敗の理由は、失敗したところでログに出している（error::log_error）。
    if summary.has_failure() {
        worker::console_error!("使われていないワークスペースの自動削除: {summary:?}");
    } else {
        worker::console_log!("使われていないワークスペースの自動削除: {summary:?}");
    }
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

#[cfg(test)]
mod tests;
