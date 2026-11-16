//! 画面（HTML）のテスト。Cookie でのワークスペースの受け取り、フォームの送信、送り元の確認を確かめる。

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};

use super::{
    TestResponse, WORKSPACE_A, WORKSPACE_B,
    fake::{FakeBackend, NOW},
    send,
};
use crate::{
    recipe::{MAX_BODY_BYTES, MAX_RECIPES_PER_WORKSPACE},
    router,
};

const HOST: &str = "recipe-box.example";
const DEMO: &str = "dddddddddddddddddddddddddddddddd";

fn backend_with_workspaces() -> FakeBackend {
    let backend = FakeBackend::default();
    backend.add_workspace(WORKSPACE_A, NOW);
    backend.add_workspace(WORKSPACE_B, NOW);
    backend
}

fn cookie(workspace: &str) -> String {
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

async fn screen_get(backend: &FakeBackend, path: &str, cookie: Option<&str>) -> TestResponse {
    send(
        router(backend.clone()),
        screen_request(&Method::GET, path, cookie, None),
    )
    .await
}

async fn screen_post(
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
async fn create_recipe(backend: &FakeBackend, workspace: &str, name: &str) -> String {
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
            .contains(r#"role="status">レシピを登録しました"#),
        "{}",
        response.body
    );
    assert!(
        set_cookie_named(&response, "__Host-flash")
            .is_some_and(|cookie| cookie.contains("Max-Age=0"))
    );
    // 知らない種類のコードは出さない（文は Cookie に入れない）。
    let response = screen_get(&backend, "/", Some(&format!("{a}; __Host-flash=<script>"))).await;
    assert!(!response.body.contains(r#"role="status""#));
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
    let response = screen_post(&backend, "/recipes", Some(&a), "name=&servings=2").await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    // 誤りの一覧は送信したときに出す。
    assert!(response.body.contains(r#"class="error-summary""#));
    for field in ["name", "servings", "ingredients", "steps"] {
        assert!(
            response.body.contains(&format!(
                r##"hx-post="/recipes/check" hx-trigger="blur" hx-include="closest form" hx-target="#field-{field}" hx-select="#field-{field}" hx-swap="outerHTML">"##
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

async fn post_photo(
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
async fn post_raw_photo(
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
    assert!(response.body.contains("レシピに戻る"), "{}", response.body);
    assert_eq!(backend.objects.lock().unwrap().len(), 1);
}

/// JPEG の先頭のバイトで始まり、全体で `size` バイトのデータ。
fn jpeg(size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    bytes[..3].copy_from_slice(&[0xff, 0xd8, 0xff]);
    bytes
}

/// 詳細の画面にある写真の URL（`/recipes/<id>/photos/<photo_id>`）。
fn photo_srcs(body: &str) -> Vec<String> {
    body.split(r#"<img src=""#)
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn photo_can_be_added_shown_and_deleted_on_the_detail_page() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let response = screen_get(&backend, &path, Some(&a)).await;
    assert!(response.body.contains("写真はありません"));
    assert!(response.body.contains(r#"enctype="multipart/form-data""#));

    let response = post_photo(&backend, &path, Some(&a), Some(&jpeg(10))).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    assert_eq!(response.location, path);
    assert!(
        set_cookie_named(&response, "__Host-flash")
            .unwrap()
            .starts_with("__Host-flash=photo-added;")
    );
    let response = screen_get(&backend, &path, Some(&a)).await;
    let srcs = photo_srcs(&response.body);
    assert_eq!(srcs.len(), 1);
    assert!(response.body.contains(r#"alt="カレーの写真 1""#));

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
    assert!(
        set_cookie_named(&response, "__Host-flash")
            .unwrap()
            .starts_with("__Host-flash=photo-deleted;")
    );
    let response = screen_get(&backend, &path, Some(&a)).await;
    assert_eq!(photo_srcs(&response.body), Vec::<String>::new());
    assert!(backend.objects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn photo_errors_are_shown_on_the_detail_page() {
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
            Some(jpeg(crate::photo::MAX_PHOTO_BYTES + 1)),
            StatusCode::PAYLOAD_TOO_LARGE,
            "写真は 1 枚 4,000,000 バイト（約 4 MB）までです",
        ),
    ] {
        let response = post_photo(&backend, &path, Some(&a), photo.as_deref()).await;
        assert_eq!(response.status, status, "{message}");
        assert!(response.body.contains(message), "{}", response.body);
        // 詳細の画面のまま出し、レシピの内容も見える。
        assert!(response.body.contains("<h2>材料</h2>"), "{message}");
    }
    assert!(backend.photos.lock().unwrap().is_empty());
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
            .contains(r#"<script src="/assets/search.js" defer></script>"#)
    );
    assert!(response.body.contains(" data-search-input"));
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
    assert!(response.body.contains(
        r#"<button type="submit" form="clear-search" class="clear" data-search-clear aria-label="検索の文字を消す">"#
    ));
    // × は SVG の図形で描き、読み上げない（名前はボタンの aria-label）。
    assert!(
        response
            .body
            .contains(r#"aria-hidden="true" focusable="false""#)
    );
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
    let response = screen_get(&backend, &path, Some(&a)).await;
    assert!(
        response
            .body
            .contains(r#"<script src="/assets/photo-upload.js" defer></script>"#)
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
    let response = screen_get(&backend, &path, Some(&a)).await;
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
    let response = screen_get(&backend, &path, Some(&a)).await;
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

    let response = screen_get(&backend, &path, None).await;
    assert_eq!(photo_srcs(&response.body).len(), 1);
    assert!(
        response
            .body
            .contains("共有のデモでは、写真を追加・削除できません")
    );
    assert!(!response.body.contains(r#"enctype="multipart/form-data""#));
    assert!(!response.body.contains("写真を削除"));
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
            .contains(&format!(r#"href="{path}">レシピに戻る"#))
    );
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unreadable_photo_form_is_shown_on_the_detail_page() {
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
    let detail = include_str!("../../templates/detail.html");
    assert!(detail.contains(&format!("1 枚 {megabytes} MB まで")));
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
async fn too_large_photo_form_without_content_length_is_shown_on_the_detail_page() {
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
    // エラーの画面ではなく、詳細の画面に出す。
    assert!(response.body.contains("<h2>材料</h2>"));
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
