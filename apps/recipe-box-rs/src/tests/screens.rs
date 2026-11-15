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
use crate::{recipe::MAX_RECIPES_PER_WORKSPACE, router};

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
    let cookies = set_cookies(&response);
    assert_eq!(cookies.len(), 1);
    let id = backend
        .workspaces
        .lock()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    assert_eq!(
        cookies[0],
        format!("__Host-workspace={id}; Path=/; Max-Age=34560000; Secure; HttpOnly; SameSite=Lax")
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
    // 正しくないページの番号は、エラーの画面にする。
    let response = screen_get(&backend, "/?page=9", Some(&a)).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
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
        set_cookies(&response),
        ["__Host-workspace=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax"]
    );
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
    assert!(set_cookies(&response)[0].starts_with(&cookie(WORKSPACE_B)));

    // 無い ID と形の正しくない ID は、理由を出して切り替えない。
    for id in ["cccccccccccccccccccccccccccccccc", "xyz"] {
        let response = screen_post(&backend, "/workspace/switch", None, &format!("id={id}")).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND);
        assert!(response.body.contains("見つかりません"));
        assert_eq!(set_cookies(&response), Vec::<String>::new());
    }

    let response = screen_post(
        &backend,
        "/workspace/forget",
        Some(&cookie(WORKSPACE_A)),
        "",
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert!(set_cookies(&response)[0].contains("Max-Age=0"));
    // 消すのは Cookie だけで、ワークスペースは残る。
    assert!(backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
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
