//! ワークスペース。ログインの代わりに、推測できない ID でデータの持ち主を分ける。

use std::fmt;

use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{HeaderMap, StatusCode, request::Parts},
};
use serde::Serialize;

use crate::{
    backend::{Backend, BackendError, PhotoKey, WorkspaceLimits},
    error::{ApiError, log_error, run_to_completion_or_internal},
    id,
    limits::{DAY_MILLIS, client_key, start_of_utc_day},
    photo::{self, PendingCleanup, object_key},
    recipe::RecipeId,
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

// guide:start workspace-extractor
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
// guide:end workspace-extractor

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
        since: start_of_utc_day(now),
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

/// ワークスペースと、そのレシピ・写真を消す。API と画面の両方から使う。
///
/// 共有のデモと、自動削除から外す印（`pinned`）の付いたワークスペースは消さない。
/// 印でも断るのは、`DEMO_WORKSPACE_ID` を外した後に、漏れたデモの ID で消されないようにするためである。
///
/// レシピを消すとき（[`crate::recipe::remove`]）と同じ順に消す（[`remove_with_photos`]）。
/// 写真を隠してからは、送り手との接続が切れても最後まで動かす（[`Backend::run_to_completion`]）。
pub async fn remove<B: Backend>(backend: &B, workspace: &WorkspaceId) -> Result<(), ApiError> {
    if backend.demo_workspace_id().as_ref() == Some(workspace) {
        return Err(ApiError::DemoWorkspaceProtected);
    }
    // 写真を隠す前に、断る理由を確かめる（隠すときと D1 の削除でも印を確かめるが、理由を分けて返すため）。
    let Some(status) = backend.workspace_status(workspace).await? else {
        return Err(ApiError::WorkspaceNotFound);
    };
    if status.pinned {
        return Err(ApiError::DemoWorkspaceProtected);
    }
    let task = remove_with_photos(backend.clone(), workspace.clone(), None);
    if run_to_completion_or_internal(backend, task, "ワークスペースを消す").await? {
        Ok(())
    } else {
        Err(ApiError::WorkspaceNotFound)
    }
}

/// ワークスペースを、写真のデータごと消す。消さなかったとき（無い・印がある・`last_used_before` より後に使われた）は `false` を返す。
///
/// 1. D1 で、写真を読み、印の無い写真の行に、削除が終わっていない印を付けて隠す（1 つのトランザクション）。
///    消さない条件のワークスペースなら、何も隠さない。
/// 2. 1 で読んだ写真のデータを R2 から消す。ここで失敗すると、写真は隠れたまま残り、自動削除が片付ける（[`photo::clean_pending`]）。
///    ワークスペースは写真の無い状態で残り、もう一度消せる。
/// 3. D1 でワークスペース・レシピ・写真の行を 1 つのトランザクションで消し、消した写真を受け取る。
///    ここで失敗しても、写真は隠れたまま残るので、データの無い写真は見えない（自動削除が片付ける）。
/// 4. 3 で消した写真のデータを、もう一度すべて R2 から消す。1 の後に追加された写真のデータを残さないため。
async fn remove_with_photos<B: Backend>(
    backend: B,
    workspace: WorkspaceId,
    last_used_before: Option<i64>,
) -> Result<bool, ApiError> {
    let before = backend
        .hide_workspace_photos(&workspace, backend.now_millis(), last_used_before)
        .await?;
    remove_photo_objects(&backend, &workspace, &before).await?;
    let Some(deleted) = backend
        .delete_workspace(&workspace, last_used_before)
        .await?
    else {
        return Ok(false);
    };
    remove_photo_objects(&backend, &workspace, &deleted).await?;
    Ok(true)
}

/// 最後に使われてから、自動で消すまでの期間（14 日）。
pub const UNUSED_DELETE_AFTER_MILLIS: i64 = 14 * DAY_MILLIS;

/// 定期実行の間隔（1 時間）。wrangler.toml の `[triggers]` の `0 * * * *` に合わせる
/// （テストの `cron_runs_as_often_as_the_cleanup_budget_assumes` で確かめる）。
pub const CLEANUP_INTERVAL_MILLIS: i64 = 60 * 60 * 1000;
/// 定期実行が 1 日に動く回数。
pub const CLEANUP_RUNS_PER_DAY: i64 = DAY_MILLIS / CLEANUP_INTERVAL_MILLIS;
/// 定期実行の 1 回で消すワークスペースの数の上限。残りは次の実行に回す（選び方は [`pick_for_run`]）。
pub const MAX_WORKSPACES_PER_RUN: i64 = 8;
// アプリ全体のワークスペースが、すべて同じ日に対象になっても、その日のうちに消し終える（残りがたまり続けない）。
// 対象になるのは最終利用日時で決まるので、1 日に作れる数ではなく、全体の数で確かめる。
const _: () = assert!(MAX_WORKSPACES_PER_RUN * CLEANUP_RUNS_PER_DAY >= MAX_WORKSPACES);

/// 定期実行の 1 回で処理するものを、並べた `items` から `count` 個まで選び、選んだものと、次の実行に回した数を返す。
///
/// 選び始める位置を、実行ごと（[`CLEANUP_INTERVAL_MILLIS`] ごと）に `count` 個ずつずらし、先頭に戻って続ける。
/// 失敗し続けるもの（次の実行でも残る）が先頭にあっても、ほかのものが毎回後に回されないようにするためである。
pub(crate) fn pick_for_run<T>(mut items: Vec<T>, now: i64, count: usize) -> (Vec<T>, usize) {
    let total = items.len();
    if total <= count {
        return (items, 0);
    }
    let run = u64::try_from(now.div_euclid(CLEANUP_INTERVAL_MILLIS)).unwrap_or(0);
    let step = u64::try_from(count).unwrap_or(0);
    let len = u64::try_from(total).unwrap_or(1);
    let start = usize::try_from(run.wrapping_mul(step) % len).unwrap_or(0);
    items.rotate_left(start);
    items.truncate(count);
    (items, total - count)
}

/// 定期実行の 1 回の実行で呼べる、D1 のクエリと R2 の呼び出しの数の上限（無料のプラン）。
///
/// D1 は 1 回の実行のクエリを 50 までにする（batch は中の文の数によらず 1 回に数える）。
/// Workers は、D1 と R2 への呼び出しも含めた subrequest を 50 までにする。どちらにも収まるよう、両方を合わせて数える。
/// 超えると、それより後の呼び出しがすべて失敗する。
pub const MAX_SUBREQUESTS_PER_RUN: i64 = 50;
/// 1 つのワークスペースを消すのに使う呼び出しの数の最大（[`remove_with_photos`]）。
/// 写真を隠す D1 の 1 回・R2 の削除・D1 の削除の 1 回・R2 の削除（写真が無ければ R2 は呼ばない）。
/// 1 つのワークスペースの写真は、R2 の 1 回の削除で消せる（[`photo::MAX_KEYS_PER_DELETE`]）。
const SUBREQUESTS_PER_WORKSPACE: i64 = 4;
/// 自動削除（[`delete_unused`]）の 1 回の実行の呼び出しの数の最大。
/// 古い記録の削除の 1 回・書き込み中の写真の片付け・対象を選ぶ 1 回・ワークスペースごとの削除。
pub const CLEANUP_SUBREQUESTS: i64 =
    1 + photo::PENDING_CLEANUP_SUBREQUESTS + 1 + MAX_WORKSPACES_PER_RUN * SUBREQUESTS_PER_WORKSPACE;
const _: () = assert!(CLEANUP_SUBREQUESTS <= MAX_SUBREQUESTS_PER_RUN);

/// 自動削除の結果。ログに出す。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanupSummary {
    /// 消したワークスペースの数。
    pub deleted: usize,
    /// 対象に選んだが、消さなかった数（共有のデモ・選んだ後に使われた・印が付いた）。
    pub skipped: usize,
    /// 消すのに失敗した数（理由はログに出す）。
    pub failed: usize,
    /// 1 回に消す数の上限（[`MAX_WORKSPACES_PER_RUN`]）を超えたので、次の実行に回した数。
    pub deferred: usize,
    /// 対象のワークスペースを選べなかったか。
    pub list_failed: bool,
    /// 1 日の数を数えるための古い記録を消せなかったか。
    pub records_failed: bool,
    /// 書き込みか削除の途中で残った写真の行を片付けた結果（[`photo::clean_pending`]）。
    pub pending_photos: PendingCleanup,
}

impl CleanupSummary {
    /// どこかで失敗したか。
    pub const fn has_failure(&self) -> bool {
        self.failed > 0
            || self.list_failed
            || self.records_failed
            || self.pending_photos.failed > 0
            || self.pending_photos.list_failed
    }
}

/// 定期実行（Cron Trigger）で呼ぶ。14 日使われていないワークスペースを、レシピと写真ごと消す。
///
/// 1 回の実行で呼べる D1 と R2 の数に上限がある（[`MAX_SUBREQUESTS_PER_RUN`]）ので、1 回に処理する数を絞り、
/// 数の決まった片付けを先に動かす。
/// 1. 1 日の数を数えるための記録（写真の追加・ワークスペースの作成）のうち、前の日までのものを消す。
/// 2. 書き込みか削除の途中で失敗して残った写真の行を、R2 のデータと一緒に片付ける（[`photo::clean_pending`]）。
/// 3. 使われていないワークスペースを、最終利用日時の古い順に並べ、[`MAX_WORKSPACES_PER_RUN`] 個まで消す（[`pick_for_run`]）。
///    残りは次の実行に回す。
///
/// 1 つのワークスペースで失敗しても、ほかのワークスペースは消す。
/// 失敗の理由はログに出す。ワークスペースの ID は持ち主の証なので、ログに出す前に伏せる（[`crate::id::redact`]）。
pub async fn delete_unused<B: Backend>(backend: &B) -> CleanupSummary {
    let now = backend.now_millis();
    let before = now - UNUSED_DELETE_AFTER_MILLIS;
    let demo = backend.demo_workspace_id();
    let mut summary = CleanupSummary::default();
    // UTC の今日の 0 時より前の記録は、1 日の数に入らない。
    if let Err(error) = backend.delete_old_records(start_of_utc_day(now)).await {
        log_error(&error);
        summary.records_failed = true;
    }
    summary.pending_photos = photo::clean_pending(backend, now).await;
    let rows = match backend.list_unused_workspaces(before).await {
        Ok(rows) => rows,
        Err(error) => {
            log_error(&error);
            summary.list_failed = true;
            Vec::new()
        }
    };
    let mut targets = Vec::new();
    for row in rows {
        // 形の正しくない ID の行（アプリは作らない）は消せないので、失敗に数えて知らせ、ほかは消す。
        let Some(id) = WorkspaceId::parse(&row.id) else {
            // 行を探す手がかりに、値を添える（ID の形の部分は log_error が伏せる）。
            log_error(&BackendError(format!(
                "使われていないワークスペースの ID の形が正しくない: {:?}",
                row.id
            )));
            summary.failed += 1;
            continue;
        };
        // 共有のデモは、印を付け忘れていても消さない。
        if demo.as_ref() == Some(&id) {
            summary.skipped += 1;
            continue;
        }
        targets.push((row.last_used_at, id));
    }
    // 長く使われていないものから並べる。同じ日時なら ID の順にして、毎回同じ順にする。
    targets.sort_by(|a, b| (a.0, a.1.as_str()).cmp(&(b.0, b.1.as_str())));
    // 8 は usize に収まる。
    let per_run = usize::try_from(MAX_WORKSPACES_PER_RUN).unwrap_or(1);
    let (targets, deferred) = pick_for_run(targets, now, per_run);
    summary.deferred = deferred;
    for (_, id) in targets {
        // 選んだ後に使われたワークスペースは、写真を隠すときと D1 で消すときに、同じ条件で確かめ直して消さない。
        match remove_with_photos(backend.clone(), id, Some(before)).await {
            Ok(true) => summary.deleted += 1,
            Ok(false) => summary.skipped += 1,
            // 失敗の理由は、失敗したところでログに出している。
            Err(_) => summary.failed += 1,
        }
    }
    summary
}

/// ワークスペースの写真のデータを R2 から消す。
async fn remove_photo_objects<B: Backend>(
    backend: &B,
    workspace: &WorkspaceId,
    photos: &[PhotoKey],
) -> Result<(), ApiError> {
    let keys = photos
        .iter()
        .map(|photo| {
            // D1 の行はアプリが作った ID だけを持つが、R2 のキーの形を型で保つため、ここでも確かめる。
            let recipe = RecipeId::parse(&photo.recipe_id).ok_or_else(|| {
                // BackendError からの変換ではないので、ここでログに出す。
                log_error(&BackendError(
                    "写真の行のレシピの ID の形が正しくない".to_owned(),
                ));
                ApiError::Internal
            })?;
            Ok(object_key(workspace, &recipe, &photo.id))
        })
        .collect::<Result<_, ApiError>>()?;
    photo::delete_objects(backend, keys).await
}
