//! 画面（HTML）。askama のテンプレートを返し、htmx で一覧などを入れ替える。
//!
//! ワークスペースの ID は Cookie で受け取る。Cookie が無ければ共有のデモを、デモも無ければ「はじめる」画面を出す。
//! ワークスペースは、GET では作らない（検索エンジンの巡回などで、アプリ全体の 1 日の上限を使い切らせないため）。
//! 画面は API と同じ処理（`recipe`・`workspace`）を使い、上限や入力の決まりも API と同じにする。

use std::fmt::Write as _;

use askama::Template;
use axum::{
    Form,
    extract::{
        Path, Query, Request, State,
        rejection::{FormRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;

use crate::{
    backend::Backend,
    error::ApiError,
    limits::check_write,
    recipe::{self, MAX_PAGE, PAGE_SIZE, Recipe, RecipeId, RecipeInput, validate},
    workspace::{self, WorkspaceId},
};

/// ワークスペースの ID を入れる Cookie の名前。
///
/// `__Host-` で始まる名前の Cookie は、Secure・Path=/・Domain なしでしか保存されないので、
/// ほかのサブドメインや HTTP から書き換えられない。
const COOKIE_NAME: &str = "__Host-workspace";
/// Cookie を残す期間（400 日。ブラウザが受け付ける最長）。ワークスペースは 14 日使わないと消えるが、
/// 使っている間に Cookie が先に切れないよう、長くしておく。
const COOKIE_MAX_AGE_SECONDS: u32 = 400 * 24 * 60 * 60;

/// htmx に、アドレスバーの URL を変えるかを指示する応答のヘッダー。
const HX_PUSH_URL: HeaderName = HeaderName::from_static("hx-push-url");

/// 画面の応答に付けるヘッダー。
///
/// - CSP: スクリプトとスタイルは自分のドメインのファイル（htmx・CSS）だけにし、インラインを禁じる。
/// - Cache-Control: 画面にはワークスペースのデータが入るので、共有のキャッシュに残さない。
const SECURITY_HEADERS: [(HeaderName, &str); 4] = [
    (
        header::CONTENT_SECURITY_POLICY,
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
         connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
    ),
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::REFERRER_POLICY, "same-origin"),
    (header::CACHE_CONTROL, "no-store"),
];

/// 画面を見ている人が使うワークスペース。
enum Viewer {
    /// Cookie で受け取った、その人のワークスペース。
    Own(WorkspaceId),
    /// 共有のデモ。ID は画面に出さない。
    Demo(WorkspaceId),
}

impl Viewer {
    fn id(&self) -> &WorkspaceId {
        match self {
            Self::Own(id) | Self::Demo(id) => id,
        }
    }

    fn is_demo(&self) -> bool {
        matches!(self, Self::Demo(_))
    }
}

/// 画面の失敗。
pub enum ScreenError {
    /// API と同じ失敗。エラーの画面に、状態コードと説明を出す。
    Api(ApiError),
    /// 使えるワークスペースが無い。「はじめる」画面（`/`）へ移す。
    NoWorkspace,
}

impl From<ApiError> for ScreenError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl IntoResponse for ScreenError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => {
                let (status, _, message) = error.parts();
                render(status, &ErrorPage { message })
            }
            Self::NoWorkspace => see_other("/"),
        }
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    message: &'static str,
}

#[derive(Template)]
#[template(path = "landing.html")]
struct LandingPage {
    error: Option<&'static str>,
}

/// 一覧のページへのリンク。
struct PageLink {
    number: i64,
    href: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "list.html")]
struct ListPage {
    demo: bool,
    q: String,
    total: i64,
    page: i64,
    recipes: Vec<Recipe>,
    pages: Vec<PageLink>,
    first_page_href: String,
}

#[derive(Template)]
#[template(path = "detail.html")]
struct DetailPage {
    demo: bool,
    recipe: Recipe,
}

#[derive(Template)]
#[template(path = "form.html")]
struct FormPage {
    demo: bool,
    heading: &'static str,
    action: String,
    cancel_href: String,
    name: String,
    servings: String,
    ingredients: String,
    steps: String,
    error: Option<&'static str>,
    /// 誤りのある項目の名前。無ければ空。
    error_field: &'static str,
}

#[derive(Template)]
#[template(path = "workspace.html")]
struct WorkspacePage {
    demo: bool,
    own_id: Option<String>,
    error: Option<&'static str>,
}

/// テンプレートを HTML にして返す。テンプレートの失敗は、中身を出さずに 500 にする。
fn render<T: Template>(status: StatusCode, template: &T) -> Response {
    match template.render() {
        Ok(html) => (status, Html(html)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "画面を表示できませんでした",
        )
            .into_response(),
    }
}

/// `303 See Other` で `location` へ移す。POST の後に使い、再読み込みで二重に送られないようにする。
fn see_other(location: &str) -> Response {
    (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response()
}

/// ワークスペースの ID を保存する Cookie。
fn set_cookie(id: &WorkspaceId) -> HeaderValue {
    let value = format!(
        "{COOKIE_NAME}={id}; Path=/; Max-Age={COOKIE_MAX_AGE_SECONDS}; Secure; HttpOnly; SameSite=Lax"
    );
    // ID は 16 進数だけなので、ヘッダーの値として必ず正しい。
    HeaderValue::from_str(&value).expect("Cookie の値は ASCII だけでできている")
}

/// ワークスペースの ID の Cookie を消す。
fn clear_cookie() -> HeaderValue {
    HeaderValue::from_static("__Host-workspace=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax")
}

/// Cookie からワークスペースの ID を読む。無い・形が正しくない・2 つ以上あるときは `None`。
fn cookie_workspace(headers: &HeaderMap) -> Option<WorkspaceId> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        let Ok(text) = value.to_str() else {
            continue;
        };
        for pair in text.split(';') {
            let Some((name, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if name == COOKIE_NAME {
                // どちらのワークスペースか決められないので、使わない。
                if found.is_some() {
                    return None;
                }
                found = Some(value);
            }
        }
    }
    found.and_then(WorkspaceId::parse)
}

/// 画面を見ている人のワークスペースを決める。
///
/// 戻り値の 2 つ目は、Cookie に ID があったのに、そのワークスペースが無かったか（自動削除で消えたなど）。
/// そのときは Cookie を消し、デモ（無ければ「はじめる」画面）を出す。
async fn resolve<B: Backend>(
    backend: &B,
    headers: &HeaderMap,
) -> Result<(Option<Viewer>, bool), ApiError> {
    let mut stale = false;
    if let Some(id) = cookie_workspace(headers) {
        if workspace::touch(backend, &id).await? {
            return Ok((Some(Viewer::Own(id)), false));
        }
        stale = true;
    }
    if let Some(id) = backend.demo_workspace_id()
        && workspace::touch(backend, &id).await?
    {
        return Ok((Some(Viewer::Demo(id)), stale));
    }
    Ok((None, stale))
}

/// ワークスペースが要る画面で、使うワークスペースを決める。無ければ「はじめる」画面へ移す。
async fn require_viewer<B: Backend>(
    backend: &B,
    headers: &HeaderMap,
) -> Result<Viewer, ScreenError> {
    resolve(backend, headers)
        .await?
        .0
        .ok_or(ScreenError::NoWorkspace)
}

/// URL のクエリの値として使えるよう、パーセントエンコーディングにする（RFC 3986 の unreserved 以外）。
fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            // String への書き込みは失敗しない。
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// 一覧の URL。検索の文字列とページの番号を保つ。
fn list_href(q: &str, page: i64) -> String {
    let mut params = Vec::new();
    if !q.is_empty() {
        params.push(format!("q={}", encode(q)));
    }
    if page != 1 {
        params.push(format!("page={page}"));
    }
    if params.is_empty() {
        "/".to_owned()
    } else {
        format!("/?{}", params.join("&"))
    }
}

/// `GET /?q=<文字列>&page=<番号>`: レシピの一覧。ワークスペースが無ければ「はじめる」画面。
pub async fn index<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
    params: Result<Query<Vec<(String, String)>>, QueryRejection>,
) -> Result<Response, ScreenError> {
    let (viewer, stale) = resolve(&backend, &headers).await?;
    let mut response = match viewer {
        Some(viewer) => list(&backend, &viewer, params).await?,
        None => render(StatusCode::OK, &LandingPage { error: None }),
    };
    if stale {
        response
            .headers_mut()
            .append(header::SET_COOKIE, clear_cookie());
    }
    Ok(response)
}

async fn list<B: Backend>(
    backend: &B,
    viewer: &Viewer,
    params: Result<Query<Vec<(String, String)>>, QueryRejection>,
) -> Result<Response, ScreenError> {
    let Query(params) = params.map_err(|_| ApiError::InvalidField {
        field: "q",
        message: "検索の条件（q）が正しくありません",
    })?;
    let (pattern, page) = recipe::parse_list_params(&params)?;
    let result = recipe::list_page(backend, viewer.id(), pattern.as_deref(), page).await?;
    let q = pattern.unwrap_or_default();
    // レシピが無くても 1 ページ目は出す。レシピの数の上限があるので、ページの数は MAX_PAGE を超えない。
    let page_count = ((result.total + PAGE_SIZE - 1) / PAGE_SIZE).clamp(1, MAX_PAGE);
    let pages = (1..=page_count)
        .map(|number| PageLink {
            number,
            href: list_href(&q, number),
            current: number == page,
        })
        .collect();
    Ok(render(
        StatusCode::OK,
        &ListPage {
            demo: viewer.is_demo(),
            first_page_href: list_href(&q, 1),
            q,
            total: result.total,
            page,
            recipes: result.recipes,
            pages,
        },
    ))
}

/// 登録と編集のフォームの値。どれも文字列で受け取り、[`RecipeForm::to_input`] で API と同じ形にする。
#[derive(Debug, Default, Deserialize)]
pub struct RecipeForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    servings: String,
    #[serde(default)]
    ingredients: String,
    #[serde(default)]
    steps: String,
}

impl RecipeForm {
    /// API の本文と同じ形にする。
    ///
    /// - ブラウザは textarea の改行を CRLF で送るので、API と同じ LF にそろえる（長さの数え方もそろう）。
    /// - 何人分かが空なら「無い」、数として読めなければ範囲の外の値（NaN）にして、API と同じ誤りにする。
    fn to_input(&self) -> RecipeInput {
        let servings = self.servings.trim();
        RecipeInput {
            name: Some(self.name.clone()),
            servings: (!servings.is_empty()).then(|| servings.parse().unwrap_or(f64::NAN)),
            ingredients: Some(normalize_newlines(&self.ingredients)),
            steps: Some(normalize_newlines(&self.steps)),
        }
    }
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// フォームを、入力した値と誤りを保ったまま出し直す。
fn form_page(
    viewer: &Viewer,
    heading: &'static str,
    action: String,
    cancel_href: String,
    form: &RecipeForm,
    error: Option<&ApiError>,
) -> FormPage {
    let (error, error_field) = match error {
        Some(ApiError::InvalidField { field, message }) => (Some(*message), *field),
        Some(other) => (Some(other.parts().2), ""),
        None => (None, ""),
    };
    FormPage {
        demo: viewer.is_demo(),
        heading,
        action,
        cancel_href,
        name: form.name.clone(),
        servings: form.servings.clone(),
        ingredients: normalize_newlines(&form.ingredients),
        steps: normalize_newlines(&form.steps),
        error,
        error_field,
    }
}

/// フォームの本文を読めなかったとき（`Content-Type` が違うなど）の失敗。
fn form_error(_: &FormRejection) -> ScreenError {
    ScreenError::Api(ApiError::InvalidBody)
}

/// 入力の誤りとレシピの数の上限は、フォームに出し直す。それ以外の失敗は、エラーの画面にする。
fn is_form_error(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::InvalidField { .. } | ApiError::RecipeLimitReached
    )
}

/// `GET /recipes/new`: 登録のフォーム。
pub async fn new_form<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &headers).await?;
    let page = form_page(
        &viewer,
        "レシピを登録する",
        "/recipes".to_owned(),
        "/".to_owned(),
        &RecipeForm::default(),
        None,
    );
    Ok(render(StatusCode::OK, &page))
}

/// `POST /recipes`: レシピを登録し、詳細の画面へ移す。
pub async fn create<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
    form: Result<Form<RecipeForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &headers).await?;
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    let result = match validate(form.to_input()) {
        Ok(fields) => recipe::insert(&backend, viewer.id(), fields).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(recipe) => Ok(see_other(&format!("/recipes/{}", recipe.id))),
        Err(error) if is_form_error(&error) => {
            let (status, _, _) = error.parts();
            let page = form_page(
                &viewer,
                "レシピを登録する",
                "/recipes".to_owned(),
                "/".to_owned(),
                &form,
                Some(&error),
            );
            Ok(render(status, &page))
        }
        Err(error) => Err(error.into()),
    }
}

/// パスのレシピの ID を読む。形が正しくなければ、API と同じく「見つからない」にする。
fn parse_id(id: &str) -> Result<RecipeId, ScreenError> {
    RecipeId::parse(id).ok_or(ScreenError::Api(ApiError::RecipeNotFound))
}

/// ワークスペースのレシピを読む。無ければ「見つからない」にする。
async fn load<B: Backend>(
    backend: &B,
    viewer: &Viewer,
    id: &RecipeId,
) -> Result<Recipe, ScreenError> {
    backend
        .get_recipe(viewer.id(), id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ScreenError::Api(ApiError::RecipeNotFound))
}

/// `GET /recipes/{id}`: レシピの詳細。
pub async fn detail<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &headers).await?;
    let recipe = load(&backend, &viewer, &parse_id(&id)?).await?;
    Ok(render(
        StatusCode::OK,
        &DetailPage {
            demo: viewer.is_demo(),
            recipe,
        },
    ))
}

/// `GET /recipes/{id}/edit`: 編集のフォーム。今の値を入れておく。
pub async fn edit_form<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &headers).await?;
    let recipe = load(&backend, &viewer, &parse_id(&id)?).await?;
    let form = RecipeForm {
        servings: recipe.servings.to_string(),
        name: recipe.name,
        ingredients: recipe.ingredients,
        steps: recipe.steps,
    };
    let page = form_page(
        &viewer,
        "レシピを編集する",
        format!("/recipes/{}", recipe.id),
        format!("/recipes/{}", recipe.id),
        &form,
        None,
    );
    Ok(render(StatusCode::OK, &page))
}

/// `POST /recipes/{id}`: レシピの中身を置き換え、詳細の画面へ移す。
pub async fn update<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
    Path(id): Path<String>,
    form: Result<Form<RecipeForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &headers).await?;
    let id = parse_id(&id)?;
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    let result = match validate(form.to_input()) {
        Ok(fields) => recipe::replace(&backend, viewer.id(), &id, fields).await,
        Err(error) => Err(error),
    };
    let href = format!("/recipes/{id}");
    match result {
        Ok(_) => Ok(see_other(&href)),
        Err(error) if is_form_error(&error) => {
            let (status, _, _) = error.parts();
            let page = form_page(
                &viewer,
                "レシピを編集する",
                href.clone(),
                href,
                &form,
                Some(&error),
            );
            Ok(render(status, &page))
        }
        Err(error) => Err(error.into()),
    }
}

/// 削除のフォームの値。一覧から消したときは、同じ検索の条件とページに戻す。
#[derive(Debug, Deserialize)]
pub struct DeleteForm {
    q: Option<String>,
    page: Option<String>,
}

/// `POST /recipes/{id}/delete`: レシピを消し、一覧へ移す。
pub async fn delete<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
    Path(id): Path<String>,
    form: Result<Form<DeleteForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &headers).await?;
    let id = parse_id(&id)?;
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    if !backend
        .delete_recipe(viewer.id(), &id)
        .await
        .map_err(ApiError::from)?
    {
        return Err(ScreenError::Api(ApiError::RecipeNotFound));
    }
    // 戻り先は、検索の条件とページの番号から作り直す（送られた URL をそのまま使わない）。
    // 正しくない値なら、一覧の最初に戻す。
    let mut params = Vec::new();
    if let Some(q) = form.q {
        params.push(("q".to_owned(), q));
    }
    if let Some(page) = form.page {
        params.push(("page".to_owned(), page));
    }
    let location = recipe::parse_list_params(&params).map_or_else(
        |_| "/".to_owned(),
        |(q, page)| list_href(q.as_deref().unwrap_or_default(), page),
    );
    Ok(see_other(&location))
}

/// `GET /workspace`: ワークスペースの ID の表示と切り替え。
pub async fn workspace_page<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
) -> Result<Response, ScreenError> {
    let (viewer, _) = resolve(&backend, &headers).await?;
    let page = WorkspacePage {
        demo: viewer.as_ref().is_some_and(Viewer::is_demo),
        own_id: match viewer {
            Some(Viewer::Own(id)) => Some(id.to_string()),
            _ => None,
        },
        error: None,
    };
    Ok(render(StatusCode::OK, &page))
}

/// `POST /workspace`: ワークスペースを作り、Cookie に保存して一覧へ移す。
pub async fn create_workspace<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
) -> Result<Response, ScreenError> {
    match workspace::create_new(&backend, &headers).await {
        Ok(id) => {
            let mut response = see_other("/");
            response
                .headers_mut()
                .append(header::SET_COOKIE, set_cookie(&id));
            Ok(response)
        }
        // 回数や全体の上限に当たったときは、「はじめる」画面に理由を出す。
        Err(error @ (ApiError::TooManyRequests | ApiError::CapacityReached)) => {
            let (status, _, message) = error.parts();
            Ok(render(
                status,
                &LandingPage {
                    error: Some(message),
                },
            ))
        }
        Err(error) => Err(error.into()),
    }
}

/// 切り替えのフォームの値。
#[derive(Debug, Deserialize)]
pub struct SwitchForm {
    #[serde(default)]
    id: String,
}

/// `POST /workspace/switch`: 入力した ID のワークスペースに切り替える。
pub async fn switch_workspace<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
    form: Result<Form<SwitchForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    // 前後の空白は、コピーしたときに入りやすいので取り除く。
    let found = match WorkspaceId::parse(form.id.trim()) {
        Some(id) => workspace::touch(&backend, &id).await?.then_some(id),
        None => None,
    };
    let Some(id) = found else {
        let (viewer, _) = resolve(&backend, &headers).await?;
        let page = WorkspacePage {
            demo: viewer.as_ref().is_some_and(Viewer::is_demo),
            own_id: match viewer {
                Some(Viewer::Own(id)) => Some(id.to_string()),
                _ => None,
            },
            error: Some(
                "その ID のワークスペースは見つかりません。ID が正しいか、14 日使わずに消えていないかを確かめてください",
            ),
        };
        return Ok(render(StatusCode::NOT_FOUND, &page));
    };
    let mut response = see_other("/");
    response
        .headers_mut()
        .append(header::SET_COOKIE, set_cookie(&id));
    Ok(response)
}

/// `POST /workspace/forget`: このブラウザの Cookie を消す。ワークスペースそのものは消さない。
pub async fn forget_workspace() -> Response {
    let mut response = see_other("/");
    response
        .headers_mut()
        .append(header::SET_COOKIE, clear_cookie());
    response
}

/// 画面の書き込み（POST）の前に確かめる middleware。
///
/// 1. 別のサイトから送られたフォームでないか（`Origin` が自分のホストか）。違えば `403`。
///    Cookie は SameSite=Lax なので、別のサイトからの POST には付かないが、念のため重ねて確かめる。
/// 2. API と同じ、書き込みの回数と本文の大きさ（[`check_write`]）。
pub async fn guard_writes<B: Backend>(
    State(backend): State<B>,
    request: Request,
    next: Next,
) -> Response {
    let is_post = request.method() == Method::POST;
    let mut response = if is_post && !same_origin(request.headers()) {
        render(
            StatusCode::FORBIDDEN,
            &ErrorPage {
                message: "ほかのサイトから送られたフォームは受け付けません",
            },
        )
    } else if let Err(error) = check_write(&backend, request.method(), request.headers()).await {
        let (status, _, message) = error.parts();
        render(status, &ErrorPage { message })
    } else {
        next.run(request).await
    };
    // POST に移動（303）ではなく画面（入力の誤りやエラー）で答えたときは、htmx にアドレスバーの URL を変えさせない。
    // POST のパス（/recipes など）が残ると、再読み込みで GET を送り、受け付けないメソッドになるため。
    if is_post && !response.status().is_redirection() {
        response
            .headers_mut()
            .insert(HX_PUSH_URL, HeaderValue::from_static("false"));
    }
    response
}

/// `Origin` のホストが、リクエストの `Host` と同じか。どちらかが無ければ同じとみなさない。
fn same_origin(headers: &HeaderMap) -> bool {
    let header_text = |name| {
        headers
            .get(name)
            .and_then(|value: &HeaderValue| value.to_str().ok())
    };
    let (Some(origin), Some(host)) = (header_text(header::ORIGIN), header_text(header::HOST))
    else {
        return false;
    };
    origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .is_some_and(|authority| authority == host)
}

/// 画面の応答に、セキュリティのためのヘッダーを付ける middleware。
pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in SECURITY_HEADERS {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{cookie_workspace, encode, list_href, same_origin};

    fn cookies(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(header::COOKIE, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn cookie_workspace_reads_only_a_single_valid_id() {
        let id = "0123456789abcdef0123456789abcdef";
        let read = |values: &[&str]| cookie_workspace(&cookies(values)).map(|id| id.to_string());
        assert_eq!(
            read(&[&format!("a=1; __Host-workspace={id}; b=2")]).as_deref(),
            Some(id)
        );
        assert_eq!(
            read(&["a=1", &format!("__Host-workspace={id}")]).as_deref(),
            Some(id)
        );
        // 無い・形が正しくない・2 つある・名前の前置きが無いときは使わない。
        assert_eq!(read(&[]), None);
        assert_eq!(read(&["__Host-workspace=XYZ"]), None);
        assert_eq!(
            read(&[&format!("__Host-workspace={id}; __Host-workspace={id}")]),
            None
        );
        assert_eq!(read(&[&format!("workspace={id}")]), None);
    }

    #[test]
    fn list_href_keeps_query_and_page() {
        assert_eq!(list_href("", 1), "/");
        assert_eq!(list_href("", 2), "/?page=2");
        assert_eq!(
            list_href("カレー & ライス", 2),
            "/?q=%E3%82%AB%E3%83%AC%E3%83%BC%20%26%20%E3%83%A9%E3%82%A4%E3%82%B9&page=2"
        );
        assert_eq!(encode("a-b_c.d~e+f"), "a-b_c.d~e%2Bf");
    }

    #[test]
    fn same_origin_compares_origin_and_host() {
        let headers = |origin: Option<&str>, host: Option<&str>| {
            let mut headers = HeaderMap::new();
            if let Some(origin) = origin {
                headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
            }
            if let Some(host) = host {
                headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
            }
            headers
        };
        assert!(same_origin(&headers(
            Some("https://example.com"),
            Some("example.com")
        )));
        assert!(same_origin(&headers(
            Some("http://localhost:8787"),
            Some("localhost:8787")
        )));
        assert!(!same_origin(&headers(
            Some("https://evil.example"),
            Some("example.com")
        )));
        assert!(!same_origin(&headers(Some("null"), Some("example.com"))));
        assert!(!same_origin(&headers(None, Some("example.com"))));
        assert!(!same_origin(&headers(Some("https://example.com"), None)));
    }
}
