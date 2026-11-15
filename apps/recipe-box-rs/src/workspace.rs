//! ワークスペース。ログインの代わりに、推測できない ID でデータの持ち主を分ける。

use std::fmt;

use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{HeaderMap, StatusCode, request::Parts},
};
use serde::Serialize;

use crate::{
    backend::{Backend, PhotoKey, WorkspaceLimits},
    error::ApiError,
    id,
    limits::client_key,
    photo::object_key,
};

/// API がワークスペースの ID を受け取るヘッダー。
pub const WORKSPACE_ID_HEADER: &str = "x-workspace-id";

/// アプリ全体のワークスペースの数の上限。
///
/// 費用を守るための上限で、攻撃者が IP アドレスを変えても超えられない（D1 の行を数えて判定する）。
/// レシピの数の上限（[`crate::recipe::MAX_RECIPES_PER_WORKSPACE`]）と掛け合わせた量が、
/// D1 の保存量の上限に余裕を持って収まるようにする（docs/security.md の「費用の対策」）。
pub const MAX_WORKSPACES: i64 = 100;
/// アプリ全体で 1 日（UTC の日付）に作れるワークスペースの数。
///
/// 上限いっぱいまで一度に作られるのを遅らせる。
pub const MAX_WORKSPACES_PER_DAY: i64 = 20;
/// 1 日のミリ秒。
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// 最終利用日時を書き換える間隔（1 時間）。
///
/// 使うたびに書き換えると、読むだけの API でも毎回 DB に書き込むことになる。
/// 自動削除は 14 日使われていないかで決めるので、1 時間の粗さで足りる。
pub const LAST_USED_UPDATE_INTERVAL_MILLIS: i64 = 60 * 60 * 1000;

/// ワークスペースの ID。128 ビットの乱数を、16 進数の小文字 32 文字で表したもの。
///
/// 値を作れるのは [`WorkspaceId::from_bytes`] と [`WorkspaceId::parse`] だけなので、
/// この型の値は、いつも正しい形をしている。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// 16 バイトの乱数から ID を作る。
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(id::hex_from_bytes(bytes))
    }

    /// 文字列が ID の形をしていれば、ID にする。
    pub fn parse(value: &str) -> Option<Self> {
        id::is_valid(value).then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// リクエストが使うワークスペース。
///
/// ハンドラーの引数に書くと、`X-Workspace-Id` ヘッダーを調べ、
/// ワークスペースが在れば最終利用日時を更新したうえで、その ID を渡す。
/// 無い・形が正しくない・見つからないときは、ハンドラーを呼ばずにエラーを返す。
#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
}

impl<B: Backend> FromRequestParts<B> for Workspace {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, backend: &B) -> Result<Self, Self::Rejection> {
        let mut values = parts.headers.get_all(WORKSPACE_ID_HEADER).iter();
        let value = values.next().ok_or(ApiError::WorkspaceIdMissing)?;
        // 同じヘッダーが 2 つ以上あると、どちらのワークスペースか決められないので受け付けない。
        if values.next().is_some() {
            return Err(ApiError::WorkspaceIdInvalid);
        }
        // ASCII でない値（to_str が失敗する）・空・前後の空白も、ID の形でないので受け付けない。
        let id = value
            .to_str()
            .ok()
            .and_then(WorkspaceId::parse)
            .ok_or(ApiError::WorkspaceIdInvalid)?;
        if !touch(backend, &id).await? {
            return Err(ApiError::WorkspaceNotFound);
        }
        Ok(Self { id })
    }
}

/// ワークスペースが在るかを確かめ、在れば最終利用日時を更新する。無ければ `false` を返す。
///
/// 前に書き換えてから 1 時間以上たっていたときだけ書き換える（[`LAST_USED_UPDATE_INTERVAL_MILLIS`]）。
pub async fn touch<B: Backend>(backend: &B, id: &WorkspaceId) -> Result<bool, ApiError> {
    let Some(last_used_at) = backend.workspace_last_used_at(id).await? else {
        return Ok(false);
    };
    let now = backend.now_millis();
    if now - last_used_at >= LAST_USED_UPDATE_INTERVAL_MILLIS {
        backend.set_workspace_last_used_at(id, now).await?;
    }
    Ok(true)
}

/// `POST /api/workspaces` の応答の本文。
#[derive(Serialize)]
pub struct CreatedWorkspace {
    id: String,
}

/// `POST /api/workspaces`: ワークスペースを作り、ID を返す。
pub async fn create<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<CreatedWorkspace>), ApiError> {
    let id = create_new(&backend, &headers).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedWorkspace { id: id.to_string() }),
    ))
}

/// ワークスペースを作る。API と画面の両方から使う。
///
/// IP アドレスごとの回数を先に、アプリ全体の上限を後に確かめる。
/// 全体の上限を数える D1 の読み取りを、連打で増やさないため。
pub async fn create_new<B: Backend>(
    backend: &B,
    headers: &HeaderMap,
) -> Result<WorkspaceId, ApiError> {
    if !backend
        .allow_workspace_creation(&client_key(headers))
        .await?
    {
        return Err(ApiError::TooManyRequests);
    }
    let id = WorkspaceId::from_bytes(backend.random_128());
    let now = backend.now_millis();
    let limits = WorkspaceLimits {
        max_total: MAX_WORKSPACES,
        max_created_since: MAX_WORKSPACES_PER_DAY,
        // UTC の今日の 0 時。
        since: now - now.rem_euclid(DAY_MILLIS),
    };
    if !backend.insert_workspace(&id, now, &limits).await? {
        return Err(ApiError::CapacityReached);
    }
    Ok(id)
}

/// `DELETE /api/workspaces`: `X-Workspace-Id` のワークスペースを、レシピと写真ごと消し、`204` を返す。
pub async fn delete<B: Backend>(
    State(backend): State<B>,
    workspace: Workspace,
) -> Result<StatusCode, ApiError> {
    remove(&backend, &workspace.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// ワークスペースと、そのレシピ・写真を消す。API と画面の両方から使う。共有のデモは消さない。
///
/// レシピを消すとき（[`crate::recipe::remove`]）と同じ順に消す。
/// 1. 今ある写真のデータを R2 から消す。ここで失敗しても、D1 の行が残るので、もう一度消せる。
/// 2. D1 でワークスペース・レシピ・写真の行を 1 つのトランザクションで消し、消した写真を受け取る。
/// 3. 2 で消した写真のデータを、もう一度すべて R2 から消す。1 の後に追加された写真のデータを残さないため。
pub async fn remove<B: Backend>(backend: &B, workspace: &WorkspaceId) -> Result<(), ApiError> {
    if backend.demo_workspace_id().as_ref() == Some(workspace) {
        return Err(ApiError::DemoWorkspaceProtected);
    }
    let before = backend.list_workspace_photos(workspace).await?;
    remove_photo_objects(backend, workspace, &before).await?;
    let deleted = backend
        .delete_workspace(workspace)
        .await?
        .ok_or(ApiError::WorkspaceNotFound)?;
    remove_photo_objects(backend, workspace, &deleted).await
}

/// ワークスペースの写真のデータを R2 から消す。
///
/// R2 の 1 回の削除は 1,000 個までだが、1 つのワークスペースの写真は
/// レシピの上限 × 1 レシピの写真の上限（80 × 5 = 400 枚）までなので、1 回で足りる。
async fn remove_photo_objects<B: Backend>(
    backend: &B,
    workspace: &WorkspaceId,
    photos: &[PhotoKey],
) -> Result<(), ApiError> {
    if photos.is_empty() {
        return Ok(());
    }
    let keys = photos
        .iter()
        .map(|photo| object_key(workspace, &photo.recipe_id, &photo.id))
        .collect();
    backend.delete_objects(keys).await?;
    Ok(())
}
