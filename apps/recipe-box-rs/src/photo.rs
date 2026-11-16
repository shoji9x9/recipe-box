//! レシピの写真の API（`/api/recipes/{id}/photos`）。
//!
//! 写真のデータは R2 に、上限の判定に使う大きさなどは D1 の `photos` に置く。
//! 費用を守るため、R2 に書き込む前に D1 で上限を判定する（docs/security.md の「費用の対策」）。
//!
//! 写真はバイト列をそのまま本文で受け取る（multipart にしない）。形式は `Content-Type` ではなく、
//! 中身の先頭のバイトで決める。宣言と中身が違うファイルを、別の形式として配らないため。

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{FromRequest, Path, Request, State, rejection::PathRejection},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::{
    backend::{Backend, BackendError, PhotoInsert, PhotoLimits},
    error::{ApiError, log_error},
    id,
    recipe::RecipeId,
    stream::{self, PhotoStream, RawPhotoBody},
    workspace::{MAX_WORKSPACES, Workspace, WorkspaceId},
};

/// 1 枚の写真の大きさの上限（バイト）。超えたら `413`。
pub const MAX_PHOTO_BYTES: usize = 4_000_000;
/// 1 つのレシピの写真の数の上限。超えたら `409`（`photo_limit_reached`）。
pub const MAX_PHOTOS_PER_RECIPE: i64 = 5;
/// 1 つのワークスペースの写真の大きさの合計の上限（バイト）。超えたら `409`（`photo_storage_limit_reached`）。
///
/// アプリ全体の写真の合計は、この上限とワークスペースの数の上限（[`MAX_WORKSPACES`]）で守る
/// （[`MAX_PHOTO_BYTES_TOTAL`] を超えない大きさにする）。
/// 追加のたびにアプリ全体の合計を数えると、写真の表の全体を読むことになるため、直接は数えない。
pub const MAX_PHOTO_BYTES_PER_WORKSPACE: i64 = 50_000_000;
/// アプリ全体の写真の大きさの合計の上限（バイト）。費用を守るための上限で、R2 の保存量の無料枠（10 GB-月）の半分にする。
///
/// 追加のときには数えない。ワークスペースの数とワークスペースごとの合計の上限から、超えないことを下で確かめる。
/// 写真の行はレシピを通してワークスペースにつながる（外部キー）ので、アプリ全体の合計は、ワークスペースごとの合計の和である。
pub const MAX_PHOTO_BYTES_TOTAL: i64 = 5_000_000_000;
const _: () = assert!(MAX_WORKSPACES * MAX_PHOTO_BYTES_PER_WORKSPACE <= MAX_PHOTO_BYTES_TOTAL);
/// アプリ全体で 1 日（UTC の日付）に写真を追加できる回数。超えたら `503`（`capacity_reached`）。
///
/// 費用を守るための上限。月に 31 × 200 = 6,200 回で、R2 の書き込みの操作の無料枠（100 万回/月）より十分に小さい。
/// 写真を消しても回数は戻らない（`photo_uploads` の記録を残す）。
pub const MAX_PHOTO_UPLOADS_PER_DAY: i64 = 200;

/// 写真を配るときの `Cache-Control`。
///
/// 写真の ID は使い回さず、中身も変わらないので、ブラウザに 1 日残させて、R2 の読み取りを減らす。
/// 消した写真がブラウザに残り続けないよう、長くはしない。
/// ワークスペースのデータなので、共有のキャッシュには残させない（`private`）。
const PHOTO_CACHE_CONTROL: &str = "private, max-age=86400";
/// 写真を配るときの `Vary`。ワークスペースは API ではヘッダーで、画面では Cookie で決まるので、
/// 同じ URL でもワークスペースが違えば、ブラウザのキャッシュを使わせない。
const PHOTO_VARY: &str = "X-Workspace-Id, Cookie";
/// 写真を配るときの CSP。写真を直接開いたときに、何も読み込ませない。
/// ほかのサイトの枠（iframe）にも入れさせない（画面と同じ）。
const PHOTO_CSP: &str = "default-src 'none'; frame-ancestors 'none'; sandbox";

/// 写真の形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoFormat {
    Jpeg,
    Png,
    Webp,
}

impl PhotoFormat {
    /// 中身の先頭のバイト（マジックナンバー）から形式を決める。どれにも当たらなければ `None`。
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else {
            None
        }
    }

    /// `Content-Type` の値。
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

/// 写真の ID。ワークスペースの ID と同じ形（16 進数の小文字 32 文字）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoId(String);

impl PhotoId {
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

/// 保存された写真の情報。API の応答の本文と、DB の行の両方に使う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Photo {
    pub id: String,
    /// 形式（`image/jpeg`・`image/png`・`image/webp`）。
    pub content_type: String,
    /// 大きさ（バイト）。
    pub size: i64,
    /// 追加した日時（UNIX 時間のミリ秒）。
    pub created_at: i64,
}

/// 追加する写真の本文。
pub enum PhotoBody {
    /// メモリーに読んだバイト列。multipart のフォームと、`Content-Length` の無い本文と、テストで使う。
    Bytes(Vec<u8>),
    /// 先頭だけを読んだストリーム。本番で、写真のバイト列をそのまま送ったときに使う（[`crate::stream`]）。
    Stream(PhotoStream),
}

impl PhotoBody {
    /// 本文の大きさ（バイト）。
    pub fn len(&self) -> usize {
        match self {
            Self::Bytes(bytes) => bytes.len(),
            Self::Stream(stream) => stream.length,
        }
    }

    /// 形式を決めるのに使う、本文の先頭。
    fn head(&self) -> &[u8] {
        match self {
            Self::Bytes(bytes) => bytes,
            Self::Stream(stream) => &stream.head,
        }
    }
}

/// `Content-Type` が multipart か。
pub fn is_multipart(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|value| value.to_ascii_lowercase().starts_with("multipart/"))
}

/// ヘッダーの `Content-Type` が multipart か（[`is_multipart`]）。画面の上限の選び方と本文の読み方で、同じ判定を使う。
pub fn has_multipart_body(headers: &axum::http::HeaderMap) -> bool {
    is_multipart(
        headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
    )
}

/// 写真を追加するパス（`/recipes/<id>/photos`）なら、レシピの詳細のパスを返す。
///
/// 画面の 413 のエラーの画面と、API と画面のストリームの判定（`/api` を外して使う。[`crate::stream`]）の両方が使う。
pub fn photo_recipe_path(path: &str) -> Option<&str> {
    let recipe_path = path.strip_suffix("/photos")?;
    let id = recipe_path.strip_prefix("/recipes/")?;
    RecipeId::parse(id).map(|_| recipe_path)
}

/// 写真のバイト列そのものの本文を読む（API と画面で共通）。
///
/// 本番で、本文が axum に渡らず extension に入っているとき（[`crate::stream`]）は、ストリームのまま扱う。
/// そうでなければ本文を読む。`Content-Length` が写真の上限を超えるものは、middleware が本文を読む前に断っている
/// （[`crate::limits::limit_photo_writes`]・[`crate::screen::guard_photo_writes`]）。
pub async fn read_bytes_body(
    raw: Option<Extension<RawPhotoBody>>,
    request: Request,
) -> Result<PhotoBody, ApiError> {
    if let Some(Extension(raw)) = raw {
        return Ok(PhotoBody::Stream(stream::open(raw).await?));
    }
    let bytes = Bytes::from_request(request, &())
        .await
        .map_err(|rejection| ApiError::from_body_status(rejection.status()))?;
    // Bytes がほかに持ち主の無い 1 つの塊なら、コピーせずに Vec にする。
    Ok(PhotoBody::Bytes(Vec::from(bytes)))
}

/// R2 のキー。ワークスペース・レシピ・写真の ID から決まる。
///
/// D1 の行をワークスペースで絞り込んで見つけてから使うので、ほかのワークスペースの写真のキーにはならない。
pub fn object_key(workspace: &WorkspaceId, recipe: &RecipeId, photo: &str) -> String {
    format!("photos/{workspace}/{recipe}/{photo}")
}

/// `GET /api/recipes/{id}/photos` の応答の本文。
#[derive(Debug, Serialize)]
pub struct PhotoList {
    photos: Vec<Photo>,
}

/// パスのレシピの ID を読む。形が正しくない ID のレシピは在りえないので、404 にする。
fn parse_recipe_id(path: &str) -> Result<RecipeId, ApiError> {
    RecipeId::parse(path).ok_or(ApiError::RecipeNotFound)
}

/// パスのレシピと写真の ID を読む。
pub(crate) fn parse_ids(
    path: Result<Path<(String, String)>, PathRejection>,
) -> Result<(RecipeId, PhotoId), ApiError> {
    let Path((recipe, photo)) = path.map_err(|_| ApiError::RecipeNotFound)?;
    let recipe = parse_recipe_id(&recipe)?;
    let photo = PhotoId::parse(&photo).ok_or(ApiError::PhotoNotFound)?;
    Ok((recipe, photo))
}

/// 共有のデモでは、写真を追加・削除させない（読むだけにする）。
///
/// デモの ID は secret だが、漏れても写真を R2 に書き込ませないため、API でも断る。
/// デモのサンプルの写真は、seed が wrangler で D1 と R2 に直接入れる（Issue #8）。
fn deny_demo<B: Backend>(backend: &B, workspace: &WorkspaceId) -> Result<(), ApiError> {
    if backend.demo_workspace_id().as_ref() == Some(workspace) {
        return Err(ApiError::DemoPhotosDisabled);
    }
    Ok(())
}

/// `GET /api/recipes/{id}/photos`: レシピの写真の一覧を、追加した順に返す。
pub async fn list<B: Backend>(
    State(backend): State<B>,
    workspace: Workspace,
    recipe: Result<Path<String>, PathRejection>,
) -> Result<Json<PhotoList>, ApiError> {
    let Path(recipe) = recipe.map_err(|_| ApiError::RecipeNotFound)?;
    let recipe = parse_recipe_id(&recipe)?;
    let photos = list_photos(&backend, &workspace.id, &recipe).await?;
    Ok(Json(PhotoList { photos }))
}

/// レシピの写真の一覧。レシピが無ければ `RecipeNotFound`。API と画面の両方から使う。
pub async fn list_photos<B: Backend>(
    backend: &B,
    workspace: &WorkspaceId,
    recipe: &RecipeId,
) -> Result<Vec<Photo>, ApiError> {
    if backend.get_recipe(workspace, recipe).await?.is_none() {
        return Err(ApiError::RecipeNotFound);
    }
    Ok(backend.list_photos(workspace, recipe).await?)
}

/// `POST /api/recipes/{id}/photos`: 本文の写真を追加し、`201` と写真の情報を返す。
pub async fn create<B: Backend>(
    State(backend): State<B>,
    workspace: Workspace,
    recipe: Result<Path<String>, PathRejection>,
    raw: Option<Extension<RawPhotoBody>>,
    request: Request,
) -> Result<(StatusCode, Json<Photo>), ApiError> {
    let Path(recipe) = recipe.map_err(|_| ApiError::RecipeNotFound)?;
    let recipe = parse_recipe_id(&recipe)?;
    let body = read_bytes_body(raw, request).await?;
    let photo = add(&backend, &workspace.id, &recipe, body).await?;
    Ok((StatusCode::CREATED, Json(photo)))
}

/// 写真を追加する。API と画面の両方から使う。
///
/// 1. デモでないか、大きさ、形式を確かめる（D1 も R2 も使わない）。
/// 2. D1 に行と追加の記録を入れる。上限の判定と保存を 1 つのトランザクションで行い、同時に追加されても上限を超えない。
///    行には、R2 への書き込みが終わっていない印を付ける（画面と API に見せない。上限には数える）。
/// 3. R2 に書き込む。失敗したら、D1 の行を消す（追加の記録は残し、回数に数える）。
/// 4. 行の印を外す。外せなければ（書いている間にレシピが消された）、R2 のデータを消す。
///
/// 2 から 4 は、送り手との接続が切れても最後まで動かす（[`Backend::run_to_completion`]）。
/// それでも 3 か 4 が途中で失敗するか止められて、印が付いたまま行が残れば、定期の自動削除が片付ける（[`clean_pending`]）。
pub async fn add<B: Backend>(
    backend: &B,
    workspace: &WorkspaceId,
    recipe: &RecipeId,
    body: PhotoBody,
) -> Result<Photo, ApiError> {
    deny_demo(backend, workspace)?;
    if body.len() > MAX_PHOTO_BYTES {
        return Err(ApiError::PayloadTooLarge);
    }
    let format = PhotoFormat::detect(body.head()).ok_or(ApiError::UnsupportedPhotoType)?;
    let now = backend.now_millis();
    let photo = Photo {
        id: PhotoId::from_bytes(backend.random_128()).0,
        content_type: format.content_type().to_owned(),
        // 4,000,000 以下なので i64 に収まる。
        size: i64::try_from(body.len()).map_err(|_| ApiError::PayloadTooLarge)?,
        created_at: now,
    };
    let limits = PhotoLimits {
        max_per_recipe: MAX_PHOTOS_PER_RECIPE,
        max_bytes_per_workspace: MAX_PHOTO_BYTES_PER_WORKSPACE,
        max_uploads_since: MAX_PHOTO_UPLOADS_PER_DAY,
        // UTC の今日の 0 時。
        since: crate::limits::start_of_utc_day(now),
    };
    // D1 に行を入れるところからは、接続が切れても最後まで動かす。ストリームの本文は、D1 に行を入れた後に受け取るので、
    // 途中で切れて後始末（行を消す）が動かないと、読めない行が上限に数えられたまま残ってしまうため。
    let task = store(
        backend.clone(),
        workspace.clone(),
        recipe.clone(),
        photo,
        limits,
        body,
    );
    let Some(result) = backend.run_to_completion(task).await else {
        log_error(&BackendError(
            "写真を追加する処理が、結果を返さずに終わった".into(),
        ));
        return Err(ApiError::Internal);
    };
    result
}

/// [`add`] の 2 から 4。
async fn store<B: Backend>(
    backend: B,
    workspace: WorkspaceId,
    recipe: RecipeId,
    photo: Photo,
    limits: PhotoLimits,
    body: PhotoBody,
) -> Result<Photo, ApiError> {
    match backend
        .insert_photo(&workspace, &recipe, &photo, &limits)
        .await?
    {
        PhotoInsert::Inserted => {}
        PhotoInsert::RecipeNotFound => return Err(ApiError::RecipeNotFound),
        PhotoInsert::RecipeFull => return Err(ApiError::PhotoLimitReached),
        PhotoInsert::WorkspaceFull => return Err(ApiError::PhotoStorageLimitReached),
        PhotoInsert::CapacityReached => return Err(ApiError::CapacityReached),
    }
    write_object(
        backend,
        workspace,
        recipe,
        PhotoId(photo.id.clone()),
        body,
        photo.content_type.clone(),
    )
    .await?;
    Ok(photo)
}

/// [`add`] の 3 と 4。R2 に書き込み、失敗したら D1 の行を消す。書けたら行の印を外す。
/// 書いている間に行が消されていれば、R2 のデータを消す。
async fn write_object<B: Backend>(
    backend: B,
    workspace: WorkspaceId,
    recipe: RecipeId,
    id: PhotoId,
    body: PhotoBody,
    content_type: String,
) -> Result<(), ApiError> {
    let key = object_key(&workspace, &recipe, id.as_str());
    if let Err(error) = backend.put_object(&key, body, &content_type).await {
        // 元の失敗を先にログに出す（後始末も失敗したときに、元の原因を失わないため）。
        log_error(&error);
        // 行は、上限に数えられたまま残らないよう消す。消せなくても、印が付いたままなので見せず、自動削除が片付ける。
        if let Err(cleanup) = backend.delete_photo(&workspace, &recipe, &id).await {
            log_error(&cleanup);
        }
        return Err(ApiError::Internal);
    }
    // 書いている間にレシピが消されていれば、行はもう無く、印を外せない。データだけが R2 に残らないよう消す。
    // 印を外すのに失敗したときは、行に印が付いたまま残るので、自動削除が R2 のデータと一緒に片付ける。
    if !backend.finish_photo(&workspace, &recipe, &id).await? {
        backend.delete_objects(vec![key]).await?;
        return Err(
            if backend.get_recipe(&workspace, &recipe).await?.is_some() {
                ApiError::PhotoNotFound
            } else {
                ApiError::RecipeNotFound
            },
        );
    }
    Ok(())
}

/// `GET /api/recipes/{id}/photos/{photo_id}`: 写真のデータを返す。
pub async fn get<B: Backend>(
    State(backend): State<B>,
    workspace: Workspace,
    path: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (recipe, photo) = parse_ids(path)?;
    photo_response(&backend, &workspace.id, &recipe, &photo).await
}

/// 写真のデータの応答。API と画面の両方から使う。
pub async fn photo_response<B: Backend>(
    backend: &B,
    workspace: &WorkspaceId,
    recipe: &RecipeId,
    photo: &PhotoId,
) -> Result<Response, ApiError> {
    // D1 の行をワークスペースで絞り込んで見つけてから R2 を読む。ほかのワークスペースの写真は読めない。
    let Some(row) = backend.get_photo(workspace, recipe, photo).await? else {
        return Err(ApiError::PhotoNotFound);
    };
    let key = object_key(workspace, recipe, &row.id);
    let Some(bytes) = backend.get_object(&key).await? else {
        return Err(ApiError::PhotoNotFound);
    };
    let content_type = HeaderValue::from_str(&row.content_type).map_err(|_| ApiError::Internal)?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static(PHOTO_CACHE_CONTROL),
            ),
            (header::VARY, HeaderValue::from_static(PHOTO_VARY)),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(PHOTO_CSP),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// `DELETE /api/recipes/{id}/photos/{photo_id}`: 写真を消し、`204` を返す。
pub async fn delete<B: Backend>(
    State(backend): State<B>,
    workspace: Workspace,
    path: Result<Path<(String, String)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let (recipe, photo) = parse_ids(path)?;
    remove(&backend, &workspace.id, &recipe, &photo).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 写真を消す。API と画面の両方から使う。
///
/// 先に行に印を付けて隠し、R2 を消してから D1 の行を消す。
/// D1 の行を先に消して R2 の削除が失敗すると、合計に数えられないデータが R2 に残るため。
/// R2 の削除が失敗したら、印を外して元に戻す（利用者がもう一度消せるように）。
/// D1 の削除が失敗したら（R2 のデータはもう無い）、行は隠れたまま残り、定期の自動削除が片付ける（[`clean_pending`]）。
pub async fn remove<B: Backend>(
    backend: &B,
    workspace: &WorkspaceId,
    recipe: &RecipeId,
    photo: &PhotoId,
) -> Result<(), ApiError> {
    deny_demo(backend, workspace)?;
    if !backend
        .hide_photo(workspace, recipe, photo, backend.now_millis())
        .await?
    {
        return Err(ApiError::PhotoNotFound);
    }
    let key = object_key(workspace, recipe, photo.as_str());
    if let Err(error) = backend.delete_objects(vec![key]).await {
        log_error(&error);
        // 元に戻せなければ、隠れたまま残り、自動削除が片付ける。
        if let Err(restore) = backend.finish_photo(workspace, recipe, photo).await {
            log_error(&restore);
        }
        return Err(ApiError::Internal);
    }
    backend.delete_photo(workspace, recipe, photo).await?;
    Ok(())
}

/// 書き込みか削除が終わっていない印を付けてから、この時間より長く残った写真の行を、自動削除が片付ける（1 時間）。
/// 追加と削除の途中の行（後始末は `waitUntil` で 30 秒まで動く）を片付けないよう、十分に長くする。
/// 写真の本文を 1 時間より長く送り続けると、追加の途中で片付けられ、その写真は `404`（`photo_not_found`）になる。
pub const PENDING_CLEANUP_AFTER_MILLIS: i64 = 60 * 60 * 1_000;

/// 定期実行の 1 回で片付ける、1,000 行（[`MAX_KEYS_PER_DELETE`]）ずつのまとまりの数の上限。残りは次の実行に回す。
pub const MAX_PENDING_BATCHES_PER_RUN: i64 = 2;
/// [`clean_pending`] が 1 回に使う D1 と R2 の呼び出しの数の最大。選ぶ 1 回と、まとまりごとに R2 と D1 の 1 回ずつ。
/// 自動削除の 1 回の実行の呼び出しの数（[`crate::workspace::CLEANUP_SUBREQUESTS`]）に入る。
pub const PENDING_CLEANUP_SUBREQUESTS: i64 = 1 + 2 * MAX_PENDING_BATCHES_PER_RUN;

/// 印の付いた写真の行を片付けた結果。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PendingCleanup {
    /// 片付けた行の数。
    pub cleaned: usize,
    /// 片付けるのに失敗した行の数（理由はログに出す）。
    pub failed: usize,
    /// 1 回に片付ける数の上限（[`MAX_PENDING_BATCHES_PER_RUN`]）を超えたので、次の実行に回した行の数。
    pub deferred: usize,
    /// 片付ける行を選べなかったか。
    pub list_failed: bool,
}

/// 定期実行（自動削除）で呼ぶ。書き込みか削除の途中で失敗して、印が付いたまま残った写真の行を、R2 のデータと一緒に消す。
///
/// R2 を先に消す（[`remove`] と同じ理由）。R2 にデータが無くても（書く前に失敗した）、R2 の削除は失敗しない。
/// R2 のデータを 1 回の削除で消せる数（1,000）ずつまとめて消し、消せた分の行を D1 の batch でまとめて消す。
/// D1 の batch は、中の文の数によらず 1 回のクエリに数えられる。
/// 自動削除の 1 回の実行で呼べる D1 と R2 の数に上限がある（[`crate::workspace::MAX_SUBREQUESTS_PER_RUN`]）ので、
/// 1 回に片付けるのは [`MAX_PENDING_BATCHES_PER_RUN`] まとまりまでにし、残りは次の実行に回す
/// （呼び出しの数は [`PENDING_CLEANUP_SUBREQUESTS`] まで）。
/// 失敗した行は残し、次の実行でもう一度試す。batch が失敗すると、その 1,000 行までの行がすべて残る
/// （R2 のデータはもう無いが、行は隠れたまま上限に数えられる）。
/// 消すときは、選んだときの条件（印を付けた日時）を D1 で確かめ直し、選んだ後に印が外れた行は消さない。
pub async fn clean_pending<B: Backend>(backend: &B, now: i64) -> PendingCleanup {
    let mut summary = PendingCleanup::default();
    let before = now - PENDING_CLEANUP_AFTER_MILLIS;
    let rows = match backend.list_pending_photos(before).await {
        Ok(rows) => rows,
        Err(error) => {
            log_error(&error);
            summary.list_failed = true;
            return summary;
        }
    };
    let mut targets = Vec::new();
    for row in rows {
        let ids = (
            WorkspaceId::parse(&row.workspace_id),
            RecipeId::parse(&row.recipe_id),
            PhotoId::parse(&row.id),
        );
        // 形の正しくない ID の行（アプリは作らない）は、R2 のキーを作れないので、失敗に数えて知らせる。
        let (Some(workspace), Some(recipe), Some(photo)) = ids else {
            log_error(&BackendError(
                "片付ける写真の行の ID の形が正しくない".to_owned(),
            ));
            summary.failed += 1;
            continue;
        };
        targets.push((workspace, recipe, photo));
    }
    // 1,000 と 2 は usize に収まる。
    let per_delete = usize::try_from(MAX_KEYS_PER_DELETE).unwrap_or(1);
    let per_run = usize::try_from(MAX_PENDING_BATCHES_PER_RUN).unwrap_or(1) * per_delete;
    summary.deferred = targets.len().saturating_sub(per_run);
    targets.truncate(per_run);
    for chunk in targets.chunks(per_delete) {
        let keys = chunk
            .iter()
            .map(|(workspace, recipe, photo)| object_key(workspace, recipe, photo.as_str()))
            .collect();
        // 失敗の理由は、BackendError を ApiError に変えるところでログに出している。
        if delete_objects(backend, keys).await.is_err() {
            summary.failed += chunk.len();
            continue;
        }
        // 行は、R2 のデータを消せた分をまとめて消す（1 回の呼び出し。cron の 1 回の実行の呼び出しの数に上限があるため）。
        match backend.delete_photos(chunk, before).await {
            Ok(()) => summary.cleaned += chunk.len(),
            Err(error) => {
                log_error(&error);
                summary.failed += chunk.len();
            }
        }
    }
    summary
}

/// レシピの写真のデータを R2 から消す。
pub async fn remove_objects<B: Backend>(
    backend: &B,
    workspace: &WorkspaceId,
    recipe: &RecipeId,
    photos: &[String],
) -> Result<(), ApiError> {
    let keys = photos
        .iter()
        .map(|photo| object_key(workspace, recipe, photo))
        .collect();
    delete_objects(backend, keys).await
}

/// R2 の 1 回の削除で消せるキーの数。
pub const MAX_KEYS_PER_DELETE: i64 = 1_000;
// 1 つのワークスペースの写真を、1 回の削除で消せるようにする（上限を上げたら、ここで気付く）。
const _: () = assert!(
    crate::recipe::MAX_RECIPES_PER_WORKSPACE * MAX_PHOTOS_PER_RECIPE <= MAX_KEYS_PER_DELETE
);

/// 写真のデータを、キーで R2 から消す。キーが無ければ R2 を呼ばない。
///
/// キーは、1 回で消せる数（1,000）までを渡す（1 つのワークスペースの写真は、それに収まる）。
pub async fn delete_objects<B: Backend>(backend: &B, keys: Vec<String>) -> Result<(), ApiError> {
    if keys.is_empty() {
        return Ok(());
    }
    backend.delete_objects(keys).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::PhotoFormat;

    #[test]
    fn detects_formats_by_magic_bytes() {
        assert_eq!(
            PhotoFormat::detect(&[0xff, 0xd8, 0xff, 0xe0]),
            Some(PhotoFormat::Jpeg)
        );
        assert_eq!(
            PhotoFormat::detect(b"\x89PNG\r\n\x1a\n...."),
            Some(PhotoFormat::Png)
        );
        assert_eq!(
            PhotoFormat::detect(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Some(PhotoFormat::Webp)
        );
        // GIF・SVG・RIFF の別の形式（WAVE）・空は受け付けない。
        for bytes in [
            b"GIF89a".as_slice(),
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            b"RIFF\x00\x00\x00\x00WAVEfmt ",
            b"RIFF",
            b"",
        ] {
            assert_eq!(PhotoFormat::detect(bytes), None, "{bytes:?}");
        }
    }
}
