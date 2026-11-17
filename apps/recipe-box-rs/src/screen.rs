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
    body::Bytes,
    extract::{
        FromRequest, Multipart, Path, Query, Request, State,
        multipart::MultipartRejection,
        rejection::{BytesRejection, FormRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header},
    middleware::Next,
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;

use crate::{
    backend::{Backend, ListedRecipe},
    error::ApiError,
    limits::{check_body_size, check_write},
    photo::{self, Photo, PhotoBody},
    recipe::{self, FieldError, MAX_PAGE, PAGE_SIZE, Recipe, RecipeId, RecipeInput},
    stream::RawPhotoBody,
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

/// 写真を送るフォーム（multipart）の本文の大きさの上限（バイト）。
/// 1 枚の写真の上限に、multipart の区切りやヘッダーの分（64 KiB）を足す。写真そのものの大きさは、別に確かめる。
pub const PHOTO_FORM_MAX_BYTES: usize = photo::MAX_PHOTO_BYTES + 64 * 1024;

/// htmx に、アドレスバーの URL を変えるかを指示する応答のヘッダー。
const HX_PUSH_URL: HeaderName = HeaderName::from_static("hx-push-url");

/// 画面の応答に付けるヘッダー。
///
/// - CSP: スクリプトとスタイルは自分のドメインのファイル（htmx・写真の追加の JavaScript・CSS）だけにし、インラインを禁じる。
///   写真の追加の JavaScript は、同じドメインに写真を送り（`connect-src`）、失敗の応答の画面から知らせの文を読む。
/// - Cache-Control: 画面にはワークスペースのデータが入るので、共有のキャッシュに残さない。
// guide:start security-headers
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
// guide:end security-headers

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
    WorkspaceDeleted,
    PhotoAdded,
    PhotoDeleted,
    /// Cookie の ID のワークスペースが無くなっていた（自動削除など）。
    /// ほかと違い、誤りの知らせ（赤）として出す（[`Layout::gone`]）。
    WorkspaceGone,
}

impl Flash {
    const ALL: [Self; 11] = [
        Self::RecipeCreated,
        Self::RecipeSaved,
        Self::RecipeDeleted,
        Self::WorkspaceCreated,
        Self::WorkspaceSwitched,
        Self::DemoViewStarted,
        Self::DemoViewEnded,
        Self::WorkspaceDeleted,
        Self::PhotoAdded,
        Self::PhotoDeleted,
        Self::WorkspaceGone,
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
            Self::WorkspaceDeleted => "workspace-deleted",
            Self::PhotoAdded => "photo-added",
            Self::PhotoDeleted => "photo-deleted",
            Self::WorkspaceGone => "workspace-gone",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::RecipeCreated => "レシピを登録しました",
            Self::RecipeSaved => "レシピを保存しました",
            Self::RecipeDeleted => "レシピを削除しました",
            Self::WorkspaceCreated => {
                "あなた専用のレシピ帳を作りました。14 日アクセスが無いと、レシピと写真ごと自動で消えます。\
                 ID は「ワークスペース」の画面で確かめられるので、控えておいてください"
            }
            Self::WorkspaceSwitched => "ワークスペースを切り替えました",
            Self::DemoViewStarted => {
                "共有のデモに切り替えました。1 時間たつと、自分のレシピ帳に戻ります"
            }
            Self::DemoViewEnded => "自分のレシピ帳に戻りました",
            Self::WorkspaceDeleted => "ワークスペースを削除しました",
            Self::PhotoAdded => "写真を追加しました",
            Self::PhotoDeleted => "写真を削除しました",
            Self::WorkspaceGone => {
                "このブラウザが覚えていたワークスペースは見つかりませんでした。\
                 14 日アクセスが無くて消えたか、削除されたためです"
            }
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
    /// 共有のデモの ID は入らない（[`identify`] が外す。[`resolve`] などはこれを前提にする）。
    own: Option<WorkspaceId>,
    /// 自分のワークスペースを持ちながら、一時的に共有のデモを見ているか。
    viewing_demo: bool,
    /// 前の操作から渡された、一度だけ出すメッセージ（[`Flash::WorkspaceGone`] は入らない。`workspace_gone` にする）。
    flash: Option<Flash>,
    /// 覚えていたワークスペースが無くなっていたことを、画面に知らせるか。
    /// この要求で分かった（[`identify`] が Cookie を消す）か、前の応答から一度だけ出すメッセージで回されてきた。
    workspace_gone: bool,
}

/// 書き込みの回数の制限に当たったことを、ハンドラーに渡す印。[`guard`] が付ける。
///
/// 理由を出す画面が決まっているフォーム（[`shows_write_limit_itself`]）だけに付け、middleware では断らない。
/// 印を受け取ったハンドラーは、何も書き込まずに、その画面に理由を出す（`429`）。
#[derive(Debug, Clone, Copy)]
pub struct WriteLimited;

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
    /// 覚えていたワークスペースが無くなっていたことの知らせ（赤。[`Flash::WorkspaceGone`]）。
    /// `alert` とは別に持ち、ほかの誤りと一緒でも出す。出した画面の応答には [`GoneNoticeShown`] を付ける。
    gone: Option<&'static str>,
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
            gone: session
                .workspace_gone
                .then(|| Flash::WorkspaceGone.message()),
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
    /// 画面だけの失敗。エラーの画面に、状態コードと説明を出す（API の説明が画面に合わないとき）。
    Page(StatusCode, &'static str),
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
            Self::Page(status, message) => message_page(status, message),
        }
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    layout: Layout,
    /// 「編集の画面に戻る」の戻り先（写真の追加の失敗）。`None` なら「一覧に戻る」にする。
    edit_href: Option<String>,
}

/// エラーの画面。説明は見出しの下の知らせに出す。
fn error_page(error: &ApiError) -> Response {
    let (status, _, message) = error.parts();
    message_page(status, message)
}

/// `message` を見出しの下の知らせに出す、エラーの画面。
fn message_page(status: StatusCode, message: &'static str) -> Response {
    render(
        status,
        &ErrorPage {
            layout: Layout::default().with_alert(message),
            edit_href: None,
        },
    )
}

/// 画面の無いパスへの応答（router の fallback が呼ぶ）。ブラウザ自身のエラー画面ではなく、一覧に戻れるエラーの画面を出す。
/// `route_layer` の外なので、画面と同じヘッダーをここで付ける。
pub fn not_found_page() -> Response {
    let mut response = message_page(StatusCode::NOT_FOUND, "ページが見つかりません");
    add_security_headers(&mut response);
    response
}

/// 画面のパスが、そのメソッドを受け付けないときの応答（`method_not_allowed_fallback`）。
///
/// POST だけのパスへの GET（と HEAD）は、そのパスに関係する画面へ `303` で移す。
/// JavaScript が動かずに POST の後の画面（入力の誤りなど）が出ているとき、アドレスバーで Enter を押すと、
/// その POST のパスに GET が届く。エラーの画面より、操作を続けられる画面に移すほうが使いやすいためである。
/// ほかのメソッド（作った要求でしか届かない）は、`405` のエラーの画面にする。
/// `route_layer` の外なので（ワークスペースを読まない）、画面と同じヘッダーをここで付ける。
pub async fn method_not_allowed(method: Method, uri: Uri) -> Response {
    let mut response = if matches!(method, Method::GET | Method::HEAD) {
        see_other(&page_for_post_path(uri.path()))
    } else {
        message_page(
            StatusCode::METHOD_NOT_ALLOWED,
            "このページは、この方法では開けません",
        )
    };
    add_security_headers(&mut response);
    response
}

/// POST だけのパス（GET のルートの無いもの）に GET が届いたときの移り先。
///
/// - `/recipes/<id>/delete` は、そのレシピの詳細の画面。
/// - `/recipes/<id>/photos` と `/recipes/<id>/photos/<photo_id>/delete` は、そのレシピの編集の画面。
/// - `/workspace/...` は、「ワークスペース」の画面。
/// - ほか（`/recipes`・`/recipes/check`）と、ID の形が正しくないときは、一覧。
///
/// パスの文字を移り先にそのまま写さないよう、ID は形を確かめたものだけを使う。
fn page_for_post_path(path: &str) -> String {
    if path.starts_with("/workspace/") {
        return "/workspace".to_owned();
    }
    let recipe = path
        .strip_prefix("/recipes/")
        .and_then(|rest| rest.split_once('/'))
        .and_then(|(id, tail)| RecipeId::parse(id).map(|id| (id, tail)));
    match recipe {
        Some((id, "delete")) => format!("/recipes/{id}"),
        Some((id, _)) => edit_path(&id),
        None => "/".to_owned(),
    }
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
    /// 自分のワークスペースを見ているか（共有のデモでないか）。自動で消えることを一覧の下に書く。
    own_workspace: bool,
    /// 検索の条件の誤り。あれば一覧の代わりに出す（検索欄の htmx が入れ替える #results の中に出すため）。
    query_error: Option<&'static str>,
    q: String,
    total: i64,
    page: i64,
    /// レシピと、その先頭の写真（あれば一覧に小さく出す）。
    recipes: Vec<ListedRecipe>,
    /// 遅らせずに高い優先度で読む写真の、`recipes` の中の位置。写真のある最初のレシピ（最初の表示で一番大きな要素になりやすい）。
    priority_photo: Option<usize>,
    pages: Vec<PageLink>,
    first_page_href: String,
}

#[derive(Template)]
#[template(path = "detail.html")]
struct DetailPage {
    layout: Layout,
    recipe: Recipe,
    /// 写真は見せるだけで、追加・削除は編集の画面で行う。
    photos: Vec<Photo>,
    /// 写真を追加・削除できるか（共有のデモではできない）。できれば、編集の画面でできることを書く。
    photos_editable: bool,
}

/// 編集の画面の、写真の欄（`photo_section.html`）。写真は、追加・削除したときにすぐ保存する（料理名などの「保存する」とは別）。
struct PhotoSection {
    recipe_id: String,
    /// 写真の代替テキスト（「〇〇の写真 1」）に使う、保存してある料理名。
    recipe_name: String,
    photos: Vec<Photo>,
    /// 写真を追加・削除できるか（共有のデモではできない）。
    editable: bool,
    max_photos: i64,
    /// あと何枚追加できるか。0 なら追加のフォームを出さない。画面の JavaScript は、選んだ枚数が多すぎないかを送る前に確かめる。
    remaining: i64,
    /// 1 枚の写真の大きさの上限と、超えたときの説明。画面の JavaScript が、送る前に確かめる。
    max_photo_bytes: usize,
    photo_too_large: &'static str,
    /// 小さな版の長い辺と大きさの上限。画面の JavaScript が、この大きさに縮めて作る（[`crate::thumbnail`]）。
    thumbnail_edge: u16,
    thumbnail_bytes: usize,
}

/// フォームの画面の、写真にかかわる部分。
enum PhotoArea {
    /// 何も出さない（共有のデモの登録の画面と、欄から離れたときの確かめ直し）。
    None,
    /// 登録の画面。レシピの ID がまだ無いので写真を付けられず、保存した後に編集の画面で追加できることを書く。
    AfterSave,
    /// 編集の画面の、写真の欄。
    Section(PhotoSection),
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
    /// フォームの上の誤りの一覧に出す、すべての項目の誤り。欄から離れて確かめ直したときは空（[`Validation::Revalidated`]）。
    errors: Vec<FieldError>,
    name_error: Option<&'static str>,
    servings_error: Option<&'static str>,
    ingredients_error: Option<&'static str>,
    steps_error: Option<&'static str>,
    /// 送信して誤りが出た後か。後なら、欄から離れたときにも確かめ直す（reValidateMode: onBlur）。
    revalidate: bool,
    /// 誤りの一覧にフォーカスを移すか（送信したときだけ。欄から離れたときに移すと、入力の邪魔になる）。
    focus_summary: bool,
    photo_area: PhotoArea,
}

/// フォームを確かめた段階。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Validation {
    /// まだ送信していない。誤りは出さず、欄から離れても確かめない（mode: onSubmit）。
    NotSubmitted,
    /// 送信して誤りが出た。誤りの一覧にフォーカスを移し、以後は欄から離れたときにも確かめる。
    Submitted,
    /// 送信した後に、欄から離れて確かめ直した。誤りの一覧は出さず（欄の誤りだけを出す）、フォーカスも動かさない。
    /// 一覧を入れ替えると、押している途中の一覧のリンクが入れ替わり、クリックが失われるため。
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

/// 画面のテンプレート。どれも base.html を継いで、見出しの上下に [`Layout`] を出す。
trait Page: Template {
    fn layout(&self) -> &Layout;
}

macro_rules! impl_page {
    ($($page:ty),*) => {
        $(impl Page for $page {
            fn layout(&self) -> &Layout {
                &self.layout
            }
        })*
    };
}

impl_page!(
    ErrorPage,
    LandingPage,
    ListPage,
    DetailPage,
    FormPage,
    WorkspacePage
);

/// 応答の画面に [`Layout::gone`] を出したことの印。[`identify`] が、知らせを次の画面に回すかを決めるのに使う。
#[derive(Debug, Clone, Copy)]
struct GoneNoticeShown;

/// テンプレートを HTML にして返す。テンプレートの失敗は、中身を出さずに 500 にする。
fn render<T: Page>(status: StatusCode, template: &T) -> Response {
    match template.render() {
        Ok(html) => {
            let mut response = (status, Html(html)).into_response();
            if template.layout().gone.is_some() {
                response.extensions_mut().insert(GoneNoticeShown);
            }
            response
        }
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

/// 応答から、`name` の Cookie を付ける・消す `Set-Cookie` を取り除く。
fn remove_set_cookie(response: &mut Response, name: &str) {
    let kept: Vec<HeaderValue> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter(|value| !is_cookie_named(value, name))
        .cloned()
        .collect();
    response.headers_mut().remove(header::SET_COOKIE);
    for value in kept {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// `Set-Cookie` の値が、`name` の Cookie のものか。
fn is_cookie_named(value: &HeaderValue, name: &str) -> bool {
    value.to_str().is_ok_and(|text| {
        text.strip_prefix(name)
            .is_some_and(|rest| rest.starts_with('='))
    })
}

/// 応答が、`name` の Cookie を付けるか消すか（`Set-Cookie` にあるか）。
fn sets_cookie(response: &Response, name: &str) -> bool {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|value| is_cookie_named(value, name))
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
///   無くなっていれば（自動削除など）か、共有のデモの ID なら、どの画面でも Cookie を消し、そのことを一度だけ知らせる
///   （[`Flash::WorkspaceGone`]）。
///   知らせは、その応答の画面に出す。出せない応答（移動・エラーの画面・写真など）なら、
///   一度だけ出すメッセージの Cookie で、次の画面に回す（ハンドラーが付けたメッセージより優先する）。
///   Cookie を消した後は、ワークスペースが無いことが分からなくなり、知らせられないため。
///   htmx が一部だけを入れ替える要求（一覧の検索）なら、画面の全体を読み直させる（`HX-Refresh`）。
///   入れ替えた一覧だけがデモのレシピになり、自分のレシピ帳のように見えたまま、知らせも出ないのを避けるため。
///   回した知らせは、出せる画面まで残す。出せない応答では Cookie に触れない（並んで走る写真の要求などが付け直さないように）。
///   ハンドラーが同じ応答でワークスペースの Cookie を付けた・消したとき（作った・切り替えたときなど）は、
///   Cookie をそのままにし、知らせも出さない。後ろに削除を足すと新しい Cookie を打ち消し、同じ名前の `Set-Cookie` が 2 つ並ぶため。
/// - 一度だけ出すメッセージがあれば読み、画面を返すときに消す（移動のときは、移動先で読むので残す）。
pub async fn identify<B: Backend>(
    State(backend): State<B>,
    mut request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let flash = cookie_value(headers, FLASH_COOKIE).and_then(Flash::parse);
    let viewing_demo = cookie_value(headers, DEMO_VIEW_COOKIE) == Some("1");
    // Cookie の ID が共有のデモの ID なら（Cookie を手で書き換えたなど）、自分のワークスペースとして扱わず、Cookie を消す。
    // デモでは写真を追加・削除させないなど、デモかどうかで扱いが変わるので、どの画面でも同じに決めるため。
    // デモの ID かどうかが分からないよう、無い ID と同じく D1 を読み、同じく Cookie を消す（応答の時間とヘッダーをそろえる）。
    let demo = backend.demo_workspace_id();
    let cookie_id = cookie_workspace(headers);
    let (own, stale) = match cookie_id {
        Some(id) => match workspace::touch(&backend, &id).await {
            Ok(true) if demo.as_ref() == Some(&id) => (None, true),
            Ok(true) => (Some(id), false),
            Ok(false) => (None, true),
            Err(error) => return error_page(&error),
        },
        None => (None, false),
    };
    let gone_forwarded = flash == Some(Flash::WorkspaceGone);
    request.extensions_mut().insert(Session {
        own,
        viewing_demo,
        flash: flash.filter(|flash| *flash != Flash::WorkspaceGone),
        workspace_gone: stale || gone_forwarded,
    });
    // htmx が画面の一部だけを入れ替える要求（一覧の検索）。画面を返しても、見出しの下の知らせは入れ替わらない。
    // hx-boost の移動（HX-Boosted）と、戻るでの履歴の復元（HX-History-Restore-Request）は、body の全体を入れ替える。
    let partial = request.headers().contains_key("hx-request")
        && !request.headers().contains_key("hx-boosted")
        && !request.headers().contains_key("hx-history-restore-request");
    let mut response = next.run(request).await;
    // ハンドラーがワークスペースの Cookie を付けた・消したときは、新しいワークスペースを使い始めたので、知らせない。
    let switched = sets_cookie(&response, WORKSPACE_COOKIE);
    let gone_shown = !partial && response.extensions().get::<GoneNoticeShown>().is_some();
    if stale && !switched {
        for name in [WORKSPACE_COOKIE, DEMO_VIEW_COOKIE] {
            if !sets_cookie(&response, name) {
                remove_cookie(&mut response, name);
            }
        }
        if partial {
            response
                .headers_mut()
                .insert("hx-refresh", HeaderValue::from_static("true"));
        }
    }
    let forward_gone = !switched
        && !gone_shown
        // 回されてきた知らせは、ハンドラーがメッセージを付けたとき（移動）だけ付け直し、ほかは Cookie を残す。
        && (stale || (gone_forwarded && sets_cookie(&response, FLASH_COOKIE)));
    if forward_gone {
        // 出せなかった知らせを、次の画面に回す。ハンドラーが付けたメッセージの Cookie は、置き換える。
        remove_set_cookie(&mut response, FLASH_COOKIE);
        add_cookie(
            &mut response,
            FLASH_COOKIE,
            Flash::WorkspaceGone.code(),
            FLASH_MAX_AGE,
        );
    } else if flash.is_some()
        && !response.status().is_redirection()
        && (!gone_forwarded || gone_shown)
    {
        remove_cookie(&mut response, FLASH_COOKIE);
    }
    response
}

/// 画面を見ている人が使うワークスペースを決める。
///
/// 1. 自分のワークスペースがあり、一時的にデモを見ていなければ、それを使う。
/// 2. 共有のデモがあれば、それを使う。
/// 3. デモが無くても、自分のワークスペースがあれば、それを使う（デモを見ている途中でデモが無くなったとき）。
///
/// `session.own` は、デモの ID でないことを [`identify`] が確かめている。
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
                    own_workspace: matches!(viewer, Viewer::Own(_)),
                    query_error: Some(message),
                    first_page_href: "/".to_owned(),
                    q,
                    total: 0,
                    page: 1,
                    recipes: Vec::new(),
                    priority_photo: None,
                    pages: Vec::new(),
                },
            ));
        }
    };
    let result = recipe::list_page(&backend, viewer.id(), pattern.as_deref(), page, true).await?;
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
            own_workspace: matches!(viewer, Viewer::Own(_)),
            query_error: None,
            first_page_href: list_href(&q, 1),
            q,
            total: result.total,
            page,
            priority_photo: result
                .recipes
                .iter()
                .position(|listed| listed.first_photo_id.is_some()),
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
    photo_area: PhotoArea,
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
        errors: if validation == Validation::Revalidated {
            Vec::new()
        } else {
            errors
        },
        layout,
        revalidate: validation != Validation::NotSubmitted,
        focus_summary: validation == Validation::Submitted,
        photo_area,
    }
}

/// 写真を追加・削除できるか。共有のデモではできない（デモの ID の Cookie は、identify が自分のワークスペースから外す）。
/// 詳細・登録・編集の画面で同じ決まりにする。
fn photos_editable(viewer: &Viewer) -> bool {
    matches!(viewer, Viewer::Own(_))
}

/// 登録の画面の、写真にかかわる部分。共有のデモでは写真を付けられないので、何も書かない。
fn new_photo_area(viewer: &Viewer) -> PhotoArea {
    if photos_editable(viewer) {
        PhotoArea::AfterSave
    } else {
        PhotoArea::None
    }
}

/// 編集の画面の、写真の欄。
async fn photo_section<B: Backend>(
    backend: &B,
    viewer: &Viewer,
    recipe: &Recipe,
    id: &RecipeId,
) -> Result<PhotoSection, ScreenError> {
    let photos = backend
        .list_photos(viewer.id(), id)
        .await
        .map_err(ApiError::from)?;
    let count = i64::try_from(photos.len()).unwrap_or(i64::MAX);
    Ok(PhotoSection {
        recipe_id: recipe.id.clone(),
        recipe_name: recipe.name.clone(),
        photos,
        editable: photos_editable(viewer),
        max_photos: photo::MAX_PHOTOS_PER_RECIPE,
        remaining: (photo::MAX_PHOTOS_PER_RECIPE - count).max(0),
        max_photo_bytes: photo::MAX_PHOTO_BYTES,
        photo_too_large: PHOTO_TOO_LARGE,
        thumbnail_edge: crate::thumbnail::MAX_THUMBNAIL_EDGE,
        thumbnail_bytes: crate::thumbnail::MAX_THUMBNAIL_BYTES,
    })
}

/// 入力の誤りでフォームを出し直すときの、写真にかかわる部分。
/// 写真の欄を読めなくても（別のタブでレシピを消した・D1 の一時的な失敗）、写真の欄を出さずに入力の誤りを出す。
/// エラーの画面にすると、書きかけの入力が消えるためである。
async fn photo_area_or_none<B: Backend>(
    backend: &B,
    viewer: &Viewer,
    kind: &FormKind,
) -> PhotoArea {
    photo_area(backend, viewer, kind)
        .await
        .unwrap_or(PhotoArea::None)
}

/// フォームの画面の、写真にかかわる部分。編集なら、保存してあるレシピと写真を読む。
async fn photo_area<B: Backend>(
    backend: &B,
    viewer: &Viewer,
    kind: &FormKind,
) -> Result<PhotoArea, ScreenError> {
    match kind {
        FormKind::New => Ok(new_photo_area(viewer)),
        FormKind::Edit(id) => {
            let id = parse_id(id)?;
            let recipe = load(backend, viewer, &id).await?;
            Ok(PhotoArea::Section(
                photo_section(backend, viewer, &recipe, &id).await?,
            ))
        }
    }
}

/// フォームの本文を読めなかったときの失敗。本文が上限より大きければ API と同じく 413、それ以外は 400。
///
/// 400 の説明は、API の「JSON として読めない」ではなく、画面の言葉にする。
/// 画面のフォームからは起きず、作った要求（同じ欄を 2 つ送るなど）でしか届かない。
fn form_error(rejection: &FormRejection) -> ScreenError {
    match ApiError::from_body_status(rejection.status()) {
        ApiError::InvalidBody => ScreenError::Page(
            StatusCode::BAD_REQUEST,
            "フォームの内容を読めませんでした。画面を読み直してから、もう一度送ってください",
        ),
        error => ScreenError::Api(error),
    }
}

/// 保存できなかったときに、エラーの画面にせず、書きかけの入力を保ったフォームに理由を出す失敗。
///
/// レシピの数の上限・書き込みの回数の制限・サーバーの一時的な失敗（D1 など）は、少し後か、ほかのレシピを消せば、
/// 同じ入力で保存できる。エラーの画面にすると、最大 7,100 文字の入力が消えるためである。
/// 消されたレシピへの保存（`404`）は、送り直しても保存できないので、エラーの画面にする。
fn keeps_form(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::RecipeLimitReached | ApiError::TooManyRequests | ApiError::Internal
    )
}

/// 保存できなかった理由を、見出しの下に赤で出し、入力を保ったフォームを出し直す（[`keeps_form`]）。
async fn form_with_alert<B: Backend>(
    backend: &B,
    session: &Session,
    viewer: &Viewer,
    kind: &FormKind,
    form: &RecipeForm,
    error: &ApiError,
) -> Response {
    let (status, _, message) = error.parts();
    let area = photo_area_or_none(backend, viewer, kind).await;
    let page = form_page(
        Layout::new(Some(viewer), session).with_alert(message),
        kind,
        form,
        Vec::new(),
        Validation::NotSubmitted,
        area,
    );
    render(status, &page)
}

/// 登録と更新で共通の処理。入力の誤りと、[`keeps_form`] の失敗は、フォームに出し直す。
///
/// `limited` は、書き込みの回数の制限に当たったか（[`WriteLimited`]）。当たっていれば、入力を確かめる前に
/// （API と同じく、回数を先に確かめる）、保存せずに理由を出す。
async fn save<B: Backend>(
    backend: &B,
    session: &Session,
    viewer: &Viewer,
    kind: FormKind,
    form: &RecipeForm,
    limited: bool,
) -> Result<Response, ScreenError> {
    if limited {
        let error = ApiError::TooManyRequests;
        return Ok(form_with_alert(backend, session, viewer, &kind, form, &error).await);
    }
    let fields = match recipe::check(form.to_input()) {
        Ok(fields) => fields,
        Err(errors) => {
            let area = photo_area_or_none(backend, viewer, &kind).await;
            let layout = Layout::new(Some(viewer), session);
            return Ok(render(
                StatusCode::BAD_REQUEST,
                &form_page(layout, &kind, form, errors, Validation::Submitted, area),
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
        Err(error) if keeps_form(&error) => {
            Ok(form_with_alert(backend, session, viewer, &kind, form, &error).await)
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
        new_photo_area(&viewer),
    );
    Ok(render(StatusCode::OK, &page))
}

/// `POST /recipes`: レシピを登録し、詳細の画面へ移す。
pub async fn create<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    limited: Option<Extension<WriteLimited>>,
    form: Result<Form<RecipeForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let limited = limited.is_some();
    let form = read_recipe_form(form, limited)?;
    save(&backend, &session, &viewer, FormKind::New, &form, limited).await
}

/// 登録・更新のフォームの本文を読む。書き込みの回数の制限に当たっていれば（[`WriteLimited`]）、
/// 読めなかったとき（`Content-Length` の無い大きすぎる本文など）も、API と同じく回数の制限を先に答える。
fn read_recipe_form(
    form: Result<Form<RecipeForm>, FormRejection>,
    limited: bool,
) -> Result<RecipeForm, ScreenError> {
    match form {
        Ok(Form(form)) => Ok(form),
        Err(_) if limited => Err(ApiError::TooManyRequests.into()),
        Err(rejection) => Err(form_error(&rejection)),
    }
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
        PhotoArea::None,
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
    id: Result<Path<String>, PathRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let id = recipe::parse_recipe_id(id)?;
    detail_page(&backend, &viewer, &session, &id).await
}

/// レシピの詳細の画面。写真は見せるだけにする（追加・削除は編集の画面）。
async fn detail_page<B: Backend>(
    backend: &B,
    viewer: &Viewer,
    session: &Session,
    id: &RecipeId,
) -> Result<Response, ScreenError> {
    let recipe = load(backend, viewer, id).await?;
    let photos = backend
        .list_photos(viewer.id(), id)
        .await
        .map_err(ApiError::from)?;
    Ok(render(
        StatusCode::OK,
        &DetailPage {
            layout: Layout::new(Some(viewer), session),
            recipe,
            photos,
            photos_editable: photos_editable(viewer),
        },
    ))
}

/// 編集の画面。保存してある値をフォームに入れ、写真の欄を付ける。
/// `alert` があれば、見出しの下に誤りの知らせとして出す（写真の追加の失敗など）。
async fn edit_page<B: Backend>(
    backend: &B,
    viewer: &Viewer,
    session: &Session,
    id: &RecipeId,
    status: StatusCode,
    alert: Option<&'static str>,
) -> Result<Response, ScreenError> {
    let recipe = load(backend, viewer, id).await?;
    let section = photo_section(backend, viewer, &recipe, id).await?;
    let mut layout = Layout::new(Some(viewer), session);
    if let Some(alert) = alert {
        layout = layout.with_alert(alert);
    }
    let kind = FormKind::Edit(recipe.id.clone());
    let form = RecipeForm {
        servings: recipe.servings.to_string(),
        name: recipe.name,
        ingredients: recipe.ingredients,
        steps: recipe.steps,
    };
    let page = form_page(
        layout,
        &kind,
        &form,
        Vec::new(),
        Validation::NotSubmitted,
        PhotoArea::Section(section),
    );
    Ok(render(status, &page))
}

/// 編集の画面のパス。写真を追加・削除した後に戻る。
fn edit_path(id: &RecipeId) -> String {
    edit_path_of(&format!("/recipes/{id}"))
}

/// レシピのパス（`/recipes/<id>`）から、編集の画面のパスを作る。
fn edit_path_of(recipe_path: &str) -> String {
    format!("{recipe_path}/edit")
}

/// 写真が大きすぎるときの説明。[`photo::MAX_PHOTO_BYTES`] を変えたら、文も直す（テストで確かめる）。
pub(crate) const PHOTO_TOO_LARGE: &str = "写真は 1 枚 4,000,000 バイト（約 4 MB）までです";

/// 写真の操作の失敗のうち、編集の画面に知らせとして出すものの説明。ほかの失敗はエラーの画面にする。
fn photo_alert(error: &ApiError) -> Option<&'static str> {
    match error {
        ApiError::PayloadTooLarge => Some(PHOTO_TOO_LARGE),
        ApiError::InvalidBody => Some("写真を読めませんでした。もう一度選んで送ってください"),
        ApiError::UnsupportedPhotoType
        | ApiError::UnsupportedPhotoMetadata
        | ApiError::PhotoLimitReached
        | ApiError::PhotoStorageLimitReached
        | ApiError::PhotoNotFound
        | ApiError::DemoPhotosDisabled
        | ApiError::CapacityReached => Some(error.parts().2),
        _ => None,
    }
}

/// 写真の操作が失敗したときの応答。知らせにできる失敗は編集の画面に出し、ほかはエラーの画面にする。
async fn photo_failure<B: Backend>(
    backend: &B,
    viewer: &Viewer,
    session: &Session,
    id: &RecipeId,
    error: ApiError,
) -> Result<Response, ScreenError> {
    match photo_alert(&error) {
        Some(alert) => {
            let status = error.parts().0;
            edit_page(backend, viewer, session, id, status, Some(alert)).await
        }
        None => Err(error.into()),
    }
}

/// `GET /recipes/{id}/photos/{photo_id}`: 写真のデータ。
pub async fn photo_file<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    ids: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let (id, photo_id) = photo::parse_ids(ids)?;
    Ok(photo::photo_response(&backend, viewer.id(), &id, &photo_id).await?)
}

/// 写真の小さな版を足すときに、元の写真の幅を渡すクエリ（`?width=<ピクセル>`）。
#[derive(Deserialize)]
pub struct ThumbnailQuery {
    width: i64,
}

/// `GET /recipes/{id}/photos/{photo_id}/thumbnail`: 写真の小さな版のデータ。小さな版が無ければ `404`。
pub async fn thumbnail_file<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    ids: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let (id, photo_id) = photo::parse_ids(ids)?;
    Ok(photo::thumbnail_response(&backend, viewer.id(), &id, &photo_id).await?)
}

/// `POST /recipes/{id}/photos/{photo_id}/thumbnail?width=<ピクセル>`: 写真の小さな版（JPEG のバイト列そのもの）を足し、`204` を返す。
///
/// 画面の JavaScript が、写真を追加した後に、canvas で作って送る（[`photo::add_thumbnail`]）。
/// 失敗しても写真は残り、元の写真を出すので、JavaScript は失敗を知らせない。
pub async fn add_thumbnail<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    ids: Result<Path<(String, String)>, PathRejection>,
    query: Result<Query<ThumbnailQuery>, QueryRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let (id, photo_id) = photo::parse_ids(ids)?;
    // クエリと本文の誤りは、デモかを確かめた後に返す（[`photo::add_thumbnail`] の確かめる順）。
    let width = query.ok().map(|Query(query)| query.width);
    let body = body
        .map(Vec::from)
        .map_err(|rejection| ApiError::from_body_status(rejection.status()));
    photo::add_thumbnail(&backend, viewer.id(), &id, &photo_id, body, width).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// 写真のバイト列そのものを追加できたときに、追加した写真の ID を返すヘッダー（JavaScript が小さな版を足すのに使う）。
pub const PHOTO_ID_HEADER: &str = "x-photo-id";

/// `POST /recipes/{id}/photos`: 写真を追加する。
///
/// 本文は 2 つの形を受け付ける。
/// - フォーム（multipart）の `photo` の欄。JavaScript が動かなくても使える。追加したら、編集の画面へ戻る（`303`）。
/// - 写真のバイト列そのもの（JavaScript が 1 枚ずつ送る）。本番では本文を読みためずに R2 へ流す（[`stream`]）。
///   追加したら `204` を返す。JavaScript は、すべて送り終えてから写真の欄だけを読み直すので、1 枚ごとに移らせない。
pub async fn add_photo<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    id: Result<Path<String>, PathRejection>,
    raw: Option<Extension<RawPhotoBody>>,
    request: Request,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let id = recipe::parse_recipe_id(id)?;
    let from_script = raw.is_some() || !photo::has_multipart_body(request.headers());
    let body = match read_photo_body(raw, request).await {
        Ok(Some(body)) => body,
        Ok(None) => {
            return edit_page(
                &backend,
                &viewer,
                &session,
                &id,
                StatusCode::BAD_REQUEST,
                Some("追加する写真を選んでください"),
            )
            .await;
        }
        Err(error) => return photo_failure(&backend, &viewer, &session, &id, error).await,
    };
    match photo::add(&backend, viewer.id(), &id, body).await {
        // JavaScript が小さな版を足すのに使うので、写真の ID を返す。
        Ok(photo) if from_script => {
            Ok((StatusCode::NO_CONTENT, [(PHOTO_ID_HEADER, photo.id)]).into_response())
        }
        Ok(_) => Ok(see_other_with(&edit_path(&id), Flash::PhotoAdded)),
        Err(error) => photo_failure(&backend, &viewer, &session, &id, error).await,
    }
}

/// 写真を追加するリクエストの本文を読む。写真が無ければ `None`。
async fn read_photo_body(
    raw: Option<Extension<RawPhotoBody>>,
    request: Request,
) -> Result<Option<PhotoBody>, ApiError> {
    if raw.is_none() && photo::has_multipart_body(request.headers()) {
        let multipart = Multipart::from_request(request, &()).await;
        return Ok(read_photo_field(multipart).await?.map(PhotoBody::Bytes));
    }
    // 写真のバイト列そのもの。空の本文は、ファイルを選ばずに送ったときと同じに扱う。
    let body = photo::read_bytes_body(raw, request).await?;
    Ok((body.len() > 0).then_some(body))
}

/// フォームの `photo` の欄のデータを読む。ファイルが選ばれていなければ `None`。
async fn read_photo_field(
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Option<Vec<u8>>, ApiError> {
    let mut multipart = multipart.map_err(|_| ApiError::InvalidBody)?;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => return Ok(None),
            Err(error) => return Err(ApiError::from_body_status(error.status())),
        };
        if field.name() != Some("photo") {
            continue;
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|error| ApiError::from_body_status(error.status()))?;
        // ファイルを選ばずに送ると、中身の空の欄が届く。
        return Ok((!bytes.is_empty()).then(|| Vec::from(bytes)));
    }
}

/// `POST /recipes/{id}/photos/{photo_id}/delete`: 写真を消し、編集の画面へ戻る。
pub async fn delete_photo<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    ids: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let (id, photo_id) = photo::parse_ids(ids)?;
    match photo::remove(&backend, viewer.id(), &id, &photo_id).await {
        Ok(()) => Ok(see_other_with(&edit_path(&id), Flash::PhotoDeleted)),
        Err(error) => photo_failure(&backend, &viewer, &session, &id, error).await,
    }
}

/// `GET /recipes/{id}/edit`: 編集のフォーム。今の値を入れておく。
pub async fn edit_form<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    id: Result<Path<String>, PathRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let id = recipe::parse_recipe_id(id)?;
    edit_page(&backend, &viewer, &session, &id, StatusCode::OK, None).await
}

/// `POST /recipes/{id}`: レシピの中身を置き換え、詳細の画面へ移す。
pub async fn update<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    limited: Option<Extension<WriteLimited>>,
    id: Result<Path<String>, PathRejection>,
    form: Result<Form<RecipeForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let id = recipe::parse_recipe_id(id)?;
    let limited = limited.is_some();
    let form = read_recipe_form(form, limited)?;
    let kind = FormKind::Edit(id.to_string());
    save(&backend, &session, &viewer, kind, &form, limited).await
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
    id: Result<Path<String>, PathRejection>,
    form: Result<Form<DeleteForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let viewer = require_viewer(&backend, &session).await?;
    let id = recipe::parse_recipe_id(id)?;
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
    limited: Option<Extension<WriteLimited>>,
    headers: HeaderMap,
) -> Result<Response, ScreenError> {
    // 書き込みの回数の制限に当たっていれば、作らずに、作る回数の制限と同じく「はじめる」画面に理由を出す。
    let result = if limited.is_some() {
        Err(ApiError::TooManyRequests)
    } else {
        workspace::create_new(&backend, &headers).await
    };
    match result {
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
    limited: Option<Extension<WriteLimited>>,
    form: Result<Form<SwitchForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    // 書き込みの回数の制限に当たっていれば、切り替えずに、「ワークスペース」の画面に理由を出す。
    if limited.is_some() {
        let (status, _, message) = ApiError::TooManyRequests.parts();
        let mut page = workspace_view(&backend, &session, None).await?;
        page.layout = page.layout.with_alert(message);
        return Ok(render(status, &page));
    }
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    // 前後の空白は、コピーしたときに入りやすいので取り除く。
    let id = form.id.trim();
    let error = if id.is_empty() {
        "ワークスペースの ID を入力してください"
    } else {
        match WorkspaceId::parse(id) {
            // 共有のデモの ID は、自分のワークスペースとして使わせない（デモかどうかで扱いが変わるため）。
            // デモの ID だと分からないよう、無い ID と同じ説明にし、D1 も同じように読む（応答の時間をそろえる）。
            Some(id)
                if workspace::touch(&backend, &id).await?
                    && backend.demo_workspace_id().as_ref() != Some(&id) =>
            {
                return Ok(use_workspace(&id, Flash::WorkspaceSwitched));
            }
            _ => {
                "その ID のワークスペースは見つかりません。ID が正しいか、14 日アクセスが無くて消えていないかを確かめてください"
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

/// 削除のフォームの値。画面に出していたワークスペースの ID。
#[derive(Debug, Deserialize)]
pub struct DeleteWorkspaceForm {
    #[serde(default)]
    id: String,
}

/// `POST /workspace/delete`: 自分のワークスペースを、レシピと写真ごと消す。このブラウザの Cookie も消す。
///
/// 一時的にデモを見ている間も、消すのは自分のワークスペースである（画面に出している ID のもの）。
/// 画面を開いた後に別のタブで切り替えていると、Cookie の ID が画面の ID と違うので、消さずに理由を出す。
/// 元に戻せない操作なので、見ていなかったワークスペースを消さないようにする。
pub async fn delete_workspace<B: Backend>(
    State(backend): State<B>,
    Extension(session): Extension<Session>,
    form: Result<Form<DeleteWorkspaceForm>, FormRejection>,
) -> Result<Response, ScreenError> {
    let Form(form) = form.map_err(|rejection| form_error(&rejection))?;
    let Some(id) = &session.own else {
        return Ok(see_other("/workspace"));
    };
    if form.id != id.as_str() {
        let mut page = workspace_view(&backend, &session, None).await?;
        page.layout = page.layout.with_alert(
            "画面を開いた後に、使うワークスペースが変わりました。ID を確かめてから、もう一度削除してください",
        );
        return Ok(render(StatusCode::CONFLICT, &page));
    }
    match workspace::remove(&backend, id).await {
        // 同時に別のタブなどで消されていたときも、消えたことに変わりはないので、同じに扱う。
        Ok(()) | Err(ApiError::WorkspaceNotFound) => {}
        Err(error) => return Err(error.into()),
    }
    let mut response = see_other_with("/", Flash::WorkspaceDeleted);
    remove_cookie(&mut response, WORKSPACE_COOKIE);
    remove_cookie(&mut response, DEMO_VIEW_COOKIE);
    Ok(response)
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
/// 1. 別のサイトから送られたフォームでないか（`Origin` が自分のホストか）。無いか違えば（`null` も）`403`。
///    Cookie は SameSite=Lax なので、別のサイトからの POST には付かないが、念のため重ねて確かめる。
/// 2. API と同じ、書き込みの回数と本文の大きさ（[`check_write`]）。
///    回数の制限に当たっても、理由を出す画面が決まっているフォームは、断らずにハンドラーへ渡す（[`shows_write_limit_itself`]）。
pub async fn guard_writes<B: Backend>(
    State(backend): State<B>,
    request: Request,
    next: Next,
) -> Response {
    guard(&backend, request, next, recipe::MAX_BODY_BYTES).await
}

/// 写真を送るフォームの [`guard_writes`]。本文の上限だけが違う。
///
/// multipart のフォームは区切りの分を足した [`PHOTO_FORM_MAX_BYTES`]、写真のバイト列そのものは 1 枚の写真の上限にする。
pub async fn guard_photo_writes<B: Backend>(
    State(backend): State<B>,
    request: Request,
    next: Next,
) -> Response {
    let max_body_bytes = if photo::has_multipart_body(request.headers()) {
        PHOTO_FORM_MAX_BYTES
    } else {
        photo::MAX_PHOTO_BYTES
    };
    guard(&backend, request, next, max_body_bytes).await
}

/// 書き込みの回数の制限に当たっても、middleware では断らずに、印（[`WriteLimited`]）を付けてハンドラーに渡すパス。
///
/// - `POST /workspace`・`POST /workspace/switch`: 状態を変えずに、「はじめる」画面か「ワークスペース」の画面に理由を出す。
/// - `POST /recipes`・`POST /recipes/{id}`: 書きかけの入力を保ったフォームに理由を出す。
///
/// ほかの書き込み（削除・写真など）は、ここで断ってエラーの画面にする。
/// `{id}` は、形の正しいレシピの ID だけにする。ほかのパスを `/recipes/` の下に足したときに、印を見ないハンドラーへ
/// 渡して回数の制限を抜けさせないためである。ここに足すパスのハンドラーは、印を見て書き込まないこと（テストで確かめる）。
fn shows_write_limit_itself(path: &str) -> bool {
    matches!(path, "/workspace" | "/workspace/switch" | "/recipes")
        || path
            .strip_prefix("/recipes/")
            .is_some_and(|id| RecipeId::parse(id).is_some())
}

/// [`guard_writes`] の中身。`max_body_bytes` は本文の大きさの上限。
async fn guard<B: Backend>(
    backend: &B,
    mut request: Request,
    next: Next,
    max_body_bytes: usize,
) -> Response {
    let is_post = request.method() == Method::POST;
    let response = if is_post && !same_origin(request.headers()) {
        render(
            StatusCode::FORBIDDEN,
            &ErrorPage {
                layout: Layout::default()
                    .with_alert("ほかのサイトから送られたフォームは受け付けません"),
                edit_href: None,
            },
        )
    } else {
        let checked =
            check_write(backend, request.method(), request.headers(), max_body_bytes).await;
        match checked {
            Ok(()) => next.run(request).await,
            Err(ApiError::TooManyRequests)
                if shows_write_limit_itself(request.uri().path())
                    // 回数の制限に当たると、check_write は本文の大きさを確かめないので、ここで確かめる。
                    // 大きすぎる本文は、ハンドラーに読ませずに、回数の制限のエラーの画面で断る（API と同じく回数を先に答える）。
                    && check_body_size(request.headers(), max_body_bytes).is_ok() =>
            {
                request.extensions_mut().insert(WriteLimited);
                next.run(request).await
            }
            Err(error) => limit_error_page(&error, request.uri().path()),
        }
    };
    add_push_url(is_post, response)
}

/// [`guard`] が書き込みの前の確かめで断るときの画面。
fn limit_error_page(error: &ApiError, path: &str) -> Response {
    match (error, photo::photo_recipe_path(path)) {
        // 写真のフォームの本文が大きすぎるときは、本文を読まずに断る。スマートフォンの写真（5 MB 以上）で
        // よく起きるので、一般の説明ではなく写真の上限を出し、編集の画面に戻れるようにする。
        (ApiError::PayloadTooLarge, Some(recipe_path)) => render(
            StatusCode::PAYLOAD_TOO_LARGE,
            &ErrorPage {
                layout: Layout::default().with_alert(PHOTO_TOO_LARGE),
                edit_href: Some(edit_path_of(recipe_path)),
            },
        ),
        _ => error_page(error),
    }
}

/// POST に移動（303）ではなく画面（入力の誤りやエラー）で答えたときは、htmx にアドレスバーの URL を変えさせない。
/// POST のパス（/recipes など）が残ると、再読み込みで GET を送り、関係する画面へ移されて（[`method_not_allowed`]）、
/// 出していた画面が消えるため。
fn add_push_url(is_post: bool, mut response: Response) -> Response {
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
                edit_href: None,
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
///
/// ハンドラーがすでに付けたヘッダーは変えない。写真のデータは、ブラウザに残させる `Cache-Control` と、
/// 写真に合わせた CSP を自分で付けるため（[`photo::photo_response`]）。
pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    add_security_headers(&mut response);
    response
}

/// [`security_headers`] の中身。応答に無いヘッダーだけを足す。
fn add_security_headers(response: &mut Response) {
    for (name, value) in SECURITY_HEADERS {
        response
            .headers_mut()
            .entry(name)
            .or_insert(HeaderValue::from_static(value));
    }
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
