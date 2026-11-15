//! レシピの API（`/api/recipes`）のテスト。

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};

use super::{
    WORKSPACE_A, WORKSPACE_B, call,
    fake::{FakeBackend, NOW},
    send,
};
use crate::{
    recipe::{MAX_BODY_BYTES, MAX_RECIPES_PER_WORKSPACE},
    router,
};

/// ワークスペース A と B を持つ偽物を作る。
fn backend_with_workspaces() -> FakeBackend {
    let backend = FakeBackend::default();
    backend.add_workspace(WORKSPACE_A, NOW);
    backend.add_workspace(WORKSPACE_B, NOW);
    backend
}

fn curry() -> Value {
    json!({
        "name": "カレー",
        "servings": 4,
        "ingredients": "玉ねぎ 2 個\nにんじん 1 本",
        "steps": "切る\n煮る",
    })
}

/// ワークスペースにレシピを登録し、応答の本文を返す。
async fn create(backend: &FakeBackend, workspace: &str, body: &Value) -> Value {
    let response = call(
        backend,
        Method::POST,
        "/api/recipes",
        Some(workspace),
        Some(body),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    response.json()
}

/// 失敗の応答の状態コード・エラーのコード・項目を確かめる。
fn assert_error(
    response: &super::TestResponse,
    status: StatusCode,
    error: &str,
    field: Option<&str>,
) {
    assert_eq!(response.status, status, "{}", response.body);
    assert_eq!(response.content_type, "application/json");
    let body = response.json();
    assert_eq!(body["error"], error, "{}", response.body);
    assert_eq!(body["field"].as_str(), field, "{}", response.body);
    assert!(body["message"].is_string());
}

#[tokio::test]
async fn create_returns_created_recipe() {
    let backend = backend_with_workspaces();
    let response = call(
        &backend,
        Method::POST,
        "/api/recipes",
        Some(WORKSPACE_A),
        Some(&curry()),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED);
    let body = response.json();
    let id = body["id"].as_str().unwrap();
    assert_eq!(response.location, format!("/api/recipes/{id}"));
    assert_eq!(
        body,
        json!({
            "id": id,
            "name": "カレー",
            "servings": 4,
            "ingredients": "玉ねぎ 2 個\nにんじん 1 本",
            "steps": "切る\n煮る",
            "created_at": NOW,
            "updated_at": NOW,
        })
    );
}

#[tokio::test]
async fn create_treats_missing_ingredients_and_steps_as_empty() {
    let backend = backend_with_workspaces();
    let body = create(&backend, WORKSPACE_A, &json!({"name": "水", "servings": 1})).await;
    assert_eq!(body["ingredients"], "");
    assert_eq!(body["steps"], "");
}

#[tokio::test]
async fn create_rejects_invalid_fields() {
    let backend = backend_with_workspaces();
    let cases = [
        (json!({"servings": 2}), "name"),
        (json!({"name": null, "servings": 2}), "name"),
        (json!({"name": "", "servings": 2}), "name"),
        (json!({"name": "  ", "servings": 2}), "name"),
        (json!({"name": "あ".repeat(101), "servings": 2}), "name"),
        (json!({"name": "カレー"}), "servings"),
        (json!({"name": "カレー", "servings": 0}), "servings"),
        (json!({"name": "カレー", "servings": -1}), "servings"),
        (json!({"name": "カレー", "servings": 101}), "servings"),
        (
            json!({"name": "カレー", "servings": 2, "ingredients": "a".repeat(5_001)}),
            "ingredients",
        ),
        (
            json!({"name": "カレー", "servings": 2, "steps": "a".repeat(10_001)}),
            "steps",
        ),
    ];
    for (body, field) in cases {
        let response = call(
            &backend,
            Method::POST,
            "/api/recipes",
            Some(WORKSPACE_A),
            Some(&body),
        )
        .await;
        assert_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_field",
            Some(field),
        );
    }
    assert!(backend.recipes.lock().unwrap().is_empty());
}

/// `servings` を `raw`（JSON の数値の書き方そのもの）にして登録する。
async fn create_with_raw_servings(backend: &FakeBackend, raw: &str) -> super::TestResponse {
    let body = format!(r#"{{"name":"カレー","servings":{raw}}}"#);
    let request = Request::post("/api/recipes")
        .header("x-workspace-id", WORKSPACE_A)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();
    send(router(backend.clone()), request).await
}

#[tokio::test]
async fn servings_is_judged_by_value() {
    let backend = backend_with_workspaces();
    // 値が整数なら、書き方（2.0・1e2）に関わらず受け付ける。
    for (raw, expected) in [("2.0", 2), ("1e2", 100), ("1", 1)] {
        let response = create_with_raw_servings(&backend, raw).await;
        assert_eq!(
            response.status,
            StatusCode::CREATED,
            "{raw}: {}",
            response.body
        );
        assert_eq!(response.json()["servings"], expected, "{raw}");
    }
    // 整数でないか範囲の外なら invalid_field。i64 に収まらない数も同じ。
    for raw in ["2.5", "101", "1e3", "99999999999999999999", "-0.5"] {
        let response = create_with_raw_servings(&backend, raw).await;
        assert_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_field",
            Some("servings"),
        );
    }
}

#[tokio::test]
async fn create_rejects_unreadable_body() {
    let backend = backend_with_workspaces();
    let raw_bodies = [
        ("JSON でない", "not json", true),
        ("配列", "[]", true),
        (
            "servings が文字列",
            r#"{"name":"カレー","servings":"2"}"#,
            true,
        ),
        ("name が数値", r#"{"name":1,"servings":2}"#, true),
        // f64 で表せない数と、対になっていないサロゲートは、JSON として読めないものとして扱う（docs/spec.md）。
        (
            "servings が 1e400",
            r#"{"name":"カレー","servings":1e400}"#,
            true,
        ),
        (
            "対になっていないサロゲート",
            r#"{"name":"\ud800","servings":2}"#,
            true,
        ),
        (
            "Content-Type が無い",
            r#"{"name":"カレー","servings":2}"#,
            false,
        ),
    ];
    for (label, raw, json_content_type) in raw_bodies {
        let mut builder = Request::post("/api/recipes").header("x-workspace-id", WORKSPACE_A);
        if json_content_type {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        let request = builder.body(Body::from(raw)).unwrap();
        let response = send(router(backend.clone()), request).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{label}");
        assert_eq!(response.json()["error"], "invalid_body", "{label}");
    }
}

#[tokio::test]
async fn create_rejects_too_large_body() {
    let backend = backend_with_workspaces();
    let body = json!({"name": "カレー", "servings": 2, "steps": "a".repeat(MAX_BODY_BYTES)});
    let response = call(
        &backend,
        Method::POST,
        "/api/recipes",
        Some(WORKSPACE_A),
        Some(&body),
    )
    .await;
    assert_error(
        &response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        None,
    );
}

#[tokio::test]
async fn create_accepts_maximum_fields_sent_as_escaped_json() {
    // すべての文字を \uXXXX のエスケープの組（絵文字は 1 文字 12 バイト）で送っても、本文の上限に収まる。
    let emoji = "\\ud83c\\udf5b";
    let raw = format!(
        r#"{{"name":"{}","servings":100,"ingredients":"{}","steps":"{}"}}"#,
        emoji.repeat(100),
        emoji.repeat(2_000),
        emoji.repeat(5_000),
    );
    assert!(raw.len() > 64 * 1024, "{}", raw.len());
    let backend = backend_with_workspaces();
    let request = Request::post("/api/recipes")
        .header("x-workspace-id", WORKSPACE_A)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(raw))
        .unwrap();
    let response = send(router(backend), request).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    assert_eq!(response.json()["steps"], "🍛".repeat(5_000));
}

#[tokio::test]
async fn update_rejects_unreadable_or_too_large_body() {
    let backend = backend_with_workspaces();
    let created = create(&backend, WORKSPACE_A, &curry()).await;
    let path = format!("/api/recipes/{}", created["id"].as_str().unwrap());

    let request = Request::put(path.as_str())
        .header("x-workspace-id", WORKSPACE_A)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"カレー","servings":"2"}"#))
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_error(&response, StatusCode::BAD_REQUEST, "invalid_body", None);

    let body = json!({"name": "カレー", "servings": 2, "steps": "a".repeat(MAX_BODY_BYTES)});
    let response = call(&backend, Method::PUT, &path, Some(WORKSPACE_A), Some(&body)).await;
    assert_error(
        &response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        None,
    );

    let response = call(
        &backend,
        Method::PUT,
        &path,
        Some(WORKSPACE_A),
        Some(&json!({"name": "カレー", "servings": 0})),
    )
    .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "invalid_field",
        Some("servings"),
    );

    // どれも失敗したので、レシピは変わっていない。
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.json(), created);
}

#[tokio::test]
async fn null_ingredients_and_steps_are_treated_as_empty() {
    let backend = backend_with_workspaces();
    let body = create(
        &backend,
        WORKSPACE_A,
        &json!({"name": "水", "servings": 1, "ingredients": null, "steps": null}),
    )
    .await;
    assert_eq!(body["ingredients"], "");
    assert_eq!(body["steps"], "");
}

#[tokio::test]
async fn list_returns_newest_first() {
    let backend = backend_with_workspaces();
    let first = create(&backend, WORKSPACE_A, &curry()).await;
    backend.advance(1);
    let second = create(
        &backend,
        WORKSPACE_A,
        &json!({"name": "シチュー", "servings": 2}),
    )
    .await;

    let response = call(
        &backend,
        Method::GET,
        "/api/recipes",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.json(),
        json!({"recipes": [second, first], "total": 2, "page": 1, "per_page": 20})
    );
}

/// `GET <path>` の応答の本文を返す（200 であることも確かめる）。
async fn list_page(backend: &FakeBackend, path: &str) -> Value {
    let response = call(backend, Method::GET, path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    response.json()
}

/// 一覧の本文から、レシピの料理名を並び順のまま取り出す。
fn names_of(body: &Value) -> Vec<String> {
    body["recipes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|recipe| recipe["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn list_is_split_into_pages() {
    let backend = backend_with_workspaces();
    // 新しい順に「料理 25」から「料理 1」まで並ぶよう、1 ミリ秒ずつずらして 25 件作る。
    for n in 1..=25 {
        create(
            &backend,
            WORKSPACE_A,
            &json!({"name": format!("料理 {n}"), "servings": 1}),
        )
        .await;
        backend.advance(1);
    }

    // page を省くと 1 ページ目。1 ページは 20 件。
    let first = list_page(&backend, "/api/recipes").await;
    assert_eq!(first["total"], 25);
    assert_eq!(first["page"], 1);
    assert_eq!(first["per_page"], 20);
    let expected: Vec<String> = (6..=25).rev().map(|n| format!("料理 {n}")).collect();
    assert_eq!(names_of(&first), expected);
    assert_eq!(list_page(&backend, "/api/recipes?page=1").await, first);

    // 2 ページ目に残りの 5 件。値で読むので 02 も 2 ページ目。
    let second = list_page(&backend, "/api/recipes?page=2").await;
    assert_eq!(second["page"], 2);
    assert_eq!(second["total"], 25);
    let expected: Vec<String> = (1..=5).rev().map(|n| format!("料理 {n}")).collect();
    assert_eq!(names_of(&second), expected);
    assert_eq!(list_page(&backend, "/api/recipes?page=02").await, second);

    // レシピが少なく、上限の番号までページが無いときは、空の一覧（総数はそのまま）。
    let response = call(
        &backend,
        Method::GET,
        "/api/recipes?page=2",
        Some(WORKSPACE_B),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(
        response.json(),
        json!({"recipes": [], "total": 0, "page": 2, "per_page": 20})
    );
}

#[tokio::test]
async fn list_pages_search_results() {
    let backend = backend_with_workspaces();
    for n in 1..=22 {
        create(
            &backend,
            WORKSPACE_A,
            &json!({"name": format!("Curry {n}"), "servings": 1}),
        )
        .await;
        backend.advance(1);
    }
    create(
        &backend,
        WORKSPACE_A,
        &json!({"name": "シチュー", "servings": 1}),
    )
    .await;

    // 総数は検索に当てはまるレシピだけを数える。
    let body = list_page(&backend, "/api/recipes?q=curry&page=2").await;
    assert_eq!(body["total"], 22);
    assert_eq!(names_of(&body), ["Curry 2", "Curry 1"]);
}

#[tokio::test]
async fn list_orders_same_time_by_id_descending() {
    let backend = backend_with_workspaces();
    // 時計を進めずに作るので、作った日時が同じになる。
    let mut ids: Vec<String> = Vec::new();
    for _ in 0..3 {
        let recipe = create(&backend, WORKSPACE_A, &curry()).await;
        ids.push(recipe["id"].as_str().unwrap().to_owned());
    }
    ids.sort();
    ids.reverse();
    let body = list_page(&backend, "/api/recipes").await;
    let listed: Vec<&str> = body["recipes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|recipe| recipe["id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, ids);
}

#[tokio::test]
async fn list_rejects_invalid_page() {
    let backend = backend_with_workspaces();
    for page in [
        "0",
        "-1",
        "%2B1",
        "1.0",
        "1e1",
        "abc",
        "",
        "3",
        "99999999999999999999",
    ] {
        let path = format!("/api/recipes?page={page}");
        let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
        assert_error(
            &response,
            StatusCode::BAD_REQUEST,
            "invalid_field",
            Some("page"),
        );
    }
    // page が 2 つあると、どちらのページか決められないので受け付けない。
    let response = call(
        &backend,
        Method::GET,
        "/api/recipes?page=1&page=2",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "invalid_field",
        Some("page"),
    );
    // q と page の両方が正しくなければ、q のエラーを先に返す。
    let path = format!("/api/recipes?q={}&page=0", "a".repeat(101));
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "invalid_field",
        Some("q"),
    );
}

/// `GET /api/recipes?q=<q>`（q はパーセントエンコーディングにする）の結果の料理名を返す。
async fn search(backend: &FakeBackend, q: &str) -> Vec<String> {
    search_raw(backend, &urlencode(q)).await
}

/// `GET /api/recipes?q=<raw>`（raw はエンコードしない）の結果の料理名を返す。
async fn search_raw(backend: &FakeBackend, raw: &str) -> Vec<String> {
    let path = format!("/api/recipes?q={raw}");
    let response = call(backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    let mut names: Vec<String> = response.json()["recipes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|recipe| recipe["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    names
}

/// クエリの値を、パーセントエンコーディングにする（テスト用の簡単なもの）。
fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn list_filters_by_name() {
    let backend = backend_with_workspaces();
    for name in ["Beef Curry", "カレーうどん", "シチュー", "100% Juice"] {
        create(&backend, WORKSPACE_A, &json!({"name": name, "servings": 2})).await;
    }
    // 部分一致。ASCII の大文字と小文字は区別しない。
    assert_eq!(search(&backend, "curry").await, ["Beef Curry"]);
    assert_eq!(search(&backend, "CURRY").await, ["Beef Curry"]);
    assert_eq!(search(&backend, "カレー").await, ["カレーうどん"]);
    // % は文字そのものとして探す。
    assert_eq!(search(&backend, "%").await, ["100% Juice"]);
    assert_eq!(search(&backend, "ラーメン").await, Vec::<String>::new());
    // 空なら絞り込まない。
    assert_eq!(search(&backend, "").await.len(), 4);
}

#[tokio::test]
async fn list_handles_unusual_queries() {
    let backend = backend_with_workspaces();
    create(
        &backend,
        WORKSPACE_A,
        &json!({"name": "A_B", "servings": 1}),
    )
    .await;
    create(
        &backend,
        WORKSPACE_A,
        &json!({"name": "AxB", "servings": 1}),
    )
    .await;
    create(
        &backend,
        WORKSPACE_A,
        &json!({"name": "a\\b", "servings": 1}),
    )
    .await;
    // _ と \ も文字そのものとして探す。
    assert_eq!(search(&backend, "_").await, ["A_B"]);
    assert_eq!(search(&backend, "\\").await, ["a\\b"]);
    // + は空白として読む。
    create(
        &backend,
        WORKSPACE_A,
        &json!({"name": "Beef Curry", "servings": 1}),
    )
    .await;
    assert_eq!(search_raw(&backend, "f+C").await, ["Beef Curry"]);
    // UTF-8 として読めないバイトは置き換え文字になり、エラーにはしない。
    let response = call(
        &backend,
        Method::GET,
        "/api/recipes?q=%FF",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    // q が 2 つあると、どちらで探すか決められないので受け付けない。
    let response = call(
        &backend,
        Method::GET,
        "/api/recipes?q=a&q=b",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "invalid_field",
        Some("q"),
    );
}

#[tokio::test]
async fn list_rejects_too_long_query() {
    let backend = backend_with_workspaces();
    let path = format!("/api/recipes?q={}", "a".repeat(101));
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "invalid_field",
        Some("q"),
    );
}

#[tokio::test]
async fn get_returns_recipe_or_not_found() {
    let backend = backend_with_workspaces();
    let created = create(&backend, WORKSPACE_A, &curry()).await;
    let path = format!("/api/recipes/{}", created["id"].as_str().unwrap());

    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.json(), created);

    for path in [
        "/api/recipes/ffffffffffffffffffffffffffffffff",
        "/api/recipes/not-an-id",
    ] {
        let response = call(&backend, Method::GET, path, Some(WORKSPACE_A), None).await;
        assert_error(&response, StatusCode::NOT_FOUND, "recipe_not_found", None);
    }
}

#[tokio::test]
async fn update_replaces_fields_and_keeps_created_at() {
    let backend = backend_with_workspaces();
    let created = create(&backend, WORKSPACE_A, &curry()).await;
    let path = format!("/api/recipes/{}", created["id"].as_str().unwrap());
    backend.advance(1_000);

    let new_body = json!({"name": "辛口カレー", "servings": 2});
    let response = call(
        &backend,
        Method::PUT,
        &path,
        Some(WORKSPACE_A),
        Some(&new_body),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(
        response.json(),
        json!({
            "id": created["id"],
            "name": "辛口カレー",
            "servings": 2,
            // PUT は置き換えなので、省いた項目は空になる。
            "ingredients": "",
            "steps": "",
            "created_at": NOW,
            "updated_at": NOW + 1_000,
        })
    );
}

#[tokio::test]
async fn update_rejects_invalid_or_unknown() {
    let backend = backend_with_workspaces();
    let created = create(&backend, WORKSPACE_A, &curry()).await;
    let path = format!("/api/recipes/{}", created["id"].as_str().unwrap());

    let response = call(
        &backend,
        Method::PUT,
        &path,
        Some(WORKSPACE_A),
        Some(&json!({"name": "", "servings": 2})),
    )
    .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "invalid_field",
        Some("name"),
    );

    let response = call(
        &backend,
        Method::PUT,
        "/api/recipes/ffffffffffffffffffffffffffffffff",
        Some(WORKSPACE_A),
        Some(&curry()),
    )
    .await;
    assert_error(&response, StatusCode::NOT_FOUND, "recipe_not_found", None);
}

#[tokio::test]
async fn non_utf8_recipe_id_is_not_found() {
    // パスの ID が UTF-8 として読めなくても、形の正しくない ID と同じく recipe_not_found にする。
    let backend = backend_with_workspaces();
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        let request = Request::builder()
            .method(method.clone())
            .uri("/api/recipes/%ff")
            .header("x-workspace-id", WORKSPACE_A)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("not json"))
            .unwrap();
        let response = send(router(backend.clone()), request).await;
        assert_error(&response, StatusCode::NOT_FOUND, "recipe_not_found", None);
    }
}

#[tokio::test]
async fn update_checks_body_before_existence() {
    // 無い ID に、読めない本文を送ると、本文の失敗（invalid_body）を先に返す（docs/spec.md の順）。
    let backend = backend_with_workspaces();
    let request = Request::put("/api/recipes/ffffffffffffffffffffffffffffffff")
        .header("x-workspace-id", WORKSPACE_A)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("not json"))
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_error(&response, StatusCode::BAD_REQUEST, "invalid_body", None);
    // 形の正しくない ID なら、本文より先に recipe_not_found を返す。
    let request = Request::put("/api/recipes/not-an-id")
        .header("x-workspace-id", WORKSPACE_A)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("not json"))
        .unwrap();
    let response = send(router(backend), request).await;
    assert_error(&response, StatusCode::NOT_FOUND, "recipe_not_found", None);
}

#[tokio::test]
async fn delete_removes_recipe() {
    let backend = backend_with_workspaces();
    let created = create(&backend, WORKSPACE_A, &curry()).await;
    let path = format!("/api/recipes/{}", created["id"].as_str().unwrap());

    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(response.body, "");

    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_error(&response, StatusCode::NOT_FOUND, "recipe_not_found", None);
}

#[tokio::test]
async fn other_workspace_cannot_read_or_change_recipe() {
    let backend = backend_with_workspaces();
    let created = create(&backend, WORKSPACE_A, &curry()).await;
    let path = format!("/api/recipes/{}", created["id"].as_str().unwrap());

    // B の一覧には出ない。
    let response = call(
        &backend,
        Method::GET,
        "/api/recipes",
        Some(WORKSPACE_B),
        None,
    )
    .await;
    assert_eq!(
        response.json(),
        json!({"recipes": [], "total": 0, "page": 1, "per_page": 20})
    );

    // B からは、ID を知っていても読めず、書き換えられず、消せない。
    let other = json!({"name": "乗っ取り", "servings": 1});
    for (method, body) in [
        (Method::GET, None),
        (Method::PUT, Some(&other)),
        (Method::DELETE, None),
    ] {
        let response = call(&backend, method.clone(), &path, Some(WORKSPACE_B), body).await;
        assert_error(&response, StatusCode::NOT_FOUND, "recipe_not_found", None);
    }

    // A のレシピは変わっていない。
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.json(), created);
}

#[tokio::test]
async fn recipe_routes_require_workspace() {
    let backend = backend_with_workspaces();
    for (method, path) in [
        (Method::GET, "/api/recipes"),
        (Method::POST, "/api/recipes"),
        (Method::GET, "/api/recipes/ffffffffffffffffffffffffffffffff"),
        (Method::PUT, "/api/recipes/ffffffffffffffffffffffffffffffff"),
        (
            Method::DELETE,
            "/api/recipes/ffffffffffffffffffffffffffffffff",
        ),
    ] {
        let response = call(&backend, method.clone(), path, None, Some(&curry())).await;
        assert_error(
            &response,
            StatusCode::BAD_REQUEST,
            "workspace_id_missing",
            None,
        );
    }
}

#[tokio::test]
async fn recipe_routes_hide_backend_errors() {
    let backend = FakeBackend {
        broken: true,
        ..FakeBackend::default()
    };
    let response = call(
        &backend,
        Method::POST,
        "/api/recipes",
        Some(WORKSPACE_A),
        Some(&curry()),
    )
    .await;
    assert_error(
        &response,
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        None,
    );
}

#[tokio::test]
async fn create_stops_at_recipe_limit() {
    let backend = backend_with_workspaces();
    for _ in 0..MAX_RECIPES_PER_WORKSPACE {
        create(&backend, WORKSPACE_A, &curry()).await;
    }
    let response = call(
        &backend,
        Method::POST,
        "/api/recipes",
        Some(WORKSPACE_A),
        Some(&curry()),
    )
    .await;
    assert_error(
        &response,
        StatusCode::CONFLICT,
        "recipe_limit_reached",
        None,
    );
    // 上限はワークスペースごとなので、ほかのワークスペースには登録できる。
    create(&backend, WORKSPACE_B, &curry()).await;
    // 1 件消せば、また登録できる。
    let body = list_page(&backend, "/api/recipes").await;
    let id = body["recipes"][0]["id"].as_str().unwrap().to_owned();
    let response = call(
        &backend,
        Method::DELETE,
        &format!("/api/recipes/{id}"),
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    create(&backend, WORKSPACE_A, &curry()).await;
}

#[tokio::test]
async fn list_and_search_work_at_recipe_limit_with_maximum_fields() {
    let backend = backend_with_workspaces();
    // 上限いっぱいの数を、項目の長さも上限いっぱい（4 バイトの絵文字）で登録する。
    let recipe = json!({
        "name": format!("Curry{}", "🍛".repeat(95)),
        "servings": 100,
        "ingredients": "🍛".repeat(2_000),
        "steps": "🍛".repeat(5_000),
    });
    for _ in 0..MAX_RECIPES_PER_WORKSPACE {
        create(&backend, WORKSPACE_A, &recipe).await;
    }
    for path in [
        "/api/recipes",
        "/api/recipes?page=2",
        "/api/recipes?q=curry&page=2",
    ] {
        let body = list_page(&backend, path).await;
        assert_eq!(body["total"], MAX_RECIPES_PER_WORKSPACE, "{path}");
        assert_eq!(body["recipes"].as_array().unwrap().len(), 20, "{path}");
    }
}

/// 書き込む API の一覧（メソッドとパス）。
fn write_requests(id: &str) -> [(Method, String); 3] {
    [
        (Method::POST, "/api/recipes".to_owned()),
        (Method::PUT, format!("/api/recipes/{id}")),
        (Method::DELETE, format!("/api/recipes/{id}")),
    ]
}

#[tokio::test]
async fn write_apis_are_rate_limited_before_workspace_check() {
    let backend = FakeBackend {
        write_limited: true,
        ..backend_with_workspaces()
    };
    let id = "0".repeat(32);
    for (method, path) in write_requests(&id) {
        // ワークスペースの ID が無くても、先に回数の制限で断る（D1 を読ませない）。
        let request = Request::builder()
            .method(method.clone())
            .uri(&path)
            .header("cf-connecting-ip", "192.0.2.1")
            .body(Body::empty())
            .unwrap();
        let response = send(router(backend.clone()), request).await;
        assert_error(
            &response,
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            None,
        );
    }
    assert_eq!(
        *backend.write_limit_keys.lock().unwrap(),
        ["192.0.2.1", "192.0.2.1", "192.0.2.1"]
    );
    // 読むだけの API は、書き込みの回数の制限を受けない。
    let response = call(
        &backend,
        Method::GET,
        "/api/recipes",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(backend.write_limit_keys.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn content_length_within_limit_or_unreadable_is_passed_on() {
    let backend = backend_with_workspaces();
    let id = "0".repeat(32);
    for (method, path) in write_requests(&id) {
        // ちょうど上限の値と、数として読めない値は、middleware では断らない。
        // 次のワークスペースの確認まで進む（数として読めない本文は、本文を読む段階で止める）。
        for length in [
            MAX_BODY_BYTES.to_string(),
            "abc".to_owned(),
            "-1".to_owned(),
        ] {
            let request = Request::builder()
                .method(method.clone())
                .uri(&path)
                .header(header::CONTENT_LENGTH, &length)
                .body(Body::empty())
                .unwrap();
            let response = send(router(backend.clone()), request).await;
            assert_error(
                &response,
                StatusCode::BAD_REQUEST,
                "workspace_id_missing",
                None,
            );
        }
    }
}

#[tokio::test]
async fn too_large_content_length_is_rejected_before_reading() {
    let backend = backend_with_workspaces();
    let id = "0".repeat(32);
    for (method, path) in write_requests(&id) {
        for length in [
            (MAX_BODY_BYTES + 1).to_string(),
            "99999999999999999999999".to_owned(),
        ] {
            // ワークスペースの ID が無くても、本文が無くても、Content-Length だけで断る。
            let request = Request::builder()
                .method(method.clone())
                .uri(&path)
                .header(header::CONTENT_LENGTH, &length)
                .body(Body::empty())
                .unwrap();
            let response = send(router(backend.clone()), request).await;
            assert_error(
                &response,
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                None,
            );
        }
    }
}
