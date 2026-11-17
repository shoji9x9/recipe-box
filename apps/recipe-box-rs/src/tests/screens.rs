//! 画面（HTML）のテスト。Cookie でのワークスペースの受け取り、フォームの送信、送り元の確認を確かめる。

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};

use super::{
    TestResponse, WORKSPACE_A, WORKSPACE_B,
    fake::{FakeBackend, NOW},
    jpeg, jpeg_with_location, jpeg_with_orientation_only, send,
};
use crate::{
    recipe::{MAX_BODY_BYTES, MAX_RECIPES_PER_WORKSPACE},
    router,
};

pub(super) const HOST: &str = "recipe-box.example";
pub(super) const DEMO: &str = "dddddddddddddddddddddddddddddddd";

pub(super) fn backend_with_workspaces() -> FakeBackend {
    let backend = FakeBackend::default();
    backend.add_workspace(WORKSPACE_A, NOW);
    backend.add_workspace(WORKSPACE_B, NOW);
    backend
}

pub(super) fn cookie(workspace: &str) -> String {
    format!("__Host-workspace={workspace}")
}

/// 画面へのリクエストを作る。`cookie` があれば Cookie に、`form` があれば URL エンコードの本文にする。
/// POST には、同じホストからの送信として `Origin` を付ける。
fn screen_request(
    method: &Method,
    path: &str,
    cookie: Option<&str>,
    form: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header("cf-connecting-ip", "192.0.2.1");
    if *method == Method::POST {
        builder = builder.header(header::ORIGIN, format!("https://{HOST}"));
    }
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    match form {
        Some(form) => builder
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(form.to_owned()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

pub(super) async fn screen_get(
    backend: &FakeBackend,
    path: &str,
    cookie: Option<&str>,
) -> TestResponse {
    send(
        router(backend.clone()),
        screen_request(&Method::GET, path, cookie, None),
    )
    .await
}

pub(super) async fn screen_post(
    backend: &FakeBackend,
    path: &str,
    cookie: Option<&str>,
    form: &str,
) -> TestResponse {
    send(
        router(backend.clone()),
        screen_request(&Method::POST, path, cookie, Some(form)),
    )
    .await
}

/// 応答の Set-Cookie をすべて返す。
fn set_cookies(response: &TestResponse) -> Vec<String> {
    response
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect()
}

/// 応答の Set-Cookie のうち、名前が `name` のものを返す。
fn set_cookie_named(response: &TestResponse, name: &str) -> Option<String> {
    set_cookies(response)
        .into_iter()
        .find(|cookie| cookie.starts_with(&format!("{name}=")))
}

/// レシピを画面から登録し、詳細の画面のパスを返す。
pub(super) async fn create_recipe(backend: &FakeBackend, workspace: &str, name: &str) -> String {
    let form = format!("name={}&servings=2&ingredients=&steps=", urlencode(name));
    let response = screen_post(backend, "/recipes", Some(&cookie(workspace)), &form).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    response.location
}

fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn landing_is_shown_without_workspace_and_creates_nothing() {
    let backend = FakeBackend::default();
    let response = screen_get(&backend, "/", None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.content_type.starts_with("text/html"));
    assert!(response.body.contains("はじめる"), "{}", response.body);
    // GET では作らない（巡回などで全体の上限を使わせない）。
    assert!(backend.workspaces.lock().unwrap().is_empty());
    // 画面には CSP などのヘッダーが付く。
    let csp = response.headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.contains("script-src 'self'"), "{csp}");
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    // hx-boost の後のフォーカスと、Invoker Commands に対応していないブラウザのダイアログは、自分のドメインのファイルで補う。
    assert!(
        response
            .body
            .contains(r#"<script src="/assets/page.js" type="module"></script>"#)
    );
}

#[tokio::test]
async fn workspace_delete_button_is_named() {
    let backend = backend_with_workspaces();
    let response = screen_get(&backend, "/workspace", Some(&cookie(WORKSPACE_A))).await;
    assert!(
        response
            .body
            .contains(r#"command="show-modal">ワークスペースを削除する</button>"#),
        "{}",
        response.body
    );
    assert!(
        response
            .body
            .contains(r#"aria-describedby="delete-workspace-text""#)
    );
}

#[tokio::test]
async fn demo_is_shown_without_cookie_and_its_id_is_hidden() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..FakeBackend::default()
    };
    backend.add_workspace(DEMO, NOW);
    let path = create_recipe(&backend, DEMO, "デモのカレー").await;

    let response = screen_get(&backend, "/", None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.body.contains("共有のデモ"), "{}", response.body);
    assert!(response.body.contains("デモのカレー"));
    // デモの帯は、名前の付いた区画（ランドマーク）にし、読み上げで見つけられるようにする。
    assert!(
        response
            .body
            .contains(r#"<section class="demo-banner" aria-label="共有のデモ">"#)
    );
    // デモの ID は、一覧にもワークスペースの画面にも出さない。
    assert!(!response.body.contains(DEMO));
    let response = screen_get(&backend, "/workspace", None).await;
    assert!(!response.body.contains(DEMO));
    // デモでも詳細を見られる。
    let response = screen_get(&backend, &path, None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.body.contains("デモのカレー"));
}

#[tokio::test]
async fn demo_viewers_can_use_id_from_another_device() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..backend_with_workspaces()
    };
    backend.add_workspace(DEMO, NOW);

    // 「共有のデモ」（Cookie なし）と「共有のデモ（一時的）」の両方から、ほかの端末の ID に切り替えられる。
    let visiting = format!("{}; __Host-demo-view=1", cookie(WORKSPACE_A));
    for (state, cookies, notice) in [
        ("shared", None, "共有のデモを表示しています"),
        ("visiting", Some(visiting.as_str()), "自分のレシピ帳に戻る"),
    ] {
        let response = screen_get(&backend, "/workspace", cookies).await;
        assert_eq!(response.status, StatusCode::OK, "{state}");
        assert!(response.body.contains(notice), "{state}: {}", response.body);
        assert!(
            response.body.contains(r#"action="/workspace/switch""#),
            "{state}"
        );

        let response = screen_post(
            &backend,
            "/workspace/switch",
            cookies,
            &format!("id={WORKSPACE_B}"),
        )
        .await;
        assert_eq!(response.status, StatusCode::SEE_OTHER, "{state}");
        assert_eq!(response.location, "/", "{state}");
        assert!(
            set_cookie_named(&response, "__Host-workspace")
                .unwrap()
                .starts_with(&cookie(WORKSPACE_B)),
            "{state}"
        );
        // 一時的にデモを見ていた印も消す。
        assert!(
            set_cookie_named(&response, "__Host-demo-view")
                .unwrap()
                .contains("Max-Age=0"),
            "{state}"
        );

        // 切り替えた後は、デモの帯を出さない。
        let response = screen_get(&backend, "/", Some(&cookie(WORKSPACE_B))).await;
        assert_eq!(response.status, StatusCode::OK, "{state}");
        assert!(!response.body.contains("demo-banner"), "{state}");
    }
}

#[tokio::test]
async fn demo_that_does_not_exist_falls_back_to_landing() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..FakeBackend::default()
    };
    let response = screen_get(&backend, "/", None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.body.contains("はじめる"));
    assert!(!response.body.contains("共有のデモ"));
}

#[tokio::test]
async fn start_creates_workspace_and_sets_cookie() {
    let backend = FakeBackend::default();
    let response = screen_post(&backend, "/workspace", None, "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    assert_eq!(response.location, "/");
    let id = backend
        .workspaces
        .lock()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    assert_eq!(
        set_cookie_named(&response, "__Host-workspace").unwrap(),
        format!("__Host-workspace={id}; Path=/; Max-Age=34560000; Secure; HttpOnly; SameSite=Lax")
    );
    // 移動先に一度だけ出すメッセージを、短い寿命の Cookie で渡す。
    assert_eq!(
        set_cookie_named(&response, "__Host-flash").unwrap(),
        "__Host-flash=workspace-created; Path=/; Max-Age=60; Secure; HttpOnly; SameSite=Lax"
    );
    // その Cookie で一覧を開ける。
    let response = screen_get(&backend, "/", Some(&cookie(&id))).await;
    assert!(response.body.contains("レシピの一覧"), "{}", response.body);
    assert!(!response.body.contains("共有のデモ"));
}

#[tokio::test]
async fn start_shows_reason_when_limits_are_reached() {
    let backend = FakeBackend {
        rate_limited: true,
        ..FakeBackend::default()
    };
    let response = screen_post(&backend, "/workspace", None, "").await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        response.body.contains("しばらく待って"),
        "{}",
        response.body
    );
    assert_eq!(set_cookies(&response), Vec::<String>::new());
}

#[tokio::test]
async fn posts_from_other_sites_are_rejected() {
    let backend = backend_with_workspaces();
    for origin in [None, Some("https://evil.example"), Some("null")] {
        let mut builder = Request::post("/recipes")
            .header(header::HOST, HOST)
            .header(header::COOKIE, cookie(WORKSPACE_A))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        let request = builder.body(Body::from("name=x&servings=1")).unwrap();
        let response = send(router(backend.clone()), request).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN, "{origin:?}");
    }
    assert!(backend.recipes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn screen_writes_are_rate_limited() {
    let backend = FakeBackend {
        write_limited: true,
        ..backend_with_workspaces()
    };
    let response = screen_post(
        &backend,
        "/recipes",
        Some(&cookie(WORKSPACE_A)),
        "name=x&servings=1",
    )
    .await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(response.content_type.starts_with("text/html"));
    assert!(backend.recipes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pages_that_need_workspace_redirect_to_start() {
    let backend = FakeBackend::default();
    for path in ["/recipes/new", "/recipes/0123456789abcdef0123456789abcdef"] {
        let response = screen_get(&backend, path, None).await;
        assert_eq!(response.status, StatusCode::SEE_OTHER, "{path}");
        assert_eq!(response.location, "/");
    }
}

#[tokio::test]
async fn recipe_can_be_created_viewed_edited_and_deleted() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let response = screen_get(&backend, "/recipes/new", Some(&a)).await;
    assert_eq!(response.status, StatusCode::OK);

    // 改行は CRLF で送られても LF にそろえる。
    let response = screen_post(
        &backend,
        "/recipes",
        Some(&a),
        "name=%E3%82%AB%E3%83%AC%E3%83%BC&servings=4&ingredients=%E7%8E%89%E3%81%AD%E3%81%8E%0D%0A%E8%82%89&steps=%E7%85%AE%E3%82%8B",
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    let path = response.location.clone();
    let id = path.strip_prefix("/recipes/").unwrap().to_owned();
    let saved = backend.recipes.lock().unwrap()[&(WORKSPACE_A.to_owned(), id.clone())].clone();
    assert_eq!(saved.ingredients, "玉ねぎ\n肉");
    assert_eq!(saved.servings, 4);

    let response = screen_get(&backend, &path, Some(&a)).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.body.contains("カレー"));
    assert!(response.body.contains("4 人分"));

    let response = screen_get(&backend, &format!("{path}/edit"), Some(&a)).await;
    assert!(
        response.body.contains(r#"value="カレー""#),
        "{}",
        response.body
    );
    let response = screen_post(
        &backend,
        &path,
        Some(&a),
        "name=%E3%82%B7%E3%83%81%E3%83%A5%E3%83%BC&servings=2",
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    assert_eq!(response.location, path);
    let saved = backend.recipes.lock().unwrap()[&(WORKSPACE_A.to_owned(), id.clone())].clone();
    assert_eq!(saved.name, "シチュー");
    assert_eq!(saved.ingredients, "");

    let response = screen_post(&backend, &format!("{path}/delete"), Some(&a), "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location, "/");
    assert!(backend.recipes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn names_are_escaped() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, r#"<script>alert("x")</script>"#).await;
    for page in ["/", path.as_str()] {
        let response = screen_get(&backend, page, Some(&a)).await;
        assert!(!response.body.contains("<script>alert"), "{page}");
        // askama は < を &#60; に、" を &#34; にする。
        assert!(response.body.contains("&#60;script&#62;alert"), "{page}");
        // 削除のボタンの名前（属性の値）でも、エスケープする。
        assert!(
            response.body.contains(
                r#"aria-label="「&#60;script&#62;alert(&#34;x&#34;)&#60;/script&#62;」を削除""#
            ),
            "{page}"
        );
    }
}

#[tokio::test]
async fn invalid_input_is_shown_on_the_form_with_values_kept() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    for (form, field) in [
        ("name=&servings=2&steps=%E7%85%AE%E3%82%8B", "name"),
        ("name=x&servings=abc", "servings"),
        ("name=x&servings=", "servings"),
        ("name=x&servings=2.5", "servings"),
        ("name=x&servings=101", "servings"),
    ] {
        let response = screen_post(&backend, "/recipes", Some(&a), form).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{form}");
        assert!(response.body.contains(r#"role="alert""#), "{form}");
        assert!(
            response
                .body
                .contains(&format!(r#"id="{field}" name="{field}""#)),
            "{form}"
        );
        assert!(response.body.contains(r#"aria-invalid="true""#), "{form}");
    }
    // 入力した値は消さない。
    let response = screen_post(
        &backend,
        "/recipes",
        Some(&a),
        "name=&servings=3&steps=%E7%85%AE%E3%82%8B",
    )
    .await;
    assert!(response.body.contains(r#"value="3""#));
    assert!(response.body.contains(">煮る</textarea>"));
    assert!(backend.recipes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn recipe_limit_is_shown_on_the_form() {
    let backend = backend_with_workspaces();
    for n in 0..MAX_RECIPES_PER_WORKSPACE {
        create_recipe(&backend, WORKSPACE_A, &format!("料理 {n}")).await;
    }
    let response = screen_post(
        &backend,
        "/recipes",
        Some(&cookie(WORKSPACE_A)),
        "name=x&servings=1",
    )
    .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert!(response.body.contains("上限"), "{}", response.body);
}

#[tokio::test]
async fn list_pages_keep_the_search() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    for n in 1..=25 {
        create_recipe(&backend, WORKSPACE_A, &format!("Curry {n}")).await;
        backend.advance(1);
    }
    let response = screen_get(&backend, "/?q=curry", Some(&a)).await;
    assert!(response.body.contains("25 件"), "{}", response.body);
    assert!(
        response.body.contains(r#"href="/?q=curry&#38;page=2""#),
        "{}",
        response.body
    );
    let response = screen_get(&backend, "/?q=curry&page=2", Some(&a)).await;
    assert!(response.body.contains("Curry 1<"), "{}", response.body);
    assert!(response.body.contains(r#"aria-current="page">2<"#));
    // 検索の条件が正しくなければ、API と同じく 400 にし、誤りを一覧の場所（#results の中）に出す。
    // 入力しながらの検索は #results だけを入れ替えるので、そこに誤りが無いと一覧が黙って空になる。
    for (path, message) in [
        (
            "/?page=5".to_owned(),
            "ページの番号は 1 から 4 までの整数です",
        ),
        (
            format!("/?q={}", "a".repeat(101)),
            "検索する料理名は 100 文字までです",
        ),
    ] {
        let response = screen_get(&backend, &path, Some(&a)).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{path}");
        let results = response
            .body
            .split(r#"<section id="results">"#)
            .nth(1)
            .unwrap();
        assert!(
            results.contains(&format!(
                r#"<p class="message error" role="alert">{message}</p>"#
            )),
            "{path}: {results}"
        );
        // 件数は出さない（件数の読み上げの入れ物は、空のまま残す）。
        assert!(
            response
                .body
                .contains(r#"<p id="results-count" class="count" role="status"></p>"#),
            "{path}"
        );
    }
    // 入力した検索の文字列は、検索欄に残す。
    let response = screen_get(&backend, &format!("/?q={}", "a".repeat(101)), Some(&a)).await;
    assert!(
        response
            .body
            .contains(&format!(r#"value="{}""#, "a".repeat(101)))
    );
}

#[tokio::test]
async fn delete_from_list_returns_to_the_same_search() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "Curry").await;
    let response = screen_post(
        &backend,
        &format!("{path}/delete"),
        Some(&a),
        "q=cur&page=1",
    )
    .await;
    assert_eq!(response.location, "/?q=cur");
    // 戻り先の値が正しくなければ、一覧の最初に戻す。
    let path = create_recipe(&backend, WORKSPACE_A, "Curry").await;
    let response = screen_post(
        &backend,
        &format!("{path}/delete"),
        Some(&a),
        "q=cur&page=https%3A%2F%2Fevil.example",
    )
    .await;
    assert_eq!(response.location, "/");
}

#[tokio::test]
async fn other_workspace_cannot_see_or_change_recipe() {
    let backend = backend_with_workspaces();
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let b = cookie(WORKSPACE_B);
    for response in [
        screen_get(&backend, &path, Some(&b)).await,
        screen_get(&backend, &format!("{path}/edit"), Some(&b)).await,
        screen_post(&backend, &path, Some(&b), "name=x&servings=1").await,
        screen_post(&backend, &format!("{path}/delete"), Some(&b), "").await,
    ] {
        assert_eq!(response.status, StatusCode::NOT_FOUND);
    }
    assert_eq!(backend.recipes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn stale_cookie_is_cleared() {
    let backend = FakeBackend::default();
    let response = screen_get(&backend, "/", Some(&cookie(WORKSPACE_A))).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.body.contains("はじめる"));
    assert_eq!(
        set_cookie_named(&response, "__Host-workspace").unwrap(),
        "__Host-workspace=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax"
    );
    // 黙って「はじめる」画面にせず、覚えていたワークスペースが無いことを、誤りの知らせとして出す。
    assert!(response.body.contains(&gone_alert()), "{}", response.body);
    // この画面で出したので、次の画面に回さない。
    assert_eq!(set_cookie_named(&response, "__Host-flash"), None);
}

#[tokio::test]
async fn stale_cookie_is_cleared_on_every_screen() {
    // 共有のデモがあっても、消えたワークスペースの Cookie を持つ人を、デモに書き込ませたままにしない。
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..FakeBackend::default()
    };
    backend.add_workspace(DEMO, NOW);
    let stale = cookie(WORKSPACE_A);
    for response in [
        screen_get(&backend, "/recipes/new", Some(&stale)).await,
        screen_get(&backend, "/workspace", Some(&stale)).await,
        screen_post(&backend, "/recipes", Some(&stale), "name=x&servings=1").await,
    ] {
        assert!(
            set_cookie_named(&response, "__Host-workspace")
                .is_some_and(|cookie| cookie.contains("Max-Age=0")),
            "{:?}",
            set_cookies(&response)
        );
    }
}

#[tokio::test]
async fn workspace_page_shows_own_id_and_switches() {
    let backend = backend_with_workspaces();
    let response = screen_get(&backend, "/workspace", Some(&cookie(WORKSPACE_A))).await;
    assert!(response.body.contains(WORKSPACE_A));

    // 前後の空白は取り除いて受け付ける。
    let response = screen_post(
        &backend,
        "/workspace/switch",
        Some(&cookie(WORKSPACE_A)),
        &format!("id=+{WORKSPACE_B}+"),
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    assert!(
        set_cookie_named(&response, "__Host-workspace")
            .unwrap()
            .starts_with(&cookie(WORKSPACE_B))
    );

    // 無い ID と形の正しくない ID は、理由を出して切り替えない。
    for id in ["cccccccccccccccccccccccccccccccc", "xyz"] {
        let response = screen_post(&backend, "/workspace/switch", None, &format!("id={id}")).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND);
        assert!(response.body.contains("見つかりません"));
        assert_eq!(set_cookies(&response), Vec::<String>::new());
    }

    let response = screen_get(&backend, "/workspace", Some(&cookie(WORKSPACE_A))).await;
    assert!(response.body.contains(r#"action="/workspace/delete""#));
    // 削除のフォームは、画面に出したワークスペースの ID を送る。
    assert!(response.body.contains(&format!(
        r#"<input type="hidden" name="id" value="{WORKSPACE_A}">"#
    )));

    // 画面を開いた後に、別のタブで B に切り替えていた。A の画面から送っても、B を消さない。
    let response = screen_post(
        &backend,
        "/workspace/delete",
        Some(&cookie(WORKSPACE_B)),
        &format!("id={WORKSPACE_A}"),
    )
    .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert!(response.body.contains("使うワークスペースが変わりました"));
    assert!(backend.workspaces.lock().unwrap().contains_key(WORKSPACE_B));

    let response = screen_post(
        &backend,
        "/workspace/delete",
        Some(&cookie(WORKSPACE_A)),
        &format!("id={WORKSPACE_A}"),
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    for name in ["__Host-workspace", "__Host-demo-view"] {
        assert!(
            set_cookie_named(&response, name)
                .unwrap()
                .contains("Max-Age=0"),
            "{name}"
        );
    }
    // Cookie だけでなく、ワークスペースそのものを消す。
    assert!(!backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));

    // 自分のワークスペースが無ければ、何も消さずにワークスペースの画面へ戻す。
    let response = screen_post(
        &backend,
        "/workspace/delete",
        None,
        &format!("id={WORKSPACE_B}"),
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert!(backend.workspaces.lock().unwrap().contains_key(WORKSPACE_B));
}

#[tokio::test]
async fn htmx_is_configured_safely() {
    let backend = backend_with_workspaces();
    let response = screen_get(&backend, "/", Some(&cookie(WORKSPACE_A))).await;
    assert!(
        response
            .body
            .contains(r#"<script src="/assets/htmx.min.js" defer></script>"#)
    );
    assert!(response.body.contains(r#""allowEval":false"#));
    assert!(response.body.contains(r#""includeIndicatorStyles":false"#));
    assert!(response.body.contains(r#"hx-boost="true""#));
}

#[tokio::test]
async fn post_answered_with_a_page_keeps_the_url() {
    let backend = backend_with_workspaces();
    // 入力の誤りでフォームを出し直すときは、アドレスバーの URL を変えさせない。
    let response = screen_post(
        &backend,
        "/recipes",
        Some(&cookie(WORKSPACE_A)),
        "name=&servings=1",
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.headers["hx-push-url"], "false");
    // 移動（303）のときは付けない（htmx が移動先の URL にする）。
    let response = screen_post(
        &backend,
        "/recipes",
        Some(&cookie(WORKSPACE_A)),
        "name=x&servings=1",
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert!(!response.headers.contains_key("hx-push-url"));
    // GET には付けない。
    let response = screen_get(&backend, "/", Some(&cookie(WORKSPACE_A))).await;
    assert!(!response.headers.contains_key("hx-push-url"));
}

#[tokio::test]
async fn flash_message_is_shown_once() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let response = screen_post(&backend, "/recipes", Some(&a), "name=x&servings=1").await;
    let flash = set_cookie_named(&response, "__Host-flash").unwrap();
    let flash = flash.split(';').next().unwrap();
    // 移動先で出し、Cookie を消す。
    let with_flash = format!("{a}; {flash}");
    let response = screen_get(&backend, &response.location, Some(&with_flash)).await;
    assert!(
        response
            .body
            .contains(r#"<div id="messages-status" role="status"><p class="message success">レシピを登録しました</p></div>"#),
        "{}",
        response.body
    );
    assert!(
        set_cookie_named(&response, "__Host-flash")
            .is_some_and(|cookie| cookie.contains("Max-Age=0"))
    );
    // 知らない種類のコードは出さない（文は Cookie に入れない）。
    // 知らせの入れ物は、読み上げのために、知らせが無くても置く。
    let response = screen_get(&backend, "/", Some(&format!("{a}; __Host-flash=<script>"))).await;
    assert!(!response.body.contains("message success"));
    assert!(
        response
            .body
            .contains(r#"<div id="messages-status" role="status"></div>"#)
    );
    assert!(
        response
            .body
            .contains(r#"<div id="messages-alert" role="alert"></div>"#)
    );
}

#[tokio::test]
async fn all_field_errors_are_shown_on_the_form() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let long_name = "🍛".repeat(101);
    let form = format!(
        "name={}&servings=0&ingredients={}&steps={}",
        urlencode(&long_name),
        "a".repeat(2_001),
        "b".repeat(5_001)
    );
    let response = screen_post(&backend, "/recipes", Some(&a), &form).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    // まとめた一覧に 4 件、各欄の下にも 1 件ずつ出す。
    assert!(
        response.body.contains("入力に誤りがあります（4 件）"),
        "{}",
        response.body
    );
    for (field, message) in [
        ("name", "料理名は 100 文字までです"),
        (
            "servings",
            "何人分かは 1 から 100 までの整数で入力してください",
        ),
        ("ingredients", "材料は 2,000 文字までです"),
        ("steps", "手順は 5,000 文字までです"),
    ] {
        assert!(
            response
                .body
                .contains(&format!(r##"<a href="#{field}">{message}</a>"##)),
            "{field}"
        );
        assert!(
            response.body.contains(&format!(
                r#"<p id="{field}-error" class="field-error">{message}</p>"#
            )),
            "{field}"
        );
        assert!(
            response
                .body
                .contains(&format!(r#"aria-describedby="{field}-error""#)),
            "{field}"
        );
    }
    // ブラウザの標準の検証は使わない（maxlength は UTF-16 で数え、サーバーとずれるため）。
    assert!(response.body.contains("novalidate"));
    assert!(!response.body.contains("maxlength"));
    assert!(!response.body.contains("required"));
    // 100 文字ちょうどの絵文字の料理名は受け付ける（コードポイントで数える）。
    let form = format!("name={}&servings=1", urlencode(&"🍛".repeat(100)));
    let response = screen_post(&backend, "/recipes", Some(&a), &form).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
}

#[tokio::test]
async fn too_large_form_body_is_rejected() {
    let backend = backend_with_workspaces();
    let body = format!("name=x&servings=1&steps={}", "a".repeat(MAX_BODY_BYTES));
    let request = |content_length: Option<usize>| {
        let mut builder = Request::post("/recipes")
            .header(header::HOST, HOST)
            .header(header::ORIGIN, format!("https://{HOST}"))
            .header(header::COOKIE, cookie(WORKSPACE_A))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(length) = content_length {
            builder = builder.header(header::CONTENT_LENGTH, length);
        }
        builder.body(Body::from(body.clone())).unwrap()
    };
    // Content-Length があれば、本文を読む前に断る。
    // Content-Length の無い本文（chunked）も、読む途中で上限で止める。
    for content_length in [Some(body.len()), None] {
        let response = send(router(backend.clone()), request(content_length)).await;
        assert_eq!(
            response.status,
            StatusCode::PAYLOAD_TOO_LARGE,
            "{content_length:?}: {}",
            response.body
        );
    }
    assert!(backend.recipes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn own_workspace_can_visit_demo_and_come_back() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..backend_with_workspaces()
    };
    backend.add_workspace(DEMO, NOW);
    let a = cookie(WORKSPACE_A);
    let response = screen_get(&backend, "/workspace", Some(&a)).await;
    assert!(response.body.contains("共有のデモを見る"));

    // デモを見る印は 1 時間で切れる Cookie。自分の ID の Cookie は残す。
    let response = screen_post(&backend, "/workspace/demo", Some(&a), "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(
        set_cookie_named(&response, "__Host-demo-view").unwrap(),
        "__Host-demo-view=1; Path=/; Max-Age=3600; Secure; HttpOnly; SameSite=Lax"
    );
    assert_eq!(set_cookie_named(&response, "__Host-workspace"), None);

    // デモを見ている間は、どの画面にも「自分のレシピ帳に戻る」を出し、登録はデモに入る。
    let visiting = format!("{a}; __Host-demo-view=1");
    for path in ["/", "/recipes/new", "/workspace"] {
        let response = screen_get(&backend, path, Some(&visiting)).await;
        assert!(response.body.contains("自分のレシピ帳に戻る"), "{path}");
        assert!(!response.body.contains(DEMO), "{path}");
    }
    let path = create_recipe(&backend, DEMO, "デモのカレー").await;
    let response = screen_get(&backend, &path, Some(&visiting)).await;
    assert_eq!(response.status, StatusCode::OK);
    // ワークスペースの画面では、デモを見ている間も自分の ID を出す。
    let response = screen_get(&backend, "/workspace", Some(&visiting)).await;
    assert!(response.body.contains(WORKSPACE_A));

    // 戻ると、印を消す。
    let response = screen_post(&backend, "/workspace/own", Some(&visiting), "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert!(
        set_cookie_named(&response, "__Host-demo-view")
            .unwrap()
            .contains("Max-Age=0")
    );
    let response = screen_get(&backend, &path, Some(&a)).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn demo_view_needs_own_workspace_and_demo() {
    // 自分のワークスペースが無い人と、デモの無い環境では、印を付けない。
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..backend_with_workspaces()
    };
    let response = screen_post(&backend, "/workspace/demo", None, "").await;
    assert_eq!(set_cookie_named(&response, "__Host-demo-view"), None);
    let backend = backend_with_workspaces();
    let response = screen_post(&backend, "/workspace/demo", Some(&cookie(WORKSPACE_A)), "").await;
    assert_eq!(set_cookie_named(&response, "__Host-demo-view"), None);
    // デモが無ければ、印があっても自分のワークスペースを使う。
    let response = screen_get(
        &backend,
        "/",
        Some(&format!("{}; __Host-demo-view=1", cookie(WORKSPACE_A))),
    )
    .await;
    assert!(!response.body.contains("共有のデモ"));
}

#[tokio::test]
async fn delete_asks_with_a_dialog_without_javascript() {
    let backend = backend_with_workspaces();
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let id = path.strip_prefix("/recipes/").unwrap();
    for page in ["/", path.as_str()] {
        let response = screen_get(&backend, page, Some(&cookie(WORKSPACE_A))).await;
        assert!(
            response
                .body
                .contains(&format!(r#"commandfor="delete-{id}" command="show-modal""#)),
            "{page}"
        );
        assert!(
            response
                .body
                .contains(&format!(r#"<dialog id="delete-{id}""#))
        );
        // 一覧では「削除」のボタンが並ぶので、何を消すかを名前（aria-label）に含める。
        assert!(
            response
                .body
                .contains(r#"aria-label="「カレー」を削除">削除</button>"#),
            "{page}"
        );
        // ダイアログの説明を、開いたときに見出しと一緒に読み上げる。
        assert!(response.body.contains(&format!(
            r#"aria-labelledby="delete-{id}-title" aria-describedby="delete-{id}-text""#
        )));
        assert!(response.body.contains(&format!(
            r#"<p id="delete-{id}-text">「カレー」を削除します。"#
        )));
        assert!(response.body.contains("元には戻せません"));
        assert!(!response.body.contains("hx-confirm"));
    }
    // 一覧から消すときは、戻る先の検索の条件とページを一緒に送る。詳細からは送らない。
    let hidden_q = r#"<input type="hidden" name="q" value="カ">"#;
    let hidden_page = r#"<input type="hidden" name="page" value="1">"#;
    let response = screen_get(&backend, "/?q=%E3%82%AB", Some(&cookie(WORKSPACE_A))).await;
    assert!(response.body.contains(hidden_q), "{}", response.body);
    assert!(response.body.contains(hidden_page));
    let response = screen_get(&backend, &path, Some(&cookie(WORKSPACE_A))).await;
    assert!(!response.body.contains(r#"name="q""#));
    assert!(!response.body.contains(r#"name="page""#));
}

#[tokio::test]
async fn empty_switch_id_is_asked_for() {
    let backend = backend_with_workspaces();
    let response = screen_post(&backend, "/workspace/switch", None, "id=").await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(
        response
            .body
            .contains("ワークスペースの ID を入力してください")
    );
    assert!(
        response
            .body
            .contains(r#"aria-describedby="workspace-id-error""#)
    );
}

#[tokio::test]
async fn fields_are_checked_on_submit_then_on_blur() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    // 送信する前は、欄から離れても確かめない（mode: onSubmit）。
    let response = screen_get(&backend, "/recipes/new", Some(&a)).await;
    assert!(!response.body.contains("hx-post"));
    // 送信して誤りが出た後は、どの欄も、離れたときに確かめ直す（reValidateMode: onBlur）。
    // 確かめるだけのルートは登録の画面の形で返すので、そのタイトルは使わない（編集の画面のタイトルを残す。ignoreTitle）。
    let response = screen_post(&backend, "/recipes", Some(&a), "name=&servings=2").await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    // 誤りの一覧は送信したときに出す。
    assert!(response.body.contains(r#"class="error-summary""#));
    for field in ["name", "servings", "ingredients", "steps"] {
        assert!(
            response.body.contains(&format!(
                r##"hx-post="/recipes/check" hx-trigger="blur" hx-include="closest form" hx-target="#field-{field}" hx-select="#field-{field}" hx-swap="outerHTML ignoreTitle:true">"##
            )),
            "{field}"
        );
    }
    // 送信したときは、誤りの一覧にフォーカスを移す。
    assert!(response.body.contains(r#"tabindex="-1" autofocus"#));
}

#[tokio::test]
async fn check_route_only_validates() {
    let backend = backend_with_workspaces();
    // ワークスペースの Cookie が無くても確かめられ、何も保存しない。
    let response = screen_post(&backend, "/recipes/check", None, "name=&servings=2").await;
    // 確かめること自体は成功なので 200。誤りは欄のすぐ下に出す（画面は欄だけを入れ替える）。
    assert_eq!(response.status, StatusCode::OK);
    // 誤りの一覧は、欄から離れたときには出さない（入れ替えると、押している途中の一覧のリンクのクリックが失われる）。
    assert!(
        !response.body.contains(r#"class="error-summary""#),
        "{}",
        response.body
    );
    assert!(
        response
            .body
            .contains(r#"<p id="name-error" class="field-error">料理名を入力してください</p>"#)
    );
    // 欄から離れたときに確かめ直すので、フォーカスは動かさない（どこにも autofocus を付けない）。続けて確かめられるよう、属性は残す。
    assert!(!response.body.contains("autofocus"));
    assert!(response.body.contains(r#"hx-post="/recipes/check""#));
    // 誤りが複数あっても、一覧は出さない。各欄の誤りは出す。
    let many = format!("name=&servings=0&ingredients={}", "a".repeat(2001));
    let response = screen_post(&backend, "/recipes/check", None, &many).await;
    assert!(!response.body.contains(r#"class="error-summary""#));
    assert_eq!(response.body.matches(r#"class="field-error""#).count(), 3);
    // 直った欄は、誤りが消える。
    let response = screen_post(&backend, "/recipes/check", None, "name=x&servings=2").await;
    assert!(!response.body.contains("field-error"));
    assert!(backend.recipes.lock().unwrap().is_empty());
    // ほかの画面の POST と同じく、別のサイトからは受け付けない。
    let request = Request::post("/recipes/check")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, "https://evil.example")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("name=x"))
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn check_route_does_not_use_the_write_limit() {
    // 書き込みの回数の制限に当たっていても、確かめるだけのルートは使える。
    let backend = FakeBackend {
        write_limited: true,
        ..backend_with_workspaces()
    };
    for _ in 0..40 {
        let response = screen_post(&backend, "/recipes/check", None, "name=&servings=2").await;
        assert_eq!(response.status, StatusCode::OK);
    }
    // 回数の制限を数えない（保存の POST の枠を食わない）。
    assert_eq!(backend.write_limit_keys.lock().unwrap().len(), 0);
    // 本文の大きさの上限は、ほかの画面の POST と同じく受ける。
    let body = format!("name=x&steps={}", "a".repeat(MAX_BODY_BYTES));
    let request = Request::post("/recipes/check")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, format!("https://{HOST}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
}

/// 写真を送るフォーム（multipart/form-data）の本文。`photo` が `None` なら、ファイルを選ばずに送ったときの空の欄にする。
fn photo_form(photo: Option<&[u8]>) -> (String, Vec<u8>) {
    let boundary = "----recipe-box-test-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n",
            if photo.is_some() { "dish.jpg" } else { "" }
        )
        .as_bytes(),
    );
    body.extend_from_slice(photo.unwrap_or_default());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

pub(super) async fn post_photo(
    backend: &FakeBackend,
    recipe_path: &str,
    cookie: Option<&str>,
    photo: Option<&[u8]>,
) -> TestResponse {
    let (content_type, body) = photo_form(photo);
    let mut builder = Request::post(format!("{recipe_path}/photos"))
        .header(header::HOST, HOST)
        .header(header::ORIGIN, format!("https://{HOST}"))
        .header("cf-connecting-ip", "192.0.2.1")
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, body.len());
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(
        router(backend.clone()),
        builder.body(Body::from(body)).unwrap(),
    )
    .await
}

/// 写真のバイト列をそのまま本文にして送る（JavaScript が 1 枚ずつ送るときの形）。
pub(super) async fn post_raw_photo(
    backend: &FakeBackend,
    recipe_path: &str,
    cookie: &str,
    content_type: &str,
    photo: Vec<u8>,
) -> TestResponse {
    let request = Request::post(format!("{recipe_path}/photos"))
        .header(header::HOST, HOST)
        .header(header::ORIGIN, format!("https://{HOST}"))
        .header("cf-connecting-ip", "192.0.2.1")
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, photo.len())
        .body(Body::from(photo))
        .unwrap();
    send(router(backend.clone()), request).await
}

#[tokio::test]
async fn photo_bytes_can_be_sent_without_multipart() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;

    // JavaScript が送るので、詳細の画面に移らせず、知らせ（flash）も付けない。
    let response = post_raw_photo(&backend, &path, &a, "image/jpeg", jpeg(10)).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT, "{}", response.body);
    assert_eq!(response.location, "");
    assert!(set_cookie_named(&response, "__Host-flash").is_none());
    {
        let stored = backend.objects.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored.values().next().unwrap().0, jpeg(10));
    }

    // 形式は中身で決めるので、宣言が画像でも中身が違えば断る。
    let response =
        post_raw_photo(&backend, &path, &a, "image/jpeg", b"not an image".to_vec()).await;
    assert_eq!(response.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    // 空の本文は、ファイルを選ばずに送ったときと同じに扱う。
    let response = post_raw_photo(&backend, &path, &a, "image/jpeg", Vec::new()).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(response.body.contains("追加する写真を選んでください"));
    // 画面の本文の上限（multipart の区切りの分だけ大きい）より小さくても、写真の上限を超えれば、本文を読む前に断る。
    // 本文は空にして `Content-Length` だけを大きくする。本文を読めば「写真を選んでください」の 400 になる。
    let request = Request::post(format!("{path}/photos"))
        .header(header::HOST, HOST)
        .header(header::ORIGIN, format!("https://{HOST}"))
        .header("cf-connecting-ip", "192.0.2.1")
        .header(header::COOKIE, &a)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CONTENT_LENGTH, crate::photo::MAX_PHOTO_BYTES + 1)
        .body(Body::empty())
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        response.body.contains("編集の画面に戻る"),
        "{}",
        response.body
    );
    assert_eq!(backend.objects.lock().unwrap().len(), 1);
}

/// 詳細の画面にある写真の URL（`/recipes/<id>/photos/<photo_id>`）。
fn photo_srcs(body: &str) -> Vec<String> {
    body.split(r#"<img src=""#)
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn photo_can_be_added_shown_and_deleted_on_the_edit_page() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let edit = format!("{path}/edit");
    let response = screen_get(&backend, &edit, Some(&a)).await;
    assert!(response.body.contains("写真はありません"));
    assert!(response.body.contains(r#"enctype="multipart/form-data""#));
    // 位置情報を取り除くことを、ファイルの欄の説明として書く。
    assert!(
        response
            .body
            .contains(r#"aria-describedby="photo-privacy""#)
    );
    assert!(response.body.contains(
        r#"id="photo-privacy">写真の位置情報は、保存する前に取り除きます（一部の写真では残ることがあります）。</p>"#
    ));
    // 写真の欄は、料理名などのフォームの外に置く（写真の操作は、料理名などの「保存する」とは別）。
    let (recipe_form, rest) = response.body.split_once("</form>").unwrap();
    assert!(recipe_form.contains(r#"name="name""#));
    assert!(!recipe_form.contains(r#"id="photos""#));
    assert!(rest.contains(r#"<section class="section" id="photos" data-photo-section"#));
    assert!(rest.contains("すぐ保存されます"));
    // 送っている間などの知らせの入れ物は、読み上げのために、空でも前から置く。
    assert!(rest.contains(r#"<p class="note" role="status" data-photo-status></p>"#));

    // JavaScript が動かないときは、普通のフォームとして送り、編集の画面に戻る。
    let response = post_photo(&backend, &path, Some(&a), Some(&jpeg(10))).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    assert_eq!(response.location, edit);
    assert!(
        set_cookie_named(&response, "__Host-flash")
            .unwrap()
            .starts_with("__Host-flash=photo-added;")
    );
    let response = screen_get(&backend, &edit, Some(&a)).await;
    let srcs = photo_srcs(&response.body);
    assert_eq!(srcs.len(), 1);
    assert!(response.body.contains(r#"alt="カレーの写真 1""#));
    assert!(response.body.contains("写真を削除"));
    // 写真の削除のボタンは同じ文字が並ぶので、何枚目かを名前（aria-label）に含める。
    assert!(
        response
            .body
            .contains(r#"aria-label="1 枚目の写真を削除">写真を削除</button>"#)
    );

    // 詳細の画面は、写真を見せるだけにする。
    let response = screen_get(&backend, &path, Some(&a)).await;
    assert_eq!(photo_srcs(&response.body), srcs);
    assert!(!response.body.contains(r#"enctype="multipart/form-data""#));
    assert!(!response.body.contains("写真を削除"));
    // 自分のレシピ帳では、編集の画面でできることは書かない（書かなくても分かる）。
    assert!(
        !response.body.contains("追加・削除でき"),
        "{}",
        response.body
    );
    // 最初の写真は遅らせずに高い優先度で読む（この写真は 1 枚なので、見える所に来てから読む写真は無い）。
    assert!(!response.body.contains(r#"loading="lazy""#));
    assert_eq!(response.body.matches(r#"fetchpriority="high""#).count(), 1);

    // 写真のデータは、画面の Cookie で読める。ブラウザに残させ、画面の no-store で上書きしない。
    let response = screen_get(&backend, &srcs[0], Some(&a)).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.content_type, "image/jpeg");
    assert_eq!(response.bytes, jpeg(10));
    assert_eq!(
        response.headers[header::CACHE_CONTROL],
        "private, max-age=86400"
    );
    assert!(
        response.headers[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    // ほかのワークスペースからは読めない。
    let response = screen_get(&backend, &srcs[0], Some(&cookie(WORKSPACE_B))).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);

    let response = screen_post(&backend, &format!("{}/delete", srcs[0]), Some(&a), "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    assert_eq!(response.location, edit);
    assert!(
        set_cookie_named(&response, "__Host-flash")
            .unwrap()
            .starts_with("__Host-flash=photo-deleted;")
    );
    let response = screen_get(&backend, &edit, Some(&a)).await;
    assert_eq!(photo_srcs(&response.body), Vec::<String>::new());
    assert!(backend.objects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn photo_errors_are_shown_on_the_edit_page() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    for (photo, status, message) in [
        (
            None,
            StatusCode::BAD_REQUEST,
            "追加する写真を選んでください",
        ),
        (
            Some(b"GIF89a".to_vec()),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "写真は JPEG・PNG・WebP のどれかにしてください",
        ),
        (
            // 頭の部分が途中で切れた JPEG。
            Some(vec![0xff, 0xd8, 0xff, 0xe1, 0x00, 0x10]),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "壊れたファイルと、画像の前の情報が 1 MiB を超えるファイルは追加できません",
        ),
        (
            // EXIF の印の付いた WebP。
            Some(b"RIFF\x16\0\0\0WEBPVP8X\x0a\0\0\0\x08\0\0\0\0\0\0\0\0\0".to_vec()),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "撮った場所などの情報（EXIF・XMP）を含む WebP は追加できません",
        ),
        (
            Some(jpeg(crate::photo::MAX_PHOTO_BYTES + 1)),
            StatusCode::PAYLOAD_TOO_LARGE,
            "写真は 1 枚 4,000,000 バイト（約 4 MB）までです",
        ),
    ] {
        let response = post_photo(&backend, &path, Some(&a), photo.as_deref()).await;
        assert_eq!(response.status, status, "{message}");
        assert!(response.body.contains(message), "{}", response.body);
        // 編集の画面に出す。画面の JavaScript が入れ替える、知らせと写真の欄がある。
        assert!(response.body.contains(r#"value="カレー""#), "{message}");
        assert!(
            response
                .body
                .contains(r#"<div id="messages" class="messages">"#)
        );
        assert!(response.body.contains(r#"id="photos""#), "{message}");
    }
    assert!(backend.photos.lock().unwrap().is_empty());
    assert!(backend.objects.lock().unwrap().is_empty());
}

/// フォーム（JavaScript の無い経路）と写真のバイト列そのもの（JavaScript の経路）のどちらでも、
/// 位置情報を取り除いてから保存する。
#[tokio::test]
async fn location_is_removed_on_both_upload_paths() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let response = post_photo(&backend, &path, Some(&a), Some(&jpeg_with_location())).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    let response = post_raw_photo(&backend, &path, &a, "image/jpeg", jpeg_with_location()).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT, "{}", response.body);
    let stored = backend.objects.lock().unwrap();
    assert_eq!(stored.len(), 2);
    for (bytes, _) in stored.values() {
        assert_eq!(bytes, &jpeg_with_orientation_only());
    }
}

/// 一覧の検索は、日本語の変換中に検索しないよう、`public/assets/search.js` が起こす search-input で検索する。
#[tokio::test]
async fn search_waits_for_the_ime_to_commit() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let response = screen_get(&backend, "/", Some(&a)).await;
    assert!(
        response
            .body
            .contains(r#"<script src="/assets/search.js" type="module"></script>"#)
    );
    assert!(response.body.contains(" data-search-input"));
    // 件数の文は、一覧の外に置いた読み上げの入れ物で、中身だけを入れ替える。
    assert!(
        response
            .body
            .contains(r##"hx-select-oob="#results-count:innerHTML""##)
    );
    let (count, results) = response
        .body
        .split_once(r#"<section id="results">"#)
        .unwrap();
    assert!(
        count.contains(r#"<p id="results-count" class="count" role="status">0 件</p>"#),
        "{count}"
    );
    assert!(!results.contains("results-count"));
    assert!(
        response
            .body
            .contains(r#"hx-trigger="search-input changed delay:300ms, search""#)
    );
}

/// 入力を消すボタンは、Tab で選べる本物のボタンにする（ブラウザが出す × は Tab で選べない）。
/// JavaScript が動かなければ、空のフォームを送って、絞り込まない一覧に戻る。
#[tokio::test]
async fn search_has_a_focusable_clear_button() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let response = screen_get(&backend, "/?q=%E3%82%AB%E3%83%AC%E3%83%BC", Some(&a)).await;
    // × は文字でなく、ボタンの中の SVG の図形だけで描き、読み上げない（名前はボタンの aria-label）。
    let start = r#"<button type="submit" form="clear-search" class="clear" data-search-clear aria-label="検索の文字を消す">"#;
    let button = response
        .body
        .split_once(start)
        .and_then(|(_, rest)| rest.split_once("</button>"))
        .map(|(inner, _)| inner.trim())
        .expect("入力を消すボタンが無い");
    assert!(
        button
            .starts_with(r#"<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">"#),
        "{button}"
    );
    assert!(button.ends_with("</svg>"), "{button}");
    assert!(
        response
            .body
            .contains(r#"<form id="clear-search" method="get" action="/" hidden></form>"#)
    );
}

/// 画面の JavaScript（`public/assets/photo-upload.js`）が、送る前に確かめるための値。
#[tokio::test]
async fn photo_form_tells_the_script_the_remaining_count_and_size_limit() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let edit = format!("{path}/edit");
    let response = screen_get(&backend, &edit, Some(&a)).await;
    assert!(
        response
            .body
            .contains(r#"<script src="/assets/photo-upload.js" type="module"></script>"#)
    );
    assert!(response.body.contains(" data-photo-upload "));
    assert!(response.body.contains(r#"data-remaining="5""#));
    assert!(response.body.contains(r#"data-max-bytes="4000000""#));
    assert!(response.body.contains(&format!(
        r#"data-too-large="{}""#,
        crate::screen::PHOTO_TOO_LARGE
    )));

    let response = post_photo(&backend, &path, Some(&a), Some(&jpeg(10))).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    let response = screen_get(&backend, &edit, Some(&a)).await;
    assert!(response.body.contains(r#"data-remaining="4""#));
}

#[tokio::test]
async fn add_form_is_hidden_when_the_recipe_has_the_maximum_photos() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    for _ in 0..crate::photo::MAX_PHOTOS_PER_RECIPE {
        let response = post_photo(&backend, &path, Some(&a), Some(&jpeg(10))).await;
        assert_eq!(response.status, StatusCode::SEE_OTHER);
    }
    let response = screen_get(&backend, &format!("{path}/edit"), Some(&a)).await;
    assert!(!response.body.contains(r#"enctype="multipart/form-data""#));
    assert!(!response.body.contains("data-photo-upload"));
    assert!(response.body.contains("写真は 5 枚まで付けられます"));
    let response = post_photo(&backend, &path, Some(&a), Some(&jpeg(10))).await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert!(response.body.contains("5 枚まで"), "{}", response.body);
}

#[tokio::test]
async fn demo_shows_photos_but_cannot_add_or_delete_them() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..FakeBackend::default()
    };
    backend.add_workspace(DEMO, NOW);
    let path = create_recipe(&backend, DEMO, "デモのカレー").await;
    let recipe = path.rsplit('/').next().unwrap().to_owned();
    // seed は R2 と D1 に直接入れる。
    let photo = "11111111111111111111111111111111";
    backend.add_photo_row(DEMO, &recipe, photo, 3);
    backend.objects.lock().unwrap().insert(
        format!("photos/{DEMO}/{recipe}/{photo}"),
        (jpeg(3), "image/jpeg".to_owned()),
    );

    // 詳細の画面と編集の画面の両方で、写真を見せるが、追加・削除のフォームは出さない。
    for page in [path.clone(), format!("{path}/edit")] {
        let response = screen_get(&backend, &page, None).await;
        assert_eq!(photo_srcs(&response.body).len(), 1, "{page}");
        assert!(
            response
                .body
                .contains("共有のデモでは、写真を追加・削除できません"),
            "{page}"
        );
        assert!(!response.body.contains(r#"enctype="multipart/form-data""#));
        assert!(!response.body.contains("写真を削除"));
        assert!(!response.body.contains("すぐ保存されます"));
    }
    let response = screen_get(&backend, &format!("{path}/photos/{photo}"), None).await;
    assert_eq!(response.status, StatusCode::OK);

    // フォームを使わずに送っても断る。
    let response = post_photo(&backend, &path, None, Some(&jpeg(10))).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let response = screen_post(&backend, &format!("{path}/photos/{photo}/delete"), None, "").await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn photo_form_from_another_site_is_rejected() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let (content_type, body) = photo_form(Some(&jpeg(10)));
    let request = Request::post(format!("{path}/photos"))
        .header(header::HOST, HOST)
        .header(header::ORIGIN, "https://evil.example")
        .header(header::COOKIE, &a)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn too_large_photo_form_shows_the_photo_limit_without_reading_the_body() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    // スマートフォンの写真（5 MB）。Content-Length で、本文を読む前に断る。
    let (content_type, _) = photo_form(Some(&jpeg(3)));
    let request = Request::post(format!("{path}/photos"))
        .header(header::HOST, HOST)
        .header(header::ORIGIN, format!("https://{HOST}"))
        .header(header::COOKIE, &a)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, 5_000_000)
        .body(Body::empty())
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        response
            .body
            .contains("写真は 1 枚 4,000,000 バイト（約 4 MB）までです"),
        "{}",
        response.body
    );
    assert!(
        response
            .body
            .contains(&format!(r#"href="{path}/edit">編集の画面に戻る"#))
    );
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unreadable_photo_form_is_shown_on_the_edit_page() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let request = Request::post(format!("{path}/photos"))
        .header(header::HOST, HOST)
        .header(header::ORIGIN, format!("https://{HOST}"))
        .header(header::COOKIE, &a)
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=x")
        .body(Body::from("not multipart"))
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(
        response.body.contains("写真を読めませんでした"),
        "{}",
        response.body
    );
    assert!(!response.body.contains("JSON"));
}

#[tokio::test]
async fn demo_id_is_never_treated_as_own_workspace() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..FakeBackend::default()
    };
    backend.add_workspace(DEMO, NOW);
    let path = create_recipe(&backend, DEMO, "デモのカレー").await;
    let recipe = path.rsplit('/').next().unwrap().to_owned();
    let photo = "11111111111111111111111111111111";
    backend.add_photo_row(DEMO, &recipe, photo, 3);

    // デモの ID は、「ほかの端末の ID」として入れても、無い ID と同じに断る（デモの ID だと分からないように）。
    let response = screen_post(&backend, "/workspace/switch", None, &format!("id={DEMO}")).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(response.body.contains("見つかりません"));
    assert_eq!(set_cookie_named(&response, "__Host-workspace"), None);

    // Cookie を手で書き換えても、デモとして扱い、無い ID と同じく Cookie を消す
    // （写真の追加・削除のボタンを出さず、送られても断る）。
    let demo_cookie = cookie(DEMO);
    let response = screen_get(&backend, &path, Some(&demo_cookie)).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(
        set_cookie_named(&response, "__Host-workspace")
            .unwrap()
            .contains("Max-Age=0")
    );
    assert!(
        response.body.contains("共有のデモです"),
        "{}",
        response.body
    );
    assert!(!response.body.contains(r#"enctype="multipart/form-data""#));
    assert!(!response.body.contains("写真を削除"));
    let response = screen_post(
        &backend,
        &format!("{path}/photos/{photo}/delete"),
        Some(&demo_cookie),
        "",
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
    // ワークスペースの画面にも、デモの ID を自分の ID として出さない。デモへの一時的な切り替えもさせない。
    let response = screen_get(&backend, "/workspace", Some(&demo_cookie)).await;
    assert!(!response.body.contains(DEMO), "{}", response.body);
    assert!(!response.body.contains("自分のレシピ帳に戻る"));
    let response = screen_post(&backend, "/workspace/demo", Some(&demo_cookie), "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    // デモの ID の Cookie を、無い ID と同じく消すだけで、デモを見る印は付けない。
    let mut cookies = set_cookies(&response);
    cookies.sort();
    assert_eq!(
        cookies,
        [
            "__Host-demo-view=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax",
            "__Host-flash=workspace-gone; Path=/; Max-Age=60; Secure; HttpOnly; SameSite=Lax",
            "__Host-workspace=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax",
        ]
    );
}

#[test]
fn photo_limit_message_matches_max_photo_bytes() {
    // 画面の文（"4,000,000 バイト（約 4 MB）"・"1 枚 4 MB まで"）は定数から作らないので、上限を変えたら文も直す。
    let digits = crate::photo::MAX_PHOTO_BYTES.to_string();
    let mut grouped = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    assert!(
        crate::screen::PHOTO_TOO_LARGE.contains(&format!("{grouped} バイト")),
        "{grouped}"
    );
    assert_eq!(
        crate::photo::MAX_PHOTO_BYTES % 1_000_000,
        0,
        "MB の書き方も直す"
    );
    let megabytes = crate::photo::MAX_PHOTO_BYTES / 1_000_000;
    assert!(crate::screen::PHOTO_TOO_LARGE.contains(&format!("約 {megabytes} MB")));
    let section = include_str!("../../templates/photo_section.html");
    assert!(section.contains(&format!("1 枚 {megabytes} MB まで")));
}

#[tokio::test]
async fn switching_from_a_deleted_workspace_keeps_the_new_cookie() {
    let backend = backend_with_workspaces();
    // Cookie の ID のワークスペースが消えている（自動削除など）まま、ほかの ID に切り替える・作る。
    let gone = cookie("cccccccccccccccccccccccccccccccc");
    let response = screen_post(
        &backend,
        "/workspace/switch",
        Some(&gone),
        &format!("id={WORKSPACE_B}"),
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    let cookies: Vec<String> = set_cookies(&response)
        .into_iter()
        .filter(|cookie| cookie.starts_with("__Host-workspace="))
        .collect();
    // ハンドラーが付けた Cookie には、消えた ID の削除を足さない（新しい Cookie を打ち消さず、同じ名前を 2 つ並べない）。
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    assert!(cookies[0].starts_with(&cookie(WORKSPACE_B)), "{cookies:?}");
    // 新しいワークスペースを使い始めたので、消えたことは知らせない。
    assert!(
        set_cookie_named(&response, "__Host-flash")
            .unwrap()
            .starts_with("__Host-flash=workspace-switched;")
    );

    let response = screen_post(&backend, "/workspace", Some(&gone), "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    let cookies: Vec<String> = set_cookies(&response)
        .into_iter()
        .filter(|cookie| cookie.starts_with("__Host-workspace="))
        .collect();
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    assert!(!cookies[0].contains("Max-Age=0"), "{cookies:?}");
}

#[tokio::test]
async fn too_large_photo_form_without_content_length_is_shown_on_the_edit_page() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    // Content-Length の無い本文（chunked）は、読む途中で本文の上限（写真の上限 + 64 KiB）に当たる。
    let (content_type, body) = photo_form(Some(&jpeg(crate::screen::PHOTO_FORM_MAX_BYTES + 1)));
    let request = Request::post(format!("{path}/photos"))
        .header(header::HOST, HOST)
        .header(header::ORIGIN, format!("https://{HOST}"))
        .header(header::COOKIE, &a)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        response.body.contains(crate::screen::PHOTO_TOO_LARGE),
        "{}",
        response.body
    );
    // エラーの画面ではなく、編集の画面に出す。
    assert!(response.body.contains(r#"value="カレー""#));
    assert!(response.body.contains(r#"id="photos""#));
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn deleting_workspace_while_visiting_demo_keeps_demo_and_removes_photos() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..backend_with_workspaces()
    };
    backend.add_workspace(DEMO, NOW);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let recipe = path.trim_start_matches("/recipes/").to_owned();
    let photo = "11111111111111111111111111111111";
    backend.add_photo_row(WORKSPACE_A, &recipe, photo, 3);
    backend.objects.lock().unwrap().insert(
        format!("photos/{WORKSPACE_A}/{recipe}/{photo}"),
        (vec![0xff, 0xd8, 0xff], "image/jpeg".to_owned()),
    );
    create_recipe(&backend, DEMO, "デモのカレー").await;

    // 一時的にデモを見ている間に消しても、消すのは自分のワークスペースで、デモは残る。
    let visiting = format!("{}; __Host-demo-view=1", cookie(WORKSPACE_A));
    let response = screen_post(
        &backend,
        "/workspace/delete",
        Some(&visiting),
        &format!("id={WORKSPACE_A}"),
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    assert!(!backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
    assert!(backend.workspaces.lock().unwrap().contains_key(DEMO));
    assert!(
        backend
            .recipes
            .lock()
            .unwrap()
            .keys()
            .any(|(workspace, _)| workspace == DEMO)
    );
    // 写真のデータも R2 から消える。
    assert!(backend.objects.lock().unwrap().is_empty());
    assert!(backend.photos.lock().unwrap().is_empty());

    // Cookie を書き換えてデモの ID にしても、自分のワークスペースとして扱わないので、デモは消えない。
    let response = screen_post(
        &backend,
        "/workspace/delete",
        Some(&cookie(DEMO)),
        &format!("id={DEMO}"),
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert!(backend.workspaces.lock().unwrap().contains_key(DEMO));
}

#[tokio::test]
async fn workspace_deleted_by_another_request_is_treated_as_deleted() {
    // 画面を開いて送るまでの間に、別のタブなどが同じワークスペースを消していた。
    let backend = FakeBackend {
        workspace_vanishes_on_delete: true,
        ..backend_with_workspaces()
    };
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let recipe = path.trim_start_matches("/recipes/").to_owned();
    let photo = "11111111111111111111111111111111";
    backend.add_photo_row(WORKSPACE_A, &recipe, photo, 3);
    backend.objects.lock().unwrap().insert(
        format!("photos/{WORKSPACE_A}/{recipe}/{photo}"),
        (vec![0xff, 0xd8, 0xff], "image/jpeg".to_owned()),
    );
    let response = screen_post(
        &backend,
        "/workspace/delete",
        Some(&cookie(WORKSPACE_A)),
        &format!("id={WORKSPACE_A}"),
    )
    .await;
    // 消えたことに変わりはないので、消せたときと同じく、知らせを付けて一覧へ移り、Cookie も消す。
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    assert_eq!(response.location, "/");
    assert!(
        set_cookie_named(&response, "__Host-flash")
            .unwrap()
            .starts_with("__Host-flash=workspace-deleted;")
    );
    for name in ["__Host-workspace", "__Host-demo-view"] {
        assert!(
            set_cookie_named(&response, name)
                .unwrap()
                .contains("Max-Age=0"),
            "{name}"
        );
    }
    // ワークスペース・レシピ・写真の行と、R2 のデータが残っていない。
    assert!(!backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
    assert!(backend.recipes.lock().unwrap().is_empty());
    assert!(backend.photos.lock().unwrap().is_empty());
    assert!(backend.objects.lock().unwrap().is_empty());
}

/// 一覧のカード（`<ul class="recipes">` の各 `<li>`）の HTML を、並びの順に返す。
fn list_cards(body: &str) -> Vec<&str> {
    let start = body.find(r#"<ul class="recipes">"#).expect("一覧が無い");
    let list = &body[start..start + body[start..].find("</ul>").unwrap()];
    list.split("<li>").skip(1).collect()
}

/// 一覧は、写真のあるレシピに先頭の写真（追加した順の最初）を小さく出し、写真の無いレシピには「写真なし」の枠を出す。
#[tokio::test]
async fn list_shows_the_first_photo_of_each_recipe() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let with_photos = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let without_photos = create_recipe(&backend, WORKSPACE_A, "シチュー").await;
    let recipe = with_photos.rsplit('/').next().unwrap().to_owned();
    // 追加した日時が同じなら、ID の小さいものを先頭にする。日時が古いものは、ID が大きくても先にする。
    backend.add_photo_row_at(
        WORKSPACE_A,
        &recipe,
        "cccccccccccccccccccccccccccccccc",
        3,
        NOW + 2,
    );
    backend.add_photo_row_at(
        WORKSPACE_A,
        &recipe,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        3,
        NOW + 1,
    );
    backend.add_photo_row_at(
        WORKSPACE_A,
        &recipe,
        "dddddddddddddddddddddddddddddddd",
        3,
        NOW + 1,
    );

    let other = create_recipe(&backend, WORKSPACE_A, "ハヤシライス").await;
    let other_id = other.rsplit('/').next().unwrap().to_owned();
    backend.add_photo_row_at(
        WORKSPACE_A,
        &other_id,
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        3,
        NOW + 1,
    );

    let response = screen_get(&backend, "/", Some(&a)).await;
    assert_eq!(response.status, StatusCode::OK);
    // ページの説明は、どの画面でも同じで、レシピの中身を書かない。
    let description = r#"<meta name="description" content="料理名・何人分か・材料・手順と写真を書き留める、ログインのいらないレシピ帳です。">"#;
    assert!(response.body.contains(description));
    assert_eq!(
        response.body.matches(r#"<meta name="description""#).count(),
        1
    );
    let cards = list_cards(&response.body);
    assert_eq!(cards.len(), 3, "{}", response.body);
    let card_of = |path: &str| {
        cards
            .iter()
            .find(|card| card.contains(&format!(r#"<a class="card-link" href="{path}">"#)))
            .unwrap_or_else(|| panic!("{path}: {}", response.body))
    };
    assert!(card_of(&with_photos).contains(&format!(
        r#"<img class="thumb" src="{with_photos}/photos/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" alt=""#
    )));
    assert!(card_of(&other).contains(&format!(
        r#"<img class="thumb" src="{other}/photos/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee" alt=""#
    )));
    // 写真の無いレシピには、読み込み中と見分けられる「写真なし」の枠を、写真と同じ所に出す。
    let empty = card_of(&without_photos);
    assert!(!empty.contains("<img"), "{empty}");
    assert!(empty.contains(r#"<div class="thumb no-photo" aria-hidden="true">"#));
    assert!(empty.contains("写真なし"));
    // 写真のある最初のカードの写真だけを、遅らせずに高い優先度で読む。ほかは見える所に来てから読む。
    assert_eq!(response.body.matches("fetchpriority").count(), 1);
    let priority = cards.iter().position(|card| card.contains("<img")).unwrap();
    for (index, card) in cards.iter().enumerate() {
        if !card.contains("<img") {
            continue;
        }
        if index == priority {
            assert!(
                card.contains(r#" fetchpriority="high" width="240" height="160">"#),
                "{card}"
            );
            assert!(!card.contains("loading="), "{card}");
        } else {
            assert!(
                card.contains(r#" loading="lazy" width="240" height="160">"#),
                "{card}"
            );
            assert!(!card.contains("fetchpriority"), "{card}");
        }
    }

    // 詳細の画面の写真の並びの最初と同じ写真である。
    let response = screen_get(&backend, &with_photos, Some(&a)).await;
    assert!(response.body.contains(description));
    // 最初の写真だけを遅らせずに高い優先度で読み、ほか（ここでは 2 枚）は見える所に来てから読む。
    assert_eq!(response.body.matches(r#"fetchpriority="high""#).count(), 1);
    assert_eq!(response.body.matches(r#"loading="lazy""#).count(), 2);
    assert_eq!(
        photo_srcs(&response.body)[0],
        format!("{with_photos}/photos/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    );

    // API の一覧は、今までどおりレシピの中身だけを返す。
    let request = Request::get("/api/recipes")
        .header("x-workspace-id", WORKSPACE_A)
        .body(Body::empty())
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert!(!response.body.contains("first_photo"), "{}", response.body);
    assert!(!response.body.contains("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
}

/// 登録の画面では、まだ写真を付けられないので、保存した後に編集の画面で追加できることを書く。
#[tokio::test]
async fn new_form_says_photos_can_be_added_after_saving() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..backend_with_workspaces()
    };
    backend.add_workspace(DEMO, NOW);
    let response = screen_get(&backend, "/recipes/new", Some(&cookie(WORKSPACE_A))).await;
    assert!(
        response
            .body
            .contains("写真は、保存した後に、編集の画面で追加できます")
    );
    assert!(!response.body.contains(r#"id="photos""#));
    // 共有のデモでは写真を付けられないので書かない。
    let response = screen_get(&backend, "/recipes/new", None).await;
    assert!(
        response.body.contains("共有のデモです"),
        "{}",
        response.body
    );
    assert!(!response.body.contains("写真は、保存した後に"));
}

/// 編集の画面で入力に誤りがあって出し直すときも、写真の欄を残す。
#[tokio::test]
async fn edit_form_errors_keep_the_photo_section() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let response = post_photo(&backend, &path, Some(&a), Some(&jpeg(10))).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    let response = screen_post(&backend, &path, Some(&a), "name=&servings=2").await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(response.body.contains(r#"id="photos""#));
    // 写真の代替テキストは、入力した（空の）料理名ではなく、保存してある料理名で作る。
    assert!(
        response.body.contains(r#"alt="カレーの写真 1""#),
        "{}",
        response.body
    );
}

/// 写真の欄を読めなくても（別のタブでレシピを消したなど）、入力の誤りの画面を出し、書きかけの入力を残す。
#[tokio::test]
async fn edit_form_errors_are_shown_even_if_the_photo_section_cannot_be_read() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let response = screen_post(&backend, &format!("{path}/delete"), Some(&a), "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    let response = screen_post(
        &backend,
        &path,
        Some(&a),
        "name=&servings=2&steps=%E7%85%AE%E3%82%8B",
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{}",
        response.body
    );
    assert!(response.body.contains("入力に誤りがあります"));
    assert!(response.body.contains("煮る"));
    assert!(!response.body.contains(r#"id="photos""#));
}

/// 書き込みの回数の制限に当たったら、「はじめる」画面に理由を出し、ワークスペースを作らない。
#[tokio::test]
async fn write_limit_on_start_shows_the_reason_on_the_landing_page() {
    let backend = FakeBackend {
        write_limited: true,
        ..FakeBackend::default()
    };
    let response = screen_post(&backend, "/workspace", None, "").await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(response.content_type.starts_with("text/html"));
    assert!(
        response.body.contains("しばらく待って"),
        "{}",
        response.body
    );
    // エラーの画面ではなく、「はじめる」画面（作るボタンがある）。
    assert!(
        response.body.contains(r#"action="/workspace""#),
        "{}",
        response.body
    );
    assert!(
        !response.body.contains("<title>エラー"),
        "{}",
        response.body
    );
    assert_eq!(set_cookies(&response), Vec::<String>::new());
    assert!(backend.workspaces.lock().unwrap().is_empty());
    assert!(backend.workspace_creations.lock().unwrap().is_empty());
    assert_eq!(response.headers["hx-push-url"], "false");
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
}

/// 書き込みの回数の制限に当たったら、「ワークスペース」の画面に理由を出し、切り替えない。
#[tokio::test]
async fn write_limit_on_switch_shows_the_reason_on_the_workspace_page() {
    let backend = FakeBackend {
        write_limited: true,
        ..backend_with_workspaces()
    };
    let response = screen_post(
        &backend,
        "/workspace/switch",
        Some(&cookie(WORKSPACE_A)),
        &format!("id={WORKSPACE_B}"),
    )
    .await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        response.body.contains("しばらく待って"),
        "{}",
        response.body
    );
    // 「ワークスペース」の画面で、今の自分の ID を出したまま。
    assert!(response.body.contains(WORKSPACE_A), "{}", response.body);
    assert!(response.body.contains(r#"action="/workspace/switch""#));
    assert_eq!(set_cookies(&response), Vec::<String>::new());
    assert_eq!(response.headers["hx-push-url"], "false");
}

/// 書き込みの回数の制限に当たったら、登録と更新のフォームを、書きかけの入力を保って出し直し、理由を出す。
#[tokio::test]
async fn write_limit_keeps_the_recipe_form_input() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let limited = FakeBackend {
        write_limited: true,
        ..backend.clone()
    };
    // 入力に誤り（空の料理名）があっても、回数の制限を先に答える（API と同じ順）。
    for form in [
        "name=%E8%82%89%E3%81%98%E3%82%83%E3%81%8C&servings=3&steps=%E7%85%AE%E3%82%8B",
        "name=&servings=3&steps=%E7%85%AE%E3%82%8B",
    ] {
        for target in ["/recipes", path.as_str()] {
            let response = screen_post(&limited, target, Some(&a), form).await;
            assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS, "{target}");
            assert!(
                response.body.contains("しばらく待って"),
                "{}",
                response.body
            );
            assert!(response.body.contains("煮る"), "{}", response.body);
            assert!(response.body.contains(&format!(r#"action="{target}""#)));
            assert!(!response.body.contains("入力に誤りがあります"));
            assert_eq!(response.headers["hx-push-url"], "false");
        }
    }
    // 保存していない。
    let recipes = backend.recipes.lock().unwrap();
    assert_eq!(recipes.len(), 1);
    assert!(recipes.values().all(|recipe| recipe.name == "カレー"));
}

/// 回数の制限に当たり、本文も大きすぎるときは、フォームを読まずにエラーの画面で回数の制限を答える。
#[tokio::test]
async fn write_limit_with_too_large_body_is_answered_without_reading_the_form() {
    let backend = FakeBackend {
        write_limited: true,
        ..backend_with_workspaces()
    };
    let mut request = screen_request(
        &Method::POST,
        "/recipes",
        Some(&cookie(WORKSPACE_A)),
        Some("name=x&servings=1"),
    );
    request.headers_mut().insert(
        header::CONTENT_LENGTH,
        (MAX_BODY_BYTES + 1).to_string().parse().unwrap(),
    );
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(response.body.contains("<title>エラー"), "{}", response.body);
    assert!(!response.body.contains(r#"name="name""#));
}

/// フォームの無い書き込み（削除など）は、回数の制限に当たると、エラーの画面で断る。
#[tokio::test]
async fn write_limit_on_other_writes_shows_the_error_page() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let limited = FakeBackend {
        write_limited: true,
        ..backend.clone()
    };
    for target in [format!("{path}/delete"), "/workspace/delete".to_owned()] {
        let form = format!("id={WORKSPACE_A}");
        let response = screen_post(&limited, &target, Some(&a), &form).await;
        assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS, "{target}");
        assert!(response.body.contains("<title>エラー"), "{}", response.body);
    }
    assert_eq!(backend.recipes.lock().unwrap().len(), 1);
    assert_eq!(backend.workspaces.lock().unwrap().len(), 2);
}

/// D1 の一時的な失敗で保存できなかったときも、書きかけの入力を保ってフォームを出し直す。
#[tokio::test]
async fn server_failure_on_save_keeps_the_recipe_form_input() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let broken = FakeBackend {
        broken_recipe_write: true,
        ..backend.clone()
    };
    for target in ["/recipes", path.as_str()] {
        let response = screen_post(
            &broken,
            target,
            Some(&a),
            "name=x&servings=3&steps=%E7%85%AE%E3%82%8B",
        )
        .await;
        assert_eq!(
            response.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{target}"
        );
        assert!(response.body.contains("サーバーで問題が起きました"));
        assert!(response.body.contains("煮る"), "{}", response.body);
        assert!(response.body.contains(&format!(r#"action="{target}""#)));
    }
}

/// 消されたレシピへの保存は、送り直しても保存できないので、エラーの画面にする。
#[tokio::test]
async fn saving_a_deleted_recipe_shows_not_found() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let response = screen_post(&backend, &format!("{path}/delete"), Some(&a), "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    let response = screen_post(&backend, &path, Some(&a), "name=x&servings=1").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(response.body.contains("レシピが見つかりません"));
    assert!(backend.recipes.lock().unwrap().is_empty());
}

/// 同じレシピを 2 つのタブで編集したときは、後から保存したほうが残る（楽観ロックをしない）。
#[tokio::test]
async fn later_save_wins_when_edited_in_two_tabs() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    for name in ["first", "second"] {
        let form = format!("name={name}&servings=1");
        let response = screen_post(&backend, &path, Some(&a), &form).await;
        assert_eq!(response.status, StatusCode::SEE_OTHER);
    }
    let response = screen_get(&backend, &path, Some(&a)).await;
    assert!(response.body.contains("second"));
    assert!(!response.body.contains("first"));
}

/// 画面の無いパスは、一覧に戻れる 404 のエラーの画面にし、画面と同じヘッダーを付ける。
#[tokio::test]
async fn unknown_page_shows_the_not_found_page() {
    let backend = backend_with_workspaces();
    for path in [
        "/nope",
        "/recipes/",
        "/assets/nope.css",
        "/workspace/switch/",
    ] {
        let response = screen_get(&backend, path, Some(&cookie(WORKSPACE_A))).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
        assert!(response.content_type.starts_with("text/html"), "{path}");
        assert!(response.body.contains("ページが見つかりません"), "{path}");
        assert!(response.body.contains(r#"href="/">一覧に戻る"#), "{path}");
        let csp = response.headers[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self'"), "{csp}");
        assert_eq!(response.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(response.headers[header::REFERRER_POLICY], "same-origin");
        assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    }
}

/// POST だけのパスへの GET は、関係する画面へ移す。ほかの受け付けないメソッドは、405 のエラーの画面にする。
#[tokio::test]
async fn get_to_post_only_paths_moves_to_the_related_page() {
    let backend = backend_with_workspaces();
    let id = "0123456789abcdef0123456789abcdef";
    let photo = "fedcba9876543210fedcba9876543210";
    let cases = [
        ("/recipes".to_owned(), "/".to_owned()),
        ("/recipes/check".to_owned(), "/".to_owned()),
        (format!("/recipes/{id}/delete"), format!("/recipes/{id}")),
        (
            format!("/recipes/{id}/photos"),
            format!("/recipes/{id}/edit"),
        ),
        (
            format!("/recipes/{id}/photos/{photo}/delete"),
            format!("/recipes/{id}/edit"),
        ),
        // ID の形が正しくなければ、パスの文字を移り先に写さずに一覧へ移す。
        (
            "/recipes/%2F%2Fevil.example/delete".to_owned(),
            "/".to_owned(),
        ),
        ("/recipes/xyz/photos".to_owned(), "/".to_owned()),
        ("/workspace/switch".to_owned(), "/workspace".to_owned()),
        ("/workspace/delete".to_owned(), "/workspace".to_owned()),
        ("/workspace/demo".to_owned(), "/workspace".to_owned()),
        ("/workspace/own".to_owned(), "/workspace".to_owned()),
    ];
    for (path, location) in cases {
        let response = screen_get(&backend, &path, Some(&cookie(WORKSPACE_A))).await;
        assert_eq!(response.status, StatusCode::SEE_OTHER, "{path}");
        assert_eq!(response.location, location, "{path}");
        assert_eq!(
            response.headers[header::CACHE_CONTROL],
            "no-store",
            "{path}"
        );
    }
    // GET の無いパスでも、ほかのメソッドは 405 のエラーの画面にする。受け付けるメソッドは Allow で知らせる。
    for (method, path) in [
        (Method::PUT, format!("/recipes/{id}")),
        (Method::DELETE, "/workspace".to_owned()),
        (Method::POST, format!("/recipes/{id}/photos/{photo}")),
        (Method::PUT, "/recipes/check".to_owned()),
    ] {
        let request = screen_request(&method, &path, Some(&cookie(WORKSPACE_A)), None);
        let response = send(router(backend.clone()), request).await;
        assert_eq!(
            response.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {path}"
        );
        assert!(response.content_type.starts_with("text/html"), "{path}");
        assert!(response.body.contains("この方法では開けません"), "{path}");
        assert!(!response.allow.is_empty(), "{path}");
        assert_eq!(response.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    }
}

/// パスの ID が UTF-8 として読めないときも、API と同じく「見つからない」の 404 にする（axum の既定の 400 にしない）。
#[tokio::test]
async fn ids_that_are_not_utf8_are_not_found() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let cases = [
        (
            Method::GET,
            "/recipes/%ff".to_owned(),
            "レシピが見つかりません",
        ),
        (
            Method::GET,
            "/recipes/%ff/edit".to_owned(),
            "レシピが見つかりません",
        ),
        (
            Method::POST,
            "/recipes/%ff".to_owned(),
            "レシピが見つかりません",
        ),
        (
            Method::POST,
            "/recipes/%ff/delete".to_owned(),
            "レシピが見つかりません",
        ),
        (
            Method::POST,
            "/recipes/%ff/photos".to_owned(),
            "レシピが見つかりません",
        ),
        (
            Method::GET,
            "/recipes/%ff/photos/x".to_owned(),
            "レシピが見つかりません",
        ),
        (
            Method::POST,
            "/recipes/%ff/photos/x/delete".to_owned(),
            "レシピが見つかりません",
        ),
        (Method::GET, format!("{path}/photos/%ff"), "見つかりません"),
    ];
    for (method, target, message) in cases {
        let form = (method == Method::POST).then_some("name=x&servings=1");
        let request = screen_request(&method, &target, Some(&a), form);
        let response = send(router(backend.clone()), request).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{method} {target}");
        assert!(
            response.body.contains(message),
            "{target}: {}",
            response.body
        );
        assert!(!response.body.contains("Invalid URL"), "{target}");
    }
}

/// 作った要求で同じ欄を 2 つ送っても、API の「JSON として読めない」ではなく、画面の言葉で答える。
#[tokio::test]
async fn unreadable_form_is_answered_in_screen_words() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    for target in ["/recipes", path.as_str(), "/recipes/check"] {
        let response = screen_post(
            &backend,
            target,
            Some(&a),
            "name=x&servings=1&steps=a&steps=b",
        )
        .await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{target}");
        assert!(
            response.body.contains("フォームの内容を読めませんでした"),
            "{}",
            response.body
        );
        assert!(!response.body.contains("JSON"), "{target}");
    }
}

/// 回数の制限に当たっているときは、フォームを読めない（Content-Length の無い大きすぎる本文など）・ID の形が正しくないときも、
/// 回数の制限を先に答える。ID の形が正しくないパスは、ハンドラーに渡さない。
#[tokio::test]
async fn write_limit_is_answered_before_unreadable_forms_and_bad_ids() {
    let backend = FakeBackend {
        write_limited: true,
        ..backend_with_workspaces()
    };
    let a = cookie(WORKSPACE_A);
    let response = screen_post(&backend, "/recipes", Some(&a), "name=x&steps=a&steps=b").await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(response.body.contains("<title>エラー"), "{}", response.body);
    for target in ["/recipes/xyz", "/recipes/%ff"] {
        let response = screen_post(&backend, target, Some(&a), "name=x&servings=1").await;
        assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS, "{target}");
        assert!(response.body.contains("<title>エラー"), "{}", response.body);
    }
}

/// 回数の制限に当たっていても、送り元（Origin）を先に確かめる。別のサイトからの POST は、印を付けてハンドラーへ渡さずに 403 で断る。
/// 印を付けるパスのほか、印を付けない書き込みと写真のフォームも確かめる。
#[tokio::test]
async fn posts_from_other_sites_are_rejected_before_the_write_limit() {
    let backend = FakeBackend {
        write_limited: true,
        ..backend_with_workspaces()
    };
    // identify に届けば最終利用日時を書き換えるよう、前の更新から時間をたたせておく。
    backend.advance(crate::workspace::LAST_USED_UPDATE_INTERVAL_MILLIS);
    let id = "0123456789abcdef0123456789abcdef";
    for path in [
        "/workspace".to_owned(),
        "/workspace/switch".to_owned(),
        "/recipes".to_owned(),
        format!("/recipes/{id}"),
        format!("/recipes/{id}/delete"),
        "/workspace/delete".to_owned(),
        format!("/recipes/{id}/photos"),
    ] {
        for origin in [None, Some("https://evil.example"), Some("null")] {
            let form = format!("id={WORKSPACE_B}&name=x&servings=1");
            let mut request = screen_request(
                &Method::POST,
                &path,
                Some(&cookie(WORKSPACE_A)),
                Some(&form),
            );
            request.headers_mut().remove(header::ORIGIN);
            if let Some(origin) = origin {
                request
                    .headers_mut()
                    .insert(header::ORIGIN, origin.parse().unwrap());
            }
            let response = send(router(backend.clone()), request).await;
            assert_eq!(response.status, StatusCode::FORBIDDEN, "{path} {origin:?}");
            assert!(
                response.body.contains("ほかのサイトから送られたフォーム"),
                "{}",
                response.body
            );
            assert_eq!(
                set_cookies(&response),
                Vec::<String>::new(),
                "{path} {origin:?}"
            );
        }
    }
    // 回数の制限を数えず、identify にも届かない（最終利用日時を書き換えない）。
    assert!(backend.write_limit_keys.lock().unwrap().is_empty());
    assert_eq!(
        backend
            .last_used_writes
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

/// 覚えていたワークスペースが無くなっていたことの知らせ（赤）の HTML。
fn gone_alert() -> String {
    "<p class=\"message error\">このブラウザが覚えていたワークスペースは見つかりませんでした。\
     14 日アクセスが無くて消えたか、削除されたためです</p>"
        .to_owned()
}

#[tokio::test]
async fn stale_cookie_is_told_once_on_the_demo() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..FakeBackend::default()
    };
    backend.add_workspace(DEMO, NOW);
    let stale = cookie(WORKSPACE_A);
    // 黙ってデモにせず、デモの画面に誤りの知らせとして出す（読み上げの入れ物の中に）。
    let response = screen_get(&backend, "/", Some(&stale)).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.body.contains("共有のデモです"));
    assert!(
        response.body.contains(&format!(
            r#"<div id="messages-alert" role="alert">{}</div>"#,
            gone_alert()
        )),
        "{}",
        response.body
    );
    assert!(
        set_cookie_named(&response, "__Host-workspace")
            .is_some_and(|cookie| cookie.contains("Max-Age=0"))
    );
    assert_eq!(set_cookie_named(&response, "__Host-flash"), None);
    // Cookie が消えた後は、もう出さない。
    let response = screen_get(&backend, "/", None).await;
    assert!(!response.body.contains("見つかりませんでした"));
    // ほかの誤りの知らせと一緒でも出す（切り替えの欄の誤り）。
    let response = screen_post(&backend, "/workspace/switch", Some(&stale), "id=xyz").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(response.body.contains(&gone_alert()), "{}", response.body);
    assert!(response.body.contains("workspace-id-error"));
}

#[tokio::test]
async fn stale_cookie_is_told_on_the_next_page_when_the_response_cannot_show_it() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..FakeBackend::default()
    };
    backend.add_workspace(DEMO, NOW);
    let stale = cookie(WORKSPACE_A);
    let gone_flash =
        "__Host-flash=workspace-gone; Path=/; Max-Age=60; Secure; HttpOnly; SameSite=Lax";

    // 移動の応答。ハンドラーが付けた知らせ（登録しました）より、消えたことを優先して次の画面に回す。
    let response = screen_post(&backend, "/recipes", Some(&stale), "name=x&servings=1").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    let flashes: Vec<String> = set_cookies(&response)
        .into_iter()
        .filter(|cookie| cookie.starts_with("__Host-flash="))
        .collect();
    assert_eq!(flashes, [gone_flash]);

    // エラーの画面（消えたワークスペースのレシピのブックマークなど）。
    let response = screen_get(
        &backend,
        "/recipes/00000000000000000000000000000000",
        Some(&stale),
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(!response.body.contains("見つかりませんでした"));
    assert_eq!(
        set_cookie_named(&response, "__Host-flash").unwrap(),
        gone_flash
    );

    // htmx が一覧の部分だけを入れ替える検索。見出しの下の知らせは入れ替わらないので、次の画面に回す。
    let request = Request::get("/?q=x")
        .header(header::HOST, HOST)
        .header(header::COOKIE, &stale)
        .header("hx-request", "true")
        .body(Body::empty())
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        set_cookie_named(&response, "__Host-flash").unwrap(),
        gone_flash
    );
    // hx-boost の移動は body の全体を入れ替えるので、その画面で出す。
    let request = Request::get("/")
        .header(header::HOST, HOST)
        .header(header::COOKIE, &stale)
        .header("hx-request", "true")
        .header("hx-boosted", "true")
        .body(Body::empty())
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert!(response.body.contains(&gone_alert()));
    assert_eq!(set_cookie_named(&response, "__Host-flash"), None);

    // 回した知らせは、次に画面を出したときに一度だけ出す。エラーの画面では出せないので、残す。
    let with_flash = "__Host-flash=workspace-gone";
    let response = screen_get(
        &backend,
        "/recipes/00000000000000000000000000000000",
        Some(with_flash),
    )
    .await;
    assert_eq!(
        set_cookie_named(&response, "__Host-flash").unwrap(),
        gone_flash
    );
    let response = screen_get(&backend, "/", Some(with_flash)).await;
    assert!(response.body.contains(&gone_alert()), "{}", response.body);
    assert!(!response.body.contains("message success"));
    assert!(
        set_cookie_named(&response, "__Host-flash")
            .is_some_and(|cookie| cookie.contains("Max-Age=0"))
    );
}

#[tokio::test]
async fn creating_a_workspace_from_a_stale_cookie_does_not_tell_it_is_gone() {
    let backend = FakeBackend::default();
    let response = screen_post(&backend, "/workspace", Some(&cookie(WORKSPACE_A)), "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    let flashes: Vec<String> = set_cookies(&response)
        .into_iter()
        .filter(|cookie| cookie.starts_with("__Host-flash="))
        .collect();
    assert_eq!(
        flashes,
        ["__Host-flash=workspace-created; Path=/; Max-Age=60; Secure; HttpOnly; SameSite=Lax"]
    );
}

#[tokio::test]
async fn auto_deletion_is_told_where_workspaces_are_made_and_used() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..backend_with_workspaces()
    };
    backend.add_workspace(DEMO, NOW);
    let a = cookie(WORKSPACE_A);
    let note = "このレシピ帳は、14 日アクセスが無いと、レシピと写真ごと自動で消えます。";

    // 作った直後の知らせ。
    let response = screen_get(
        &backend,
        "/",
        Some(&format!("{a}; __Host-flash=workspace-created")),
    )
    .await;
    assert!(
        response.body.contains("14 日アクセスが無いと"),
        "{}",
        response.body
    );
    assert!(response.body.contains("控えておいてください"));

    // 自分の一覧には出し、デモの一覧には出さない。
    let response = screen_get(&backend, "/", Some(&a)).await;
    assert!(response.body.contains(note), "{}", response.body);
    let response = screen_get(&backend, "/", None).await;
    assert!(!response.body.contains(note));
    // デモの帯に、元に戻る時刻を書く。
    assert!(response.body.contains("毎日 4 時（日本時間）に元に戻り"));

    // ワークスペースの画面: ID の欄の説明と、切り替えると今の ID を忘れること。
    let response = screen_get(&backend, "/workspace", Some(&a)).await;
    assert!(response.body.contains(
        r#"<input id="current-id" value="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" readonly autocomplete="off" aria-describedby="current-id-hint">"#
    ), "{}", response.body);
    assert!(
        response
            .body
            .contains(r#"<p id="current-id-hint" class="note">Cookie を消したり"#)
    );
    assert!(
        response
            .body
            .contains(r#"<p id="workspace-id-hint" class="note">切り替えると"#)
    );
    assert!(
        response
            .body
            .contains(r#"aria-describedby="workspace-id-hint">"#)
    );
    // 誤りがあれば、説明と誤りの両方を結び付ける。
    let response = screen_post(&backend, "/workspace/switch", Some(&a), "id=").await;
    assert!(
        response
            .body
            .contains(r#"aria-describedby="workspace-id-hint workspace-id-error""#),
        "{}",
        response.body
    );

    // 自分のワークスペースが無ければ、切り替えの説明は出さず、作る前に自動で消えることを書く。
    let response = screen_get(&backend, "/workspace", None).await;
    assert!(!response.body.contains("workspace-id-hint"));
    assert!(
        response
            .body
            .contains("自分だけのレシピ帳は、14 日アクセスが無いと")
    );
    let response = screen_post(&backend, "/workspace/switch", None, "id=").await;
    assert!(
        response
            .body
            .contains(r#"aria-describedby="workspace-id-error""#),
        "{}",
        response.body
    );
}
