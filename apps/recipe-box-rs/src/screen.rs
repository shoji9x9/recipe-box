//! 画面（HTML）。askama のテンプレートを返し、htmx で一覧などを入れ替える。
//!
//! ワークスペースの ID は Cookie で受け取る。Cookie が無ければ共有のデモを、デモも無ければ「はじめる」画面を出す。
//! ワークスペースは、GET では作らない（検索エンジンの巡回などで、アプリ全体の 1 日の上限を使い切らせないため）。
//! 画面は API と同じ処理（`recipe`・`workspace`）を使い、上限や入力の決まりも API と同じにする。
//!
//! 入力の検証はサーバーだけで行う（ブラウザの標準の検証は使わない）。決まりとメッセージを API と 1 か所にまとめるため。
//! ページ全体へのメッセージ（「保存しました」・上限など）は、見出しの下に、自動では消えない形で出す。

use std::fmt::Write as _;

use askama::Template;
use axum::{
    Extension, Form,
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
    limits::{check_body_size, check_write},
    recipe::{self, FieldError, MAX_PAGE, PAGE_SIZE, Recipe, RecipeId, RecipeInput},
    workspace::{self, WorkspaceId},
};

/// ワークスペースの ID を入れる Cookie の名前。
///
/// `__Host-` で始まる名前の Cookie は、Secure・Path=/・Domain なしでしか保存されないので、
/// ほかのサブドメインや HTTP から書き換えられない。
const WORKSPACE_COOKIE: &str = "__Host-workspace";
/// ワークスペースの Cookie を残す期間（400 日。ブラウザが受け付ける最長）。ワークスペースは 14 日使わないと消えるが、
/// 使っている間に Cookie が先に切れないよう、長くしておく。
const WORKSPACE_COOKIE_MAX_AGE: u32 = 400 * 24 * 60 * 60;
/// 自分のワークスペースを持つ人が、一時的に共有のデモを見ていることを示す Cookie の名前。
const DEMO_VIEW_COOKIE: &str = "__Host-demo-view";
/// 一時的にデモを見る期間（1 時間）。過ぎると自分のレシピ帳に戻る。
/// デモを見ていることを忘れて、自分のレシピを共有のデモに書かないようにするため。
const DEMO_VIEW_MAX_AGE: u32 = 60 * 60;
/// 次の画面に一度だけ出すメッセージ（「保存しました」など）を渡す Cookie の名前。
/// 中身はメッセージの種類のコードだけで、文は入れない。
const FLASH_COOKIE: &str = "__Host-flash";
/// メッセージを渡す Cookie を残す期間。移動の直後に読むので短くする。
const FLASH_MAX_AGE: u32 = 60;

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

/// 次の画面に一度だけ出す、操作が済んだことの知らせ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flash {
    RecipeCreated,
    RecipeSaved,
    RecipeDeleted,
    WorkspaceCreated,
    WorkspaceSwitched,
    DemoViewStarted,
    DemoViewEnded,
    WorkspaceForgotten,
}

impl Flash {
    const ALL: [Self; 8] = [
        Self::RecipeCreated,
        Self::RecipeSaved,
        Self::RecipeDeleted,
        Self::WorkspaceCreated,
        Self::WorkspaceSwitched,
        Self::DemoViewStarted,
        Self::DemoViewEnded,
        Self::WorkspaceForgotten,
    ];

    /// Cookie に入れるコード。
    fn code(self) -> &'static str {
        match self {
            Self::RecipeCreated => "recipe-created",
            Self::RecipeSaved => "recipe-saved",
            Self::RecipeDeleted => "recipe-deleted",
            Self::WorkspaceCreated => "workspace-created",
            Self::WorkspaceSwitched => "workspace-switched",
            Self::DemoViewStarted => "demo-view-started",
            Self::DemoViewEnded => "demo-view-ended",
            Self::WorkspaceForgotten => "workspace-forgotten",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::RecipeCreated => "レシピを登録しました",
            Self::RecipeSaved => "レシピを保存しました",
            Self::RecipeDeleted => "レシピを削除しました",
            Self::WorkspaceCreated => {
                "あなた専用のレシピ帳を作りました。ID は「ワークスペース」の画面で確かめられます"
            }
            Self::WorkspaceSwitched => "ワークスペースを切り替えました",
            Self::DemoViewStarted => {
                "共有のデモに切り替えました。1 時間たつと、自分のレシピ帳に戻ります"
            }
            Self::DemoViewEnded => "自分のレシピ帳に戻りました",
            Self::WorkspaceForgotten => "このブラウザでの利用をやめました",
        }
    }

    fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|flash| flash.code() == code)
    }
}

/// リクエストの Cookie から読んだ、画面を見ている人の状態。[`identify`] が作ってハンドラーに渡す。
#[derive(Debug, Clone)]
pub struct Session {
    /// その人のワークスペース（在ることを確かめ、最終利用日時を更新した後のもの）。
    own: Option<WorkspaceId>,
    /// 自分のワークスペースを持ちながら、一時的に共有のデモを見ているか。
    viewing_demo: bool,
    /// 前の操作から渡された、一度だけ出すメッセージ。
    flash: Option<Flash>,
}

/// 画面を見ている人が使うワークスペース。
enum Viewer {
    /// Cookie で受け取った、その人のワークスペース。
    Own(WorkspaceId),
    /// 共有のデモ。ID は画面に出さない。`own_available` は、自分のワークスペースに戻れるか。
    Demo {
        id: WorkspaceId,
        own_available: bool,
    },
}

impl Viewer {
    fn id(&self) -> &WorkspaceId {
        match self {
            Self::Own(id) | Self::Demo { id, .. } => id,
        }
    }
}

/// すべての画面に共通の、見出しの上下に出すもの（base.html が使う）。
#[derive(Debug, Default)]
struct Layout {
    /// 共有のデモを見ていて、自分のワークスペースが無い（「自分だけのレシピ帳を作る」を出す）。
    demo_shared: bool,
    /// 自分のワークスペースを持ちながら、一時的にデモを見ている（「自分のレシピ帳に戻る」を出す）。
    demo_visiting: bool,
    /// 操作が済んだことの知らせ（緑）。
    flash: Option<&'static str>,
    /// ページ全体への誤りの知らせ（赤）。
    alert: Option<&'static str>,
}

impl Layout {
    fn new(viewer: Option<&Viewer>, session: &Session) -> Self {
        let (demo_shared, demo_visiting) = match viewer {
            Some(Viewer::Demo { own_available, .. }) => (!own_available, *own_available),
            _ => (false, false),
        };
        Self {
            demo_shared,
            demo_visiting,
            flash: session.flash.map(Flash::message),
            alert: None,
        }
    }

    fn with_alert(mut self, alert: &'static str) -> Self {
        self.alert = Some(alert);
        self
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
            Self::Api(error) => error_page(&error),
            Self::NoWorkspace => see_other("/"),
        }
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    layout: Layout,
}

/// エラーの画面。説明は見出しの下の知らせに出す。
fn error_page(error: &ApiError) -> Response {
    let (status, _, message) = error.parts();
    render(
        status,
        &ErrorPage {
            layout: Layout::default().with_alert(message),
        },
    )
}

#[derive(Template)]
#[template(path = "landing.html")]
struct LandingPage {
    layout: Layout,
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
    layout: Layout,
    /// 検索の条件の誤り。あれば一覧の代わりに出す（検索欄の htmx が入れ替える #results の中に出すため）。
    query_error: Option<&'static str>,
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
    layout: Layout,
    recipe: Recipe,
}

#[derive(Template)]
#[template(path = "form.html")]
struct FormPage {
    layout: Layout,
    heading: &'static str,
    action: String,
    cancel_href: String,
    name: String,
    servings: String,
    ingredients: String,
    steps: String,
    /// すべての項目の誤り（フォームの上の一覧に出す）。
    errors: Vec<FieldError>,
    name_error: Option<&'static str>,
    servings_error: Option<&'static str>,
    ingredients_error: Option<&'static str>,
    steps_error: Option<&'static str>,
    /// 送信して誤りが出た後か。後なら、欄から離れたときにも確かめ直す（reValidateMode: onBlur）。
    revalidate: bool,
    /// 誤りの一覧にフォーカスを移すか（送信したときだけ。欄から離れたときに移すと、入力の邪魔になる）。
    focus_summary: bool,
}

/// フォームを確かめた段階。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Validation {
    /// まだ送信していない。誤りは出さず、欄から離れても確かめない（mode: onSubmit）。
    NotSubmitted,
    /// 送信して誤りが出た。誤りの一覧にフォーカスを移し、以後は欄から離れたときにも確かめる。
    Submitted,
    /// 送信した後に、欄から離れて確かめ直した。フォーカスは動かさない。
    Revalidated,
}

#[derive(Template)]
#[template(path = "workspace.html")]
struct WorkspacePage {
    layout: Layout,
    /// 自分のワークスペースの ID（デモを見ている間も出す）。
    own_id: Option<String>,
    /// 共有のデモを見ているか。
    viewing_demo: bool,
    /// 共有のデモがあるか（「共有のデモを見る」を出すか）。
    demo_available: bool,
    /// 切り替えの欄の誤り。
    switch_error: Option<&'static str>,
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

/// 移動先の画面に、一度だけ `flash` を出す。
fn see_other_with(location: &str, flash: Flash) -> Response {
    let mut response = see_other(location);
    add_cookie(&mut response, FLASH_COOKIE, flash.code(), FLASH_MAX_AGE);
    response
}

/// 応答に Cookie を足す。値は ASCII の英数字と `-` だけを渡す。
fn add_cookie(response: &mut Response, name: &str, value: &str, max_age: u32) {
    let cookie =
        format!("{name}={value}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=Lax");
    // 名前と値は、このファイルの定数とコード（ASCII）だけなので、ヘッダーの値として必ず正しい。
    let value = HeaderValue::from_str(&cookie).expect("Cookie の値は ASCII だけでできている");
    response.headers_mut().append(header::SET_COOKIE, value);
}

/// 応答で Cookie を消す。
fn remove_cookie(response: &mut Response, name: &str) {
    add_cookie(response, name, "", 0);
}

/// Cookie の値を読む。無いか、同じ名前が 2 つ以上あれば `None`（どちらを使うか決められないため）。
fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        let Ok(text) = value.to_str() else {
            continue;
        };
        for pair in text.split(';') {
            let Some((key, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if key == name {
                if found.is_some() {
                    return None;
                }
                found = Some(value);
            }
        }
    }
    found
}

/// Cookie からワークスペースの ID を読む。無い・形が正しくない・2 つ以上あるときは `None`。
fn cookie_workspace(headers: &HeaderMap) -> Option<WorkspaceId> {
    cookie_value(headers, WORKSPACE_COOKIE).and_then(WorkspaceId::parse)
}

/// Cookie から、画面を見ている人の状態を読む middleware。すべての画面の前に動く。
///
/// - ワークスペースの Cookie があれば、在ることを確かめて最終利用日時を更新する。
///   無くなっていれば（自動削除など）、どの画面でも Cookie を消す。
/// - 一度だけ出すメッセージがあれば読み、画面を返すときに消す（移動のときは、移動先で読むので残す）。
pub async fn identify<B: Backend>(
    State(backend): State<B>,
    mut request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let flash = cookie_value(headers, FLASH_COOKIE).and_then(Flash::parse);
    let viewing_demo = cookie_value(headers, DEMO_VIEW_COOKIE) == Some("1");
    let cookie_id = cookie_workspace(headers);
    let (own, stale) = match cookie_id {
        Some(id) => match workspace::touch(&backend, &id).await {
            Ok(true) => (Some(id), false),
            Ok(false) => (None, true),
            Err(error) => return error_page(&error),
        },
        None => (None, false),
    };
    request.extensions_mut().insert(Session {
        own,
        viewing_demo,
        flash,
    });
    let mut response = next.run(request).await;
    if stale {
        remove_cookie(&mut response, WORKSPACE_COOKIE);
        remove_cookie(&mut response, DEMO_VIEW_COOKIE);
    }
    if flash.is_some() && !response.status().is_redirection() {
        remove_cookie(&mut response, FLASH_COOKIE);
    }
    response
}

/// 画面を見ている人が使うワークスペースを決める。
///
/// 1. 自分のワークスペースがあり、一時的にデモを見ていなければ、それを使う。
/// 2. 共有のデモがあれば、それを使う。
/// 3. デモが無くても、自分のワークスペースがあれば、それを使う（デモを見ている途中でデモが無くなったとき）。
async fn resolve<B: Backend>(backend: &B, session: &Session) -> Result<Option<Viewer>, ApiError> {
    if let Some(id) = &session.own
        && !session.viewing_demo
    {
        return Ok(Some(Viewer::Own(id.clone())));
    }
    if let Some(id) = backend.demo_workspace_id()
        && workspace::touch(backend, &id).await?
    {
        return Ok(Some(Viewer::Demo {
            id,
            own_available: session.own.is_some(),
        }));
    }
    Ok(session.own.clone().map(Viewer::Own))
}

/// ワークスペースが要る画面で、使うワークスペースを決める。無ければ「はじめる」画面へ移す。
async fn require_viewer<B: Backend>(backend: &B, session: &Session) -> Result<Viewer, ScreenError> {
    resolve(backend, session)
        .await?
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
    Extension(session): Extension<Session>,
    params: Result<Query<Vec<(String, String)>>, QueryRejection>,
) -> Result<Response, ScreenError> {
    let Some(viewer) = resolve(&backend, &session).await? else {
        return Ok(render(
            StatusCode::OK,
            &LandingPage {
                layout: Layout::new(None, &session),
            },
        ));
    };
    let params = params.map(|Query(params)| params).unwrap_or_default();
    let (pattern, page) = match recipe::parse_list_params(&params) {
        Ok(parsed) => parsed,
        // 検索の条件が正しくなければ、エラーの画面ではなく一覧の画面に誤りを出す。
        // 入力しながらの検索は #results だけを入れ替えるので、そこに誤りが無いと、一覧が黙って空になるため。
        Err(error) => {
            let message = error.parts().2;
            let q = params
                .iter()
                .find(|(key, _)| key == "q")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            return Ok(render(
                StatusCode::BAD_REQUEST,
                &ListPage {
                    layout: Layout::new(Some(&viewer), &session),
                    query_error: Some(message),
                    first_page_href: "/".to_owned(),
                    q,
                    total: 0,
                    page: 1,
                    recipes: Vec::new(),
                    pages: Vec::new(),
                },
            ));
        }
    };
    let result = recipe::list_page(&backend, viewer.id(), pattern.as_deref(), page).await?;
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
            layout: Layout::new(Some(&viewer), &session),
            query_error: None,
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

/// 登録か編集か。フォームの見出しと送り先を決める。
enum FormKind {
    New,
    Edit(String),
}

/// フォームを、入力した値と誤りを保ったまま出す。
fn form_page(
    layout: Layout,
    kind: &FormKind,
    form: &RecipeForm,
    errors: Vec<FieldError>,
    validation: Validation,
) -> FormPage {
    let error_of = |field: &str| {
        errors
            .iter()
            .find(|error| error.field == field)
            .map(|error| error.message)
    };
    let (heading, action, cancel_href) = match kind {
        FormKind::New => ("レシピを登録する", "/recipes".to_owned(), "/".to_owned()),
        FormKind::Edit(id) => (
            "レシピを編集する",
            format!("/recipes/{id}"),
            format!("/recipes/{id}"),
        ),
    };
    FormPage {
        heading,
        action,
        cancel_href,
        name: form.name.clone(),
        servings: form.servings.clone(),
        ingredients: normalize_newlines(&form.ingredients),
        steps: normalize_newlines(&form.steps),
        name_error: error_of("name"),
        servings_error: error_of("servings"),
        ingredients_error: error_of("ingredients"),
        steps_error: error_of("steps"),
        errors,
        layout,
        revalidate: validation != Validation::NotSubmitted,
        focus_summary: validation == Validation::Submitted,
    }
}

/// フォームの本文を読めなかったときの失敗。本文が上限より大きければ API と同じく 413、それ以外は 400。
fn form_error(rejection: &FormRejection) -> ScreenError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ScreenError::Api(ApiError::PayloadTooLarge)
    } else {
        ScreenError::Api(ApiError::InvalidBody)
    }
}

/// 登録と更新で共通の処理。入力の誤りとレシピの数の上限は、フォームに出し直す。
async fn save<B: Backend>(
    backend: &B,
    session: &Session,
    viewer: &Viewer,
    kind: FormKind,
    form: &RecipeForm,
) -> Result<Response, ScreenError> {
    let layout = || Layout::new(Some(viewer), session);
    let fields = match recipe::check(form.to_input()) {
        Ok(fields) => fields,
        Err(errors) => {
            return Ok(render(
                StatusCode::BAD_REQUEST,
                &form_page(layout(), &kind, form, errors, Validation::Submitted),
            ));
        }
    };
    let result = match &kind {
        FormKind::New => recipe::insert(backend, viewer.id(), fields)
            .await
            .map(|recipe| (recipe.id, Flash::RecipeCreated)),
        FormKind::Edit(id) => {
            let id = parse_id(id)?;
            recipe::replace(backend, viewer.id(), &id, fields)
                .await
                .map(|recipe| (recipe.id, Flash::RecipeSaved))
        }
    };
    match result {
        Ok((id, flash)) => Ok(see_other_with(&format!("/recipes/{id}"), flash)),
        Err(error @ ApiError::RecipeLimitReached) => {
            let (status, _, message) = error.parts();
            let page = form_page(
                layout().with_alert(message),
                &kind,
                form,
                Vec::new(),
                Validation::NotSubmitted,
            );
            Ok(render(status, &page))
        }
        Err(error) => Err(error.into()),
    }
}

/// `GET /recipes/new`: 登録のフォーム。
pub async fn new_form<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let page = form_page(
        Layout::new(Some(&viewer), &session),
        &FormKind::New,
        &RecipeForm::default(),
        Vec::new(),
        Validation::NotSubmitted,
    );
    Ok(render(StatusCode::OK, &page))
}

/// `POST /recipes`: レシピを登録し、詳細の画面へ移す。
pub async fn create<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    form: Result<Form<RecipeForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    save(&backend, &session, &viewer, FormKind::New, &form).await
}

/// `POST /recipes/check`: フォームを確かめるだけで、保存しない。欄から離れたときに htmx が呼ぶ。
///
/// 返す画面からは、離れた欄（入力と、そのすぐ下の誤り）だけを使う。誤りの一覧は使わない（送信したときだけ作る）。
/// 確かめた結果が誤りでも、確かめること自体は成功なので 200 にする。
/// ワークスペースを読まないので、[`identify`] を通さない（D1 を読まない）。
pub async fn check_form(
    form: Result<Form<RecipeForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    let errors = recipe::check(form.to_input()).err().unwrap_or_default();
    let page = form_page(
        Layout::default(),
        &FormKind::New,
        &form,
        errors,
        Validation::Revalidated,
    );
    Ok(render(StatusCode::OK, &page))
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
    Extension(session): Extension<Session>,
    Path(id): Path<String>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let recipe = load(&backend, &viewer, &parse_id(&id)?).await?;
    Ok(render(
        StatusCode::OK,
        &DetailPage {
            layout: Layout::new(Some(&viewer), &session),
            recipe,
        },
    ))
}

/// `GET /recipes/{id}/edit`: 編集のフォーム。今の値を入れておく。
pub async fn edit_form<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    Path(id): Path<String>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let recipe = load(&backend, &viewer, &parse_id(&id)?).await?;
    let kind = FormKind::Edit(recipe.id.clone());
    let form = RecipeForm {
        servings: recipe.servings.to_string(),
        name: recipe.name,
        ingredients: recipe.ingredients,
        steps: recipe.steps,
    };
    let page = form_page(
        Layout::new(Some(&viewer), &session),
        &kind,
        &form,
        Vec::new(),
        Validation::NotSubmitted,
    );
    Ok(render(StatusCode::OK, &page))
}

/// `POST /recipes/{id}`: レシピの中身を置き換え、詳細の画面へ移す。
pub async fn update<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    Path(id): Path<String>,
    form: Result<Form<RecipeForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let id = parse_id(&id)?;
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    save(
        &backend,
        &session,
        &viewer,
        FormKind::Edit(id.to_string()),
        &form,
    )
    .await
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
    Extension(session): Extension<Session>,
    Path(id): Path<String>,
    form: Result<Form<DeleteForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let id = parse_id(&id)?;
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    recipe::remove(&backend, viewer.id(), &id).await?;
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
    Ok(see_other_with(&location, Flash::RecipeDeleted))
}

/// ワークスペースの画面を作る。
async fn workspace_view<B: Backend>(
    backend: &B,
    session: &Session,
    switch_error: Option<&'static str>,
) -> Result<WorkspacePage, ApiError> {
    let viewer = resolve(backend, session).await?;
    Ok(WorkspacePage {
        layout: Layout::new(viewer.as_ref(), session),
        own_id: session.own.as_ref().map(ToString::to_string),
        viewing_demo: matches!(viewer, Some(Viewer::Demo { .. })),
        demo_available: backend.demo_workspace_id().is_some(),
        switch_error,
    })
}

/// `GET /workspace`: ワークスペースの ID の表示と切り替え。
pub async fn workspace_page<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
) -> Result<Response, ScreenError> {
    let page = workspace_view(&backend, &session, None).await?;
    Ok(render(StatusCode::OK, &page))
}

/// 自分のワークスペースに切り替えて、一覧へ移す。一時的にデモを見ていたら、それもやめる。
fn use_workspace(id: &WorkspaceId, flash: Flash) -> Response {
    let mut response = see_other_with("/", flash);
    add_cookie(
        &mut response,
        WORKSPACE_COOKIE,
        id.as_str(),
        WORKSPACE_COOKIE_MAX_AGE,
    );
    remove_cookie(&mut response, DEMO_VIEW_COOKIE);
    response
}

/// `POST /workspace`: ワークスペースを作り、Cookie に保存して一覧へ移す。
pub async fn create_workspace<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
) -> Result<Response, ScreenError> {
    match workspace::create_new(&backend, &headers).await {
        Ok(id) => Ok(use_workspace(&id, Flash::WorkspaceCreated)),
        // 回数や全体の上限に当たったときは、「はじめる」画面に理由を出す。
        Err(error @ (ApiError::TooManyRequests | ApiError::CapacityReached)) => {
            let (status, _, message) = error.parts();
            Ok(render(
                status,
                &LandingPage {
                    layout: Layout::new(None, &session).with_alert(message),
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
    Extension(session): Extension<Session>,
    form: Result<Form<SwitchForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    // 前後の空白は、コピーしたときに入りやすいので取り除く。
    let id = form.id.trim();
    let error = if id.is_empty() {
        "ワークスペースの ID を入力してください"
    } else {
        match WorkspaceId::parse(id) {
            Some(id) if workspace::touch(&backend, &id).await? => {
                return Ok(use_workspace(&id, Flash::WorkspaceSwitched));
            }
            _ => {
                "その ID のワークスペースは見つかりません。ID が正しいか、14 日使わずに消えていないかを確かめてください"
            }
        }
    };
    let status = if id.is_empty() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::NOT_FOUND
    };
    let page = workspace_view(&backend, &session, Some(error)).await?;
    Ok(render(status, &page))
}

/// `POST /workspace/forget`: このブラウザの Cookie を消す。ワークスペースそのものは消さない。
pub async fn forget_workspace() -> Response {
    let mut response = see_other_with("/", Flash::WorkspaceForgotten);
    remove_cookie(&mut response, WORKSPACE_COOKIE);
    remove_cookie(&mut response, DEMO_VIEW_COOKIE);
    response
}

/// `POST /workspace/demo`: 自分のワークスペースを残したまま、1 時間だけ共有のデモを見る。
pub async fn view_demo<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
) -> Result<Response, ScreenError> {
    // 自分のワークスペースが無い人は、もともとデモを見ている。デモが無ければ切り替えられない。
    if session.own.is_none() || backend.demo_workspace_id().is_none() {
        return Ok(see_other("/"));
    }
    let mut response = see_other_with("/", Flash::DemoViewStarted);
    add_cookie(&mut response, DEMO_VIEW_COOKIE, "1", DEMO_VIEW_MAX_AGE);
    Ok(response)
}

/// `POST /workspace/own`: 一時的に見ていた共有のデモから、自分のワークスペースに戻る。
pub async fn view_own() -> Response {
    let mut response = see_other_with("/", Flash::DemoViewEnded);
    remove_cookie(&mut response, DEMO_VIEW_COOKIE);
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
                layout: Layout::default()
                    .with_alert("ほかのサイトから送られたフォームは受け付けません"),
            },
        )
    } else if let Err(error) = check_write(
        &backend,
        request.method(),
        request.headers(),
        recipe::MAX_BODY_BYTES,
    )
    .await
    {
        error_page(&error)
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

/// 確かめるだけのルート（[`check_form`]）の前に確かめる middleware。送り元と本文の大きさだけを確かめる。
///
/// 書き込みの回数の制限は使わない。欄から離れるたびに呼ぶので、使うと保存の POST の枠を食い、
/// 直している途中で保存が 429 になるため。このルートは保存も D1 の読み取りもしないので、守るものが無い。
pub async fn guard_check(request: Request, next: Next) -> Response {
    let mut response = if !same_origin(request.headers()) {
        render(
            StatusCode::FORBIDDEN,
            &ErrorPage {
                layout: Layout::default()
                    .with_alert("ほかのサイトから送られたフォームは受け付けません"),
            },
        )
    } else if let Err(error) = check_body_size(request.headers(), recipe::MAX_BODY_BYTES) {
        error_page(&error)
    } else {
        next.run(request).await
    };
    response
        .headers_mut()
        .insert(HX_PUSH_URL, HeaderValue::from_static("false"));
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

    use super::{Flash, cookie_workspace, encode, list_href, same_origin};

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
    fn flash_codes_round_trip() {
        for flash in Flash::ALL {
            assert_eq!(Flash::parse(flash.code()), Some(flash));
        }
        assert_eq!(Flash::parse("unknown"), None);
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
