//! Cloudflare の機能（D1・Rate Limiting・Web Crypto）を使う [`Backend`] の実装。
//!
//! JavaScript の値は `Send` でないが、Workers は 1 つのスレッドで動くので、
//! workers-rs の `SendWrapper`・`SendFuture` で包んで axum に渡しても問題が無い。

use std::future::Future;

use wasm_bindgen::prelude::wasm_bindgen;
use worker::{
    Env,
    send::{SendFuture, SendWrapper},
    wasm_bindgen::JsValue,
};

use crate::{
    backend::{Backend, BackendError},
    workspace::WorkspaceId,
};

/// `wrangler.toml` の D1 の binding の名前。
const DB_BINDING: &str = "DB";
/// `wrangler.toml` のレート制限の binding の名前。
const WORKSPACE_CREATION_LIMITER: &str = "WORKSPACE_CREATION_LIMITER";

#[wasm_bindgen]
extern "C" {
    /// Web Crypto の `crypto.getRandomValues()`。暗号に使える質の乱数で `buf` を埋める。
    #[wasm_bindgen(js_namespace = crypto, js_name = getRandomValues)]
    fn get_random_values(buf: &mut [u8]);
}

/// Workers の環境（binding の入れ物）を持つ [`Backend`]。
#[derive(Clone)]
pub struct CloudflareBackend {
    env: SendWrapper<Env>,
}

impl CloudflareBackend {
    pub fn new(env: Env) -> Self {
        Self {
            env: SendWrapper::new(env),
        }
    }
}

/// workers-rs のエラーを [`BackendError`] にする。どの操作で失敗したかを添える。
fn backend_error(action: &str, error: &worker::Error) -> BackendError {
    BackendError(format!("{action}: {error}"))
}

/// ミリ秒の時刻を JavaScript の数値にする。
/// JavaScript の数値は 2^53 までの整数を正確に表せるので、ミリ秒の時刻なら誤差は出ない。
#[allow(clippy::cast_precision_loss)]
fn millis_to_js(millis: i64) -> JsValue {
    JsValue::from_f64(millis as f64)
}

impl Backend for CloudflareBackend {
    fn random_128(&self) -> [u8; 16] {
        let mut bytes = [0; 16];
        get_random_values(&mut bytes);
        bytes
    }

    fn now_millis(&self) -> i64 {
        // as_millis は u64 を返す。i64 に収まらないのは約 2 億 9 千万年後なので、収まらなければ最大値にする。
        i64::try_from(worker::Date::now().as_millis()).unwrap_or(i64::MAX)
    }

    fn allow_workspace_creation(
        &self,
        client_key: &str,
    ) -> impl Future<Output = Result<bool, BackendError>> + Send {
        let env = self.env.clone();
        let key = client_key.to_owned();
        SendFuture::new(async move {
            let limiter = env
                .rate_limiter(WORKSPACE_CREATION_LIMITER)
                .map_err(|e| backend_error("レート制限の binding を取れない", &e))?;
            let outcome = limiter
                .limit(key)
                .await
                .map_err(|e| backend_error("レート制限を確かめられない", &e))?;
            Ok(outcome.success)
        })
    }

    fn insert_workspace(
        &self,
        id: &WorkspaceId,
        now: i64,
    ) -> impl Future<Output = Result<(), BackendError>> + Send {
        let env = self.env.clone();
        let id = id.to_string();
        SendFuture::new(async move {
            let db = env
                .d1(DB_BINDING)
                .map_err(|e| backend_error("D1 の binding を取れない", &e))?;
            db.prepare("INSERT INTO workspaces (id, created_at, last_used_at) VALUES (?, ?, ?)")
                .bind(&[id.into(), millis_to_js(now), millis_to_js(now)])
                .map_err(|e| backend_error("ワークスペースを作る SQL を組めない", &e))?
                .run()
                .await
                .map_err(|e| backend_error("ワークスペースを保存できない", &e))?;
            Ok(())
        })
    }

    fn touch_workspace(
        &self,
        id: &WorkspaceId,
        now: i64,
    ) -> impl Future<Output = Result<bool, BackendError>> + Send {
        let env = self.env.clone();
        let id = id.to_string();
        SendFuture::new(async move {
            let db = env
                .d1(DB_BINDING)
                .map_err(|e| backend_error("D1 の binding を取れない", &e))?;
            let result = db
                .prepare("UPDATE workspaces SET last_used_at = ? WHERE id = ?")
                .bind(&[millis_to_js(now), id.into()])
                .map_err(|e| backend_error("ワークスペースを更新する SQL を組めない", &e))?
                .run()
                .await
                .map_err(|e| backend_error("ワークスペースを更新できない", &e))?;
            // 更新した行の数が 1 なら、ワークスペースが在る。
            let changes = result
                .meta()
                .map_err(|e| backend_error("更新の結果を読めない", &e))?
                .and_then(|meta| meta.changes)
                .unwrap_or(0);
            Ok(changes > 0)
        })
    }
}
