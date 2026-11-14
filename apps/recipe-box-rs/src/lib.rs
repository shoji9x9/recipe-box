//! recipe-box の Worker。
//!
//! Cloudflare Workers から届いたリクエストを axum の `Router` に渡して処理する。

mod backend;
mod cloudflare;
mod error;
mod workspace;

use askama::Template;
use axum::{
    Json, Router,
    http::{StatusCode, header},
    response::{Html, IntoResponse},
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
    // API は、無いパスや受け付けないメソッドにも、ほかの失敗と同じ JSON の形で答える。
    let api = Router::new()
        .route("/health", get(health))
        .route("/workspaces", post(workspace::create::<B>))
        .fallback(api_not_found)
        .method_not_allowed_fallback(api_method_not_allowed);
    Router::new()
        .route("/", get(index))
        .nest("/api", api)
        .with_state(backend)
}

async fn api_not_found() -> ApiError {
    ApiError::NotFound
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
mod tests {
    use std::{
        collections::HashMap,
        future::{Future, ready},
        sync::{
            Arc, Mutex,
            atomic::{AtomicU8, Ordering},
        },
    };

    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
        routing::get,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::router;
    use crate::{
        backend::{Backend, BackendError},
        workspace::{Workspace, WorkspaceId},
    };

    /// メモリの中で動く、テスト用の [`Backend`]。
    #[derive(Clone, Default)]
    struct FakeBackend {
        /// ワークスペースの ID と最終利用日時。
        workspaces: Arc<Mutex<HashMap<String, i64>>>,
        /// 次に返す乱数の 1 バイト目。呼ぶたびに 1 増やし、毎回ちがう ID になるようにする。
        next_random: Arc<AtomicU8>,
        /// `true` なら、ワークスペースを作る回数の制限を超えたことにする。
        rate_limited: bool,
        /// `true` なら、DB が失敗したことにする。
        broken: bool,
        /// レート制限に渡されたキー。
        rate_limit_keys: Arc<Mutex<Vec<String>>>,
    }

    /// テストの中の「今」の時刻。
    const NOW: i64 = 1_800_000_000_000;

    /// 偽物は待つ必要が無いので、`ready` ですぐに終わる `Future` を返す。
    impl Backend for FakeBackend {
        fn random_128(&self) -> [u8; 16] {
            let mut bytes = [0xab; 16];
            bytes[0] = self.next_random.fetch_add(1, Ordering::Relaxed);
            bytes
        }

        fn now_millis(&self) -> i64 {
            NOW
        }

        fn allow_workspace_creation(
            &self,
            client_key: &str,
        ) -> impl Future<Output = Result<bool, BackendError>> + Send {
            self.rate_limit_keys
                .lock()
                .unwrap()
                .push(client_key.to_owned());
            ready(Ok(!self.rate_limited))
        }

        fn insert_workspace(
            &self,
            id: &WorkspaceId,
            now: i64,
        ) -> impl Future<Output = Result<(), BackendError>> + Send {
            if self.broken {
                return ready(Err(BackendError("壊れた DB".into())));
            }
            self.workspaces.lock().unwrap().insert(id.to_string(), now);
            ready(Ok(()))
        }

        fn touch_workspace(
            &self,
            id: &WorkspaceId,
            now: i64,
        ) -> impl Future<Output = Result<bool, BackendError>> + Send {
            if self.broken {
                return ready(Err(BackendError("壊れた DB".into())));
            }
            let found = self
                .workspaces
                .lock()
                .unwrap()
                .get_mut(id.as_str())
                .map(|last_used_at| *last_used_at = now)
                .is_some();
            ready(Ok(found))
        }
    }

    /// 応答の状態コード・Content-Type・本文。
    struct TestResponse {
        status: StatusCode,
        content_type: String,
        /// Allow ヘッダー。無ければ空。
        allow: String,
        body: String,
    }

    impl TestResponse {
        fn json(&self) -> serde_json::Value {
            serde_json::from_str(&self.body).unwrap()
        }
    }

    /// `request` を `app` に送り、応答を返す。
    async fn send(app: Router, request: Request<Body>) -> TestResponse {
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        // 404 などでは Content-Type が付かないので、無ければ空にする。
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|value| value.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let allow = response
            .headers()
            .get(header::ALLOW)
            .map(|value| value.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        TestResponse {
            status,
            content_type,
            allow,
            body: String::from_utf8(bytes.to_vec()).unwrap(),
        }
    }

    async fn get_path(path: &str) -> TestResponse {
        let request = Request::get(path).body(Body::empty()).unwrap();
        send(router(FakeBackend::default()), request).await
    }

    fn create_workspace_request() -> Request<Body> {
        Request::post("/api/workspaces")
            .header("cf-connecting-ip", "192.0.2.1")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn index_returns_html() {
        let response = get_path("/").await;
        assert_eq!(response.status, StatusCode::OK);
        assert!(response.content_type.starts_with("text/html"));
        assert!(response.body.contains("<h1>recipe-box</h1>"));
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let response = get_path("/api/health").await;
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
        let response = get_path("/api/no-such-api").await;
        assert_eq!(response.status, StatusCode::NOT_FOUND);
        assert_eq!(response.content_type, "application/json");
        assert_eq!(response.json()["error"], "not_found");
    }

    #[tokio::test]
    async fn unknown_page_returns_not_found() {
        let response = get_path("/no-such-page").await;
        assert_eq!(response.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn create_workspace_returns_new_id() {
        let backend = FakeBackend::default();
        let first = send(router(backend.clone()), create_workspace_request()).await;
        let second = send(router(backend.clone()), create_workspace_request()).await;

        assert_eq!(first.status, StatusCode::CREATED);
        assert_eq!(first.content_type, "application/json");
        let id = first.json()["id"].as_str().unwrap().to_owned();
        assert!(WorkspaceId::parse(&id).is_some(), "{id}");
        assert_ne!(second.json()["id"].as_str().unwrap(), id);
        // 作ったワークスペースが保存され、最終利用日時が今になっている。
        assert_eq!(backend.workspaces.lock().unwrap().get(&id), Some(&NOW));
        // 接続元の IP アドレスが、レート制限のキーとして渡っている。
        assert_eq!(
            *backend.rate_limit_keys.lock().unwrap(),
            ["192.0.2.1", "192.0.2.1"]
        );
    }

    #[tokio::test]
    async fn create_workspace_rejects_when_rate_limited() {
        let backend = FakeBackend {
            rate_limited: true,
            ..FakeBackend::default()
        };
        let response = send(router(backend.clone()), create_workspace_request()).await;
        assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.json()["error"], "too_many_requests");
        assert!(backend.workspaces.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_workspace_hides_backend_errors() {
        let backend = FakeBackend {
            broken: true,
            ..FakeBackend::default()
        };
        let response = send(router(backend), create_workspace_request()).await;
        assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response.json()["error"], "internal_error");
        assert!(!response.body.contains("壊れた DB"));
    }

    /// [`Workspace`] を引数に取るハンドラーを持つ、テスト用の router。
    /// ワークスペースを使う API はまだ無いので、ここで ID の受け取り方を確かめる。
    fn workspace_echo_router(backend: FakeBackend) -> Router {
        Router::new()
            .route(
                "/echo",
                get(|workspace: Workspace| async move { workspace.id.to_string() }),
            )
            .with_state(backend)
    }

    fn echo_request(workspace_id: Option<&str>) -> Request<Body> {
        let mut builder = Request::get("/echo");
        if let Some(id) = workspace_id {
            builder = builder.header("X-Workspace-Id", id);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn workspace_extractor_accepts_existing_id_and_updates_last_used_at() {
        let id = "0123456789abcdef0123456789abcdef";
        let backend = FakeBackend::default();
        backend.workspaces.lock().unwrap().insert(id.into(), 0);

        let response = send(
            workspace_echo_router(backend.clone()),
            echo_request(Some(id)),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, id);
        assert_eq!(backend.workspaces.lock().unwrap().get(id), Some(&NOW));
    }

    #[tokio::test]
    async fn workspace_extractor_hides_backend_errors() {
        let backend = FakeBackend {
            broken: true,
            ..FakeBackend::default()
        };
        let response = send(
            workspace_echo_router(backend),
            echo_request(Some("0123456789abcdef0123456789abcdef")),
        )
        .await;
        assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response.json()["error"], "internal_error");
        assert!(!response.body.contains("壊れた DB"));
    }

    #[tokio::test]
    async fn workspace_extractor_rejects_bad_requests() {
        let cases = [
            (None, StatusCode::BAD_REQUEST, "workspace_id_missing"),
            (
                Some("not-an-id"),
                StatusCode::BAD_REQUEST,
                "workspace_id_invalid",
            ),
            (
                Some("0123456789ABCDEF0123456789ABCDEF"),
                StatusCode::BAD_REQUEST,
                "workspace_id_invalid",
            ),
            (
                Some("ffffffffffffffffffffffffffffffff"),
                StatusCode::NOT_FOUND,
                "workspace_not_found",
            ),
        ];
        for (id, status, error) in cases {
            let response = send(
                workspace_echo_router(FakeBackend::default()),
                echo_request(id),
            )
            .await;
            assert_eq!(response.status, status, "{id:?}");
            assert_eq!(response.content_type, "application/json", "{id:?}");
            assert_eq!(response.json()["error"], error, "{id:?}");
        }
    }
}
