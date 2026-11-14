//! API が失敗を返すときの形。
//!
//! 本文は `{"error":"<機械向けのコード>","message":"<人向けの説明>"}` にそろえる。
//! リライトの前後で同じ形を保つので、コードと状態コードの組み合わせを変えるときは慎重に。

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::backend::BackendError;

/// API の失敗。
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "ワークスペースを使う API（Issue #4 のレシピの API）で使う"
    )
)]
#[derive(Debug, PartialEq, Eq)]
pub enum ApiError {
    /// `X-Workspace-Id` ヘッダーが無い。
    WorkspaceIdMissing,
    /// `X-Workspace-Id` ヘッダーの形が正しくない。
    WorkspaceIdInvalid,
    /// その ID のワークスペースが無い（消された場合も含む）。
    WorkspaceNotFound,
    /// 短い間に呼ばれすぎた。
    TooManyRequests,
    /// サーバーの中の失敗。詳しいことはログにだけ出す。
    Internal,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    message: &'static str,
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &'static str, &'static str) {
        match self {
            Self::WorkspaceIdMissing => (
                StatusCode::BAD_REQUEST,
                "workspace_id_missing",
                "X-Workspace-Id ヘッダーがありません",
            ),
            Self::WorkspaceIdInvalid => (
                StatusCode::BAD_REQUEST,
                "workspace_id_invalid",
                "X-Workspace-Id ヘッダーの形が正しくありません",
            ),
            Self::WorkspaceNotFound => (
                StatusCode::NOT_FOUND,
                "workspace_not_found",
                "ワークスペースが見つかりません",
            ),
            Self::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                "しばらく待ってから、もう一度試してください",
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "サーバーで問題が起きました",
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error, message) = self.parts();
        (status, Json(ErrorBody { error, message })).into_response()
    }
}

/// 外の世界での失敗は、ログに出したうえで 500 にする。`?` で変換できるようにしておく。
impl From<BackendError> for ApiError {
    fn from(error: BackendError) -> Self {
        log_error(&error);
        Self::Internal
    }
}

/// エラーをログに出す。Workers では `console.error` に、テストでは標準エラー出力に出す。
fn log_error(error: &BackendError) {
    #[cfg(target_arch = "wasm32")]
    worker::console_error!("{error}");
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{error}");
}
