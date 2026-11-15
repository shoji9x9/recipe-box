//! レシピの写真の API（`/api/recipes/{id}/photos`）。
//!
//! 写真のデータは R2 に、上限の判定に使う大きさなどは D1 の `photos` に置く。
//! 費用を守るため、R2 に書き込む前に D1 で上限を判定する（docs/security.md の「費用の対策」）。
//!
//! 写真はバイト列をそのまま本文で受け取る（multipart にしない）。形式は `Content-Type` ではなく、
//! 中身の先頭のバイトで決める。宣言と中身が違うファイルを、別の形式として配らないため。

use axum::{
    Json,
    body::Bytes,
    extract::{
        Path, State,
        rejection::{BytesRejection, PathRejection},
    },
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::{
    backend::{Backend, PhotoInsert, PhotoLimits},
    error::{ApiError, log_error},
    id,
    recipe::RecipeId,
    workspace::{Workspace, WorkspaceId},
};

/// 1 枚の写真の大きさの上限（バイト）。超えたら `413`。
pub const MAX_PHOTO_BYTES: usize = 4_000_000;
/// 1 つのレシピの写真の数の上限。超えたら `409`（`photo_limit_reached`）。
pub const MAX_PHOTOS_PER_RECIPE: i64 = 5;
/// 1 つのワークスペースの写真の大きさの合計の上限（バイト）。超えたら `409`（`photo_storage_limit_reached`）。
///
/// 1 つのワークスペースが、アプリ全体の上限を使い切らないようにする。
/// ワークスペースが 50 個ほど写真を使っても、アプリ全体の上限に収まる大きさにする。
pub const MAX_PHOTO_BYTES_PER_WORKSPACE: i64 = 100_000_000;
/// アプリ全体の写真の大きさの合計の上限（バイト）。超えたら `503`（`capacity_reached`）。
///
/// 費用を守るための上限。R2 の保存量の無料枠（10 GB-月）の半分にする。
pub const MAX_PHOTO_BYTES_TOTAL: i64 = 5_000_000_000;
/// アプリ全体で 1 日（UTC の日付）に写真を追加できる回数。超えたら `503`（`capacity_reached`）。
///
/// 費用を守るための上限。月に 31 × 200 = 6,200 回で、R2 の書き込みの操作の無料枠（100 万回/月）より十分に小さい。
/// 写真を消しても回数は戻らない（`photo_uploads` の記録を残す）。
pub const MAX_PHOTO_UPLOADS_PER_DAY: i64 = 200;
/// 1 日のミリ秒。
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// 写真を配るときの `Cache-Control`。
///
/// 写真の ID は使い回さず、中身も変わらないので、ブラウザーに 1 日残させて、R2 の読み取りを減らす。
/// 消した写真がブラウザーに残り続けないよう、長くはしない。
/// ワークスペースのデータなので、共有のキャッシュには残させない（`private`）。
const PHOTO_CACHE_CONTROL: &str = "private, max-age=86400";
/// 写真を配るときの `Vary`。ワークスペースは API ではヘッダーで、画面では Cookie で決まるので、
/// 同じ URL でもワークスペースが違えば、ブラウザーのキャッシュを使わせない。
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
fn parse_ids(
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
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Photo>), ApiError> {
    let Path(recipe) = recipe.map_err(|_| ApiError::RecipeNotFound)?;
    let recipe = parse_recipe_id(&recipe)?;
    let body = body.map_err(|rejection| ApiError::from_body_status(rejection.status()))?;
    // Bytes がほかに持ち主の無い 1 つの塊なら、コピーせずに Vec にする。
    let photo = add(&backend, &workspace.id, &recipe, Vec::from(body)).await?;
    Ok((StatusCode::CREATED, Json(photo)))
}

/// 写真を追加する。API と画面の両方から使う。
///
/// 1. デモでないか、大きさ、形式を確かめる（D1 も R2 も使わない）。
/// 2. D1 に行と追加の記録を入れる。上限の判定と保存を 1 つのトランザクションで行い、同時に追加されても上限を超えない。
/// 3. R2 に書き込む。失敗したら、D1 の行を消す（追加の記録は残し、回数に数える）。
/// 4. 行がまだ在るかを確かめる。書いている間にレシピが消されていれば、R2 のデータを消す。
pub async fn add<B: Backend>(
    backend: &B,
    workspace: &WorkspaceId,
    recipe: &RecipeId,
    bytes: Vec<u8>,
) -> Result<Photo, ApiError> {
    deny_demo(backend, workspace)?;
    if bytes.len() > MAX_PHOTO_BYTES {
        return Err(ApiError::PayloadTooLarge);
    }
    let format = PhotoFormat::detect(&bytes).ok_or(ApiError::UnsupportedPhotoType)?;
    let now = backend.now_millis();
    let photo = Photo {
        id: PhotoId::from_bytes(backend.random_128()).0,
        content_type: format.content_type().to_owned(),
        // 4,000,000 以下なので i64 に収まる。
        size: i64::try_from(bytes.len()).map_err(|_| ApiError::PayloadTooLarge)?,
        created_at: now,
    };
    let limits = PhotoLimits {
        max_per_recipe: MAX_PHOTOS_PER_RECIPE,
        max_bytes_per_workspace: MAX_PHOTO_BYTES_PER_WORKSPACE,
        max_bytes_total: MAX_PHOTO_BYTES_TOTAL,
        max_uploads_since: MAX_PHOTO_UPLOADS_PER_DAY,
        // UTC の今日の 0 時。
        since: now - now.rem_euclid(DAY_MILLIS),
    };
    match backend
        .insert_photo(workspace, recipe, &photo, &limits)
        .await?
    {
        PhotoInsert::Inserted => {}
        PhotoInsert::RecipeNotFound => return Err(ApiError::RecipeNotFound),
        PhotoInsert::RecipeFull => return Err(ApiError::PhotoLimitReached),
        PhotoInsert::WorkspaceFull => return Err(ApiError::PhotoStorageLimitReached),
        PhotoInsert::CapacityReached => return Err(ApiError::CapacityReached),
    }
    let key = object_key(workspace, recipe, &photo.id);
    let id = PhotoId(photo.id.clone());
    if let Err(error) = backend.put_object(&key, bytes, &photo.content_type).await {
        // 元の失敗を先にログに出す（後始末も失敗したときに、元の原因を失わないため）。
        log_error(&error);
        // 行だけが残ると、合計の大きさに数えられたまま、読めない写真になる。
        if let Err(cleanup) = backend.delete_photo(workspace, recipe, &id).await {
            log_error(&cleanup);
        }
        return Err(ApiError::Internal);
    }
    // 書いている間にレシピか写真が消されていれば、行はもう無い。データだけが R2 に残らないよう消す。
    if backend.get_photo(workspace, recipe, &id).await?.is_none() {
        backend.delete_objects(vec![key]).await?;
        return Err(if backend.get_recipe(workspace, recipe).await?.is_some() {
            ApiError::PhotoNotFound
        } else {
            ApiError::RecipeNotFound
        });
    }
    Ok(photo)
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
/// R2 を先に消す。D1 の行を先に消して R2 の削除が失敗すると、合計に数えられないデータが R2 に残るため。
pub async fn remove<B: Backend>(
    backend: &B,
    workspace: &WorkspaceId,
    recipe: &RecipeId,
    photo: &PhotoId,
) -> Result<(), ApiError> {
    deny_demo(backend, workspace)?;
    if backend.get_photo(workspace, recipe, photo).await?.is_none() {
        return Err(ApiError::PhotoNotFound);
    }
    backend
        .delete_objects(vec![object_key(workspace, recipe, photo.as_str())])
        .await?;
    backend.delete_photo(workspace, recipe, photo).await?;
    Ok(())
}

/// レシピの写真のデータを R2 から消す。
pub async fn remove_objects<B: Backend>(
    backend: &B,
    workspace: &WorkspaceId,
    recipe: &RecipeId,
    photos: &[String],
) -> Result<(), ApiError> {
    if photos.is_empty() {
        return Ok(());
    }
    let keys = photos
        .iter()
        .map(|photo| object_key(workspace, recipe, photo))
        .collect();
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
