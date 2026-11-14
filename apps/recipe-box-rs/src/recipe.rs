//! レシピの API（`/api/recipes`）。
//!
//! どのハンドラーも [`Workspace`] を引数に取るので、`X-Workspace-Id` で指定したワークスペースの
//! レシピだけを読み書きする。ほかのワークスペースのレシピは、ID を知っていても見えない（404 になる）。

use std::fmt;

use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::{backend::Backend, error::ApiError, id, workspace::Workspace};

/// 料理名の長さの上限（Unicode のコードポイントの数）。
pub const MAX_NAME_CHARS: usize = 100;
/// 何人分かの範囲。
pub const SERVINGS_RANGE: std::ops::RangeInclusive<f64> = 1.0..=100.0;
/// 材料の長さの上限（Unicode のコードポイントの数）。
pub const MAX_INGREDIENTS_CHARS: usize = 5_000;
/// 手順の長さの上限（Unicode のコードポイントの数）。
pub const MAX_STEPS_CHARS: usize = 10_000;
/// 検索の文字列の長さの上限（Unicode のコードポイントの数）。
pub const MAX_QUERY_CHARS: usize = 100;
/// 一覧の 1 ページのレシピの数。
pub const PAGE_SIZE: i64 = 20;
/// 一覧のページの番号の上限。
///
/// OFFSET で飛ばす行も D1 は読むので、大きすぎる番号を受け付けない。
/// 1 ワークスペースのレシピの数の上限（#15）を決めたら、それに合わせて下げる。
pub const MAX_PAGE: i64 = 10_000;
/// リクエストの本文の大きさの上限（バイト）。
///
/// 上の上限いっぱいの JSON が収まる大きさにする。最も大きくなるのは、すべての文字を
/// `\uXXXX` のエスケープの組（絵文字なら 1 文字 12 バイト）で送ったときで、
/// 15,100 文字 × 12 バイト = 181,200 バイトに項目名などを足しても 256 KiB に収まる。
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// レシピの ID。ワークスペースの ID と同じ形（16 進数の小文字 32 文字）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeId(String);

impl RecipeId {
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

impl fmt::Display for RecipeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 保存されたレシピ。API の応答の本文と、DB の行の両方に使う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub servings: i64,
    pub ingredients: String,
    pub steps: String,
    /// 作った日時（UNIX 時間のミリ秒）。
    pub created_at: i64,
    /// 最後に変えた日時（UNIX 時間のミリ秒）。
    pub updated_at: i64,
}

/// 検索に使う形にする。ASCII の英字だけを小文字にし、ほかの文字は変えない。
///
/// 料理名を保存するときは `search_name` 列にこの形も入れ、検索の文字列もこの形にしてから比べる。
/// 両側をそろえるので、ASCII の大文字と小文字だけを区別しない検索になる。
/// DB の `LOWER` を使わないのは、SQLite は ASCII だけ、PostgreSQL は ASCII 以外も小文字にして、結果が変わるため。
pub fn search_name(text: &str) -> String {
    text.to_ascii_lowercase()
}

/// 検証を通った、レシピの中身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeFields {
    pub name: String,
    pub servings: i64,
    pub ingredients: String,
    pub steps: String,
}

/// `POST`・`PUT` の本文。
///
/// 項目が無いときに「どの項目が無いか」を返せるよう、すべて `Option` で受け取り、[`validate`] で確かめる。
/// 型が違う（`servings` が文字列など）ときは、ここで読めずに `invalid_body` になる。
///
/// `servings` は JSON の数値ならどれでも受け取り（`2.0`・`1e2`・桁の大きな整数も）、[`validate`] で
/// 「値が 1 から 100 までの整数か」を確かめる。JavaScript の `JSON.parse` は `2` と `2.0` を区別できないので、
/// 書き方ではなく値で決めると、リライトの後（`Number.isInteger`）と結果がそろう。
#[derive(Debug, Deserialize)]
pub struct RecipeInput {
    name: Option<String>,
    servings: Option<f64>,
    ingredients: Option<String>,
    steps: Option<String>,
}

/// 入力の項目を確かめる。正しくなければ、最初に見つかった項目のエラーを返す。
///
/// 長さは Unicode のコードポイントの数で数える（`chars().count()`）。
/// バイト数や UTF-16 の長さで数えると、日本語や絵文字でリライトの後と結果が変わるため。
pub fn validate(input: RecipeInput) -> Result<RecipeFields, ApiError> {
    let invalid = |field, message| ApiError::InvalidField { field, message };

    let name = input
        .name
        .ok_or(invalid("name", "料理名（name）は必須です"))?;
    // trim は Unicode の White_Space の文字を空白とみなす。
    // JavaScript の trim は U+FEFF なども空白とみなすので、リライトの後は /^\p{White_Space}*$/u で同じ判定をする。
    if name.trim().is_empty() {
        return Err(invalid("name", "料理名（name）を入力してください"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(invalid("name", "料理名（name）は 100 文字までです"));
    }

    let servings = input
        .servings
        .ok_or(invalid("servings", "何人分か（servings）は必須です"))?;
    let servings = servings_to_int(servings).ok_or(invalid(
        "servings",
        "何人分か（servings）は 1 から 100 までの整数です",
    ))?;

    // 材料と手順は省いてよい。省いたら空にする。
    let ingredients = input.ingredients.unwrap_or_default();
    if ingredients.chars().count() > MAX_INGREDIENTS_CHARS {
        return Err(invalid(
            "ingredients",
            "材料（ingredients）は 5,000 文字までです",
        ));
    }
    let steps = input.steps.unwrap_or_default();
    if steps.chars().count() > MAX_STEPS_CHARS {
        return Err(invalid("steps", "手順（steps）は 10,000 文字までです"));
    }

    Ok(RecipeFields {
        name,
        servings,
        ingredients,
        steps,
    })
}

/// 何人分かの値が 1 から 100 までの整数なら、整数にして返す。
fn servings_to_int(value: f64) -> Option<i64> {
    if value.fract() != 0.0 || !SERVINGS_RANGE.contains(&value) {
        return None;
    }
    // 範囲と小数部を確かめた後なので、整数への変換で値は変わらない。
    #[allow(clippy::cast_possible_truncation)]
    Some(value as i64)
}

/// 本文の読み取りの失敗を、API のエラーにする。大きすぎるときだけ 413 にする。
fn body_error(rejection: &JsonRejection) -> ApiError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::PayloadTooLarge
    } else {
        ApiError::InvalidBody
    }
}

/// パスの ID を読む。形が正しくない ID のレシピは在りえないので、404 にする。
/// パーセントエンコーディングを外すと UTF-8 にならない ID（`%ff` など）も、axum の既定の 400 ではなく 404 にする。
fn parse_recipe_id(path: Result<Path<String>, PathRejection>) -> Result<RecipeId, ApiError> {
    let Path(value) = path.map_err(|_| ApiError::RecipeNotFound)?;
    RecipeId::parse(&value).ok_or(ApiError::RecipeNotFound)
}

/// `GET /api/recipes` の応答の本文。
#[derive(Debug, Serialize)]
pub struct RecipeList {
    recipes: Vec<Recipe>,
    /// 条件に当てはまるレシピの総数（すべてのページの合計）。
    total: i64,
    /// このページの番号（1 から数える）。
    page: i64,
    /// 1 ページのレシピの数。
    per_page: i64,
}

/// クエリから、名前が `key` の値を取り出す。無ければ `None`、2 つ以上あれば `Err`。
fn single_param<'a>(params: &'a [(String, String)], key: &str) -> Result<Option<&'a str>, ()> {
    let mut values = params
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.as_str());
    let first = values.next();
    if values.next().is_some() {
        return Err(());
    }
    Ok(first)
}

/// ページの番号を読む。省くと 1 ページ目。数字だけで書いた 1 から [`MAX_PAGE`] までの整数を受け付ける。
fn parse_page(value: Option<&str>) -> Option<i64> {
    let Some(value) = value else {
        return Some(1);
    };
    // i64::from_str は先頭の + を受け付けるので、数字だけかを先に確かめる。
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|page| (1..=MAX_PAGE).contains(page))
}

/// `GET /api/recipes?q=<文字列>&page=<番号>`: レシピの一覧を、作った日時の新しい順に 1 ページ分返す。
pub async fn list<B: Backend>(
    State(backend): State<B>,
    workspace: Workspace,
    params: Result<Query<Vec<(String, String)>>, QueryRejection>,
) -> Result<Json<RecipeList>, ApiError> {
    let invalid_q = || ApiError::InvalidField {
        field: "q",
        message: "検索の条件（q）が正しくありません",
    };
    // 名前と値の組の並びとしてはどんなクエリでも読めるので、ここで失敗することは無いはずだが、
    // axum の既定のエラー（JSON でない本文）を返さないよう、q のエラーにする。
    let Query(params) = params.map_err(|_| invalid_q())?;
    let pattern = single_param(&params, "q")
        .map_err(|()| invalid_q())?
        .filter(|q| !q.is_empty());
    if pattern.is_some_and(|q| q.chars().count() > MAX_QUERY_CHARS) {
        return Err(ApiError::InvalidField {
            field: "q",
            message: "検索の条件（q）は 100 文字までです",
        });
    }
    let page = single_param(&params, "page")
        .ok()
        .and_then(parse_page)
        .ok_or(ApiError::InvalidField {
            field: "page",
            message: "ページ（page）は 1 から 10000 までの整数です",
        })?;
    let query = pattern.map(search_name);
    let result = backend
        .list_recipes(
            &workspace.id,
            query.as_deref(),
            PAGE_SIZE,
            (page - 1) * PAGE_SIZE,
        )
        .await?;
    Ok(Json(RecipeList {
        recipes: result.recipes,
        total: result.total,
        page,
        per_page: PAGE_SIZE,
    }))
}

/// `POST /api/recipes`: レシピを登録し、`201` と登録したレシピを返す。
pub async fn create<B: Backend>(
    State(backend): State<B>,
    workspace: Workspace,
    body: Result<Json<RecipeInput>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(input) = body.map_err(|rejection| body_error(&rejection))?;
    let fields = validate(input)?;
    let id = RecipeId::from_bytes(backend.random_128());
    let now = backend.now_millis();
    let recipe = Recipe {
        id: id.to_string(),
        name: fields.name,
        servings: fields.servings,
        ingredients: fields.ingredients,
        steps: fields.steps,
        created_at: now,
        updated_at: now,
    };
    backend.insert_recipe(&workspace.id, &recipe).await?;
    let location = format!("/api/recipes/{id}");
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(recipe),
    )
        .into_response())
}

/// `GET /api/recipes/{id}`: レシピを 1 件返す。
pub async fn get<B: Backend>(
    State(backend): State<B>,
    workspace: Workspace,
    id: Result<Path<String>, PathRejection>,
) -> Result<Json<Recipe>, ApiError> {
    let id = parse_recipe_id(id)?;
    backend
        .get_recipe(&workspace.id, &id)
        .await?
        .map(Json)
        .ok_or(ApiError::RecipeNotFound)
}

/// `PUT /api/recipes/{id}`: レシピの中身を置き換え、変えた後のレシピを返す。
pub async fn update<B: Backend>(
    State(backend): State<B>,
    workspace: Workspace,
    id: Result<Path<String>, PathRejection>,
    body: Result<Json<RecipeInput>, JsonRejection>,
) -> Result<Json<Recipe>, ApiError> {
    let id = parse_recipe_id(id)?;
    let Json(input) = body.map_err(|rejection| body_error(&rejection))?;
    let fields = validate(input)?;
    let now = backend.now_millis();
    if !backend
        .update_recipe(&workspace.id, &id, &fields, now)
        .await?
    {
        return Err(ApiError::RecipeNotFound);
    }
    // 作った日時などを含めて返すため、変えた後のレシピを読み直す。
    backend
        .get_recipe(&workspace.id, &id)
        .await?
        .map(Json)
        .ok_or(ApiError::RecipeNotFound)
}

/// `DELETE /api/recipes/{id}`: レシピを消し、`204` を返す。
pub async fn delete<B: Backend>(
    State(backend): State<B>,
    workspace: Workspace,
    id: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let id = parse_recipe_id(id)?;
    if backend.delete_recipe(&workspace.id, &id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::RecipeNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::{RecipeInput, validate};
    use crate::error::ApiError;

    fn input(name: Option<&str>, servings: Option<f64>) -> RecipeInput {
        RecipeInput {
            name: name.map(str::to_owned),
            servings,
            ingredients: None,
            steps: None,
        }
    }

    fn field_of(result: Result<impl std::fmt::Debug, ApiError>) -> &'static str {
        match result {
            Err(ApiError::InvalidField { field, .. }) => field,
            other => panic!("invalid_field ではない: {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_minimum_and_maximum() {
        let fields = validate(input(Some("カレー"), Some(1.0))).unwrap();
        assert_eq!(fields.ingredients, "");
        assert_eq!(fields.steps, "");
        validate(input(Some(&"あ".repeat(100)), Some(100.0))).unwrap();
        validate(RecipeInput {
            name: Some("カレー".into()),
            servings: Some(2.0),
            ingredients: Some("🍛".repeat(5_000)),
            steps: Some("手".repeat(10_000)),
        })
        .unwrap();
    }

    #[test]
    fn validate_rejects_missing_or_blank_name() {
        assert_eq!(field_of(validate(input(None, Some(2.0)))), "name");
        assert_eq!(field_of(validate(input(Some(""), Some(2.0)))), "name");
        assert_eq!(
            field_of(validate(input(Some(" \n\u{3000}"), Some(2.0)))),
            "name"
        );
    }

    #[test]
    fn validate_rejects_too_long_name() {
        // 101 文字。絵文字も 1 文字と数える。
        let name = format!("{}🍛", "あ".repeat(100));
        assert_eq!(field_of(validate(input(Some(&name), Some(2.0)))), "name");
    }

    #[test]
    fn validate_rejects_servings_out_of_range() {
        for servings in [
            None,
            Some(0.0),
            Some(-1.0),
            Some(101.0),
            Some(2.5),
            Some(1e20),
            Some(f64::MAX),
        ] {
            assert_eq!(
                field_of(validate(input(Some("カレー"), servings))),
                "servings",
                "{servings:?}"
            );
        }
    }

    #[test]
    fn validate_rejects_too_long_ingredients_and_steps() {
        let mut long_ingredients = input(Some("カレー"), Some(2.0));
        long_ingredients.ingredients = Some("a".repeat(5_001));
        assert_eq!(field_of(validate(long_ingredients)), "ingredients");

        let mut long_steps = input(Some("カレー"), Some(2.0));
        long_steps.steps = Some("a".repeat(10_001));
        assert_eq!(field_of(validate(long_steps)), "steps");
    }
}
