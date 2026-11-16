//! 写真の API（`/api/recipes/{id}/photos`）のテスト。

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

use super::{
    TestResponse, WORKSPACE_A, WORKSPACE_B, call,
    fake::{FakeBackend, NOW},
    send,
};
use crate::{
    error::ApiError,
    photo::{
        MAX_PHOTO_BYTES, MAX_PHOTO_BYTES_PER_WORKSPACE, MAX_PHOTO_BYTES_TOTAL,
        MAX_PHOTO_UPLOADS_PER_DAY, MAX_PHOTOS_PER_RECIPE,
    },
    router,
    workspace::MAX_WORKSPACES,
};

const DEMO: &str = "dddddddddddddddddddddddddddddddd";
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// JPEG の先頭のバイトで始まり、全体で `size` バイトのデータ。
fn jpeg(size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    bytes[..3].copy_from_slice(&[0xff, 0xd8, 0xff]);
    bytes
}

/// ワークスペース A と B と、A のレシピ 1 件を持つ偽物を作り、レシピの ID を返す。
async fn setup() -> (FakeBackend, String) {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..FakeBackend::default()
    };
    backend.add_workspace(WORKSPACE_A, NOW);
    backend.add_workspace(WORKSPACE_B, NOW);
    backend.add_workspace(DEMO, NOW);
    let recipe = create_recipe(&backend, WORKSPACE_A).await;
    (backend, recipe)
}

async fn create_recipe(backend: &FakeBackend, workspace: &str) -> String {
    let response = call(
        backend,
        Method::POST,
        "/api/recipes",
        Some(workspace),
        Some(&json!({"name": "カレー", "servings": 2})),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    response.json()["id"].as_str().unwrap().to_owned()
}

/// 写真を本文のバイト列のまま送る。
async fn upload(
    backend: &FakeBackend,
    workspace: &str,
    recipe: &str,
    bytes: Vec<u8>,
) -> TestResponse {
    let request = Request::post(format!("/api/recipes/{recipe}/photos"))
        .header("x-workspace-id", workspace)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes))
        .unwrap();
    send(router(backend.clone()), request).await
}

fn error_code(response: &TestResponse) -> String {
    response.json()["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn photo_can_be_added_listed_read_and_deleted() {
    let (backend, recipe) = setup().await;
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(b"rest");

    let response = upload(&backend, WORKSPACE_A, &recipe, png.clone()).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    let photo: Value = response.json();
    assert_eq!(photo["content_type"], "image/png");
    assert_eq!(photo["size"], 12);
    assert_eq!(photo["created_at"], NOW);
    let id = photo["id"].as_str().unwrap().to_owned();

    let path = format!("/api/recipes/{recipe}/photos");
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.json(), json!({"photos": [photo]}));

    let path = format!("/api/recipes/{recipe}/photos/{id}");
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.content_type, "image/png");
    assert_eq!(response.bytes, png);
    assert_eq!(response.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert!(
        response.headers[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .starts_with("private")
    );

    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(backend.objects.lock().unwrap().is_empty());
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response), "photo_not_found");
}

#[tokio::test]
async fn photo_size_boundary() {
    let (backend, recipe) = setup().await;
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(MAX_PHOTO_BYTES)).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);

    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(MAX_PHOTO_BYTES + 1)).await;
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_code(&response), "payload_too_large");

    // Content-Length の無い本文も、読むときに上限で止める。
    let request = Request::post(format!("/api/recipes/{recipe}/photos"))
        .header("x-workspace-id", WORKSPACE_A)
        .body(Body::from(jpeg(MAX_PHOTO_BYTES + 1)))
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn only_jpeg_png_and_webp_are_accepted() {
    let (backend, recipe) = setup().await;
    let mut webp = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
    webp.extend_from_slice(b"VP8 ");
    let response = upload(&backend, WORKSPACE_A, &recipe, webp).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    assert_eq!(response.json()["content_type"], "image/webp");

    // 宣言（Content-Type: image/jpeg）ではなく、中身で決める。
    for bytes in [b"GIF89a....".to_vec(), b"<svg/>".to_vec(), Vec::new()] {
        let response = upload(&backend, WORKSPACE_A, &recipe, bytes).await;
        assert_eq!(response.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(error_code(&response), "unsupported_photo_type");
    }
    assert_eq!(backend.photo_uploads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn sixth_photo_of_a_recipe_is_rejected() {
    let (backend, recipe) = setup().await;
    for _ in 0..MAX_PHOTOS_PER_RECIPE {
        let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
        assert_eq!(response.status, StatusCode::CREATED);
    }
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert_eq!(error_code(&response), "photo_limit_reached");
    // 上限で断った追加は R2 に書かず、回数にも数えない。
    assert_eq!(backend.objects.lock().unwrap().len(), 5);
    assert_eq!(backend.photo_uploads.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn workspace_photo_bytes_boundary() {
    let (backend, recipe) = setup().await;
    let other = create_recipe(&backend, WORKSPACE_A).await;
    // ほかのレシピの写真で、ワークスペースの合計を上限の 10 バイト手前まで埋める。
    backend.add_photo_row(
        WORKSPACE_A,
        &other,
        "11111111111111111111111111111111",
        MAX_PHOTO_BYTES_PER_WORKSPACE - 10,
    );
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(11)).await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert_eq!(error_code(&response), "photo_storage_limit_reached");
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::CREATED);

    // ほかのワークスペースは、この上限に数えない。
    let recipe_b = create_recipe(&backend, WORKSPACE_B).await;
    let response = upload(&backend, WORKSPACE_B, &recipe_b, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::CREATED);
}

#[test]
fn workspace_photo_bytes_keep_app_wide_total() {
    // アプリ全体の合計は追加のときに数えず、ワークスペースの数とワークスペースごとの合計の上限で守る。
    // 値は docs/spec.md と docs/security.md に書いている。変えたら文書も直す。
    assert_eq!(MAX_PHOTO_BYTES_PER_WORKSPACE, 50_000_000);
    assert_eq!(MAX_PHOTO_BYTES_TOTAL, 5_000_000_000);
    // MAX_WORKSPACES × MAX_PHOTO_BYTES_PER_WORKSPACE が合計を超えないことは、photo.rs でコンパイルのときに確かめる。
    assert_eq!(MAX_WORKSPACES, 100);
    // 利用者に見せる文の値も同じにする。
    let megabytes = MAX_PHOTO_BYTES_PER_WORKSPACE / 1_000_000;
    let (_, _, message) = ApiError::PhotoStorageLimitReached.parts();
    assert!(
        message.contains(&format!("上限（{megabytes} MB）")),
        "{message}"
    );
}

#[tokio::test]
async fn other_workspaces_photo_bytes_not_counted() {
    let (backend, recipe) = setup().await;
    let recipe_b = create_recipe(&backend, WORKSPACE_B).await;
    // ほかのワークスペースの写真の合計がアプリ全体の上限に達していても、断らない（数えるのはワークスペースの合計だけ）。
    backend.add_photo_row(
        WORKSPACE_B,
        &recipe_b,
        "11111111111111111111111111111111",
        MAX_PHOTO_BYTES_TOTAL,
    );
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::CREATED);
}

#[tokio::test]
async fn app_wide_daily_uploads_boundary() {
    let (backend, recipe) = setup().await;
    // 今日（UTC）の追加を上限の 1 回手前まで、昨日の追加をたくさん記録しておく。
    let today = NOW - NOW.rem_euclid(DAY_MILLIS);
    {
        let mut uploads = backend.photo_uploads.lock().unwrap();
        uploads.extend(std::iter::repeat_n(today - 1, 1_000));
        uploads.extend(std::iter::repeat_n(
            today,
            usize::try_from(MAX_PHOTO_UPLOADS_PER_DAY - 1).unwrap(),
        ));
    }
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::CREATED);

    // 消しても回数は戻らない。
    let id = response.json()["id"].as_str().unwrap().to_owned();
    let path = format!("/api/recipes/{recipe}/photos/{id}");
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&response), "capacity_reached");

    // 次の日（UTC）には、また追加できる。
    backend.advance(DAY_MILLIS);
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::CREATED);
}

#[tokio::test]
async fn other_workspace_cannot_see_or_delete_photos() {
    let (backend, recipe) = setup().await;
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    let id = response.json()["id"].as_str().unwrap().to_owned();

    let list = format!("/api/recipes/{recipe}/photos");
    let photo = format!("/api/recipes/{recipe}/photos/{id}");
    for (method, path) in [
        (Method::GET, &list),
        (Method::GET, &photo),
        (Method::DELETE, &photo),
    ] {
        let response = call(&backend, method.clone(), path, Some(WORKSPACE_B), None).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    // B から A のレシピに写真を足すこともできない。
    let response = upload(&backend, WORKSPACE_B, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response), "recipe_not_found");
    assert_eq!(backend.objects.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn missing_and_malformed_ids_are_not_found() {
    let (backend, recipe) = setup().await;
    let cases = [
        (
            "/api/recipes/cccccccccccccccccccccccccccccccc/photos",
            "recipe_not_found",
        ),
        ("/api/recipes/xyz/photos", "recipe_not_found"),
        (
            &*format!("/api/recipes/{recipe}/photos/cccccccccccccccccccccccccccccccc"),
            "photo_not_found",
        ),
        (
            &*format!("/api/recipes/{recipe}/photos/xyz"),
            "photo_not_found",
        ),
    ];
    for (path, code) in cases {
        let response = call(&backend, Method::GET, path, Some(WORKSPACE_A), None).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(error_code(&response), code, "{path}");
    }
}

#[tokio::test]
async fn demo_photos_cannot_be_added_or_deleted_but_can_be_read() {
    let (backend, _) = setup().await;
    let recipe = create_recipe(&backend, DEMO).await;
    // seed は R2 と D1 に直接入れる。
    backend.add_photo_row(DEMO, &recipe, "11111111111111111111111111111111", 3);
    backend.objects.lock().unwrap().insert(
        format!("photos/{DEMO}/{recipe}/11111111111111111111111111111111"),
        (jpeg(3), "image/jpeg".to_owned()),
    );

    let response = upload(&backend, DEMO, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&response), "demo_photos_disabled");
    let path = format!("/api/recipes/{recipe}/photos/11111111111111111111111111111111");
    let response = call(&backend, Method::DELETE, &path, Some(DEMO), None).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let response = call(&backend, Method::GET, &path, Some(DEMO), None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.bytes, jpeg(3));
}

#[tokio::test]
async fn deleting_a_recipe_deletes_its_photos() {
    let (backend, recipe) = setup().await;
    let other = create_recipe(&backend, WORKSPACE_A).await;
    upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    upload(&backend, WORKSPACE_A, &other, jpeg(10)).await;

    let path = format!("/api/recipes/{recipe}");
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    // 消したレシピの写真だけが、R2 と D1 から消える。
    let objects = backend.objects.lock().unwrap();
    assert_eq!(objects.len(), 1);
    assert!(objects.keys().all(|key| key.contains(&other)));
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn photo_added_while_deleting_its_recipe_is_removed_from_r2() {
    let (backend, recipe) = setup().await;
    upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    // 最初の R2 の削除の後、D1 でレシピを消す前に、別のリクエストが写真を追加した。
    let backend = FakeBackend {
        photo_added_after_first_delete: true,
        ..backend
    };
    let path = format!("/api/recipes/{recipe}");
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(backend.injected_photo.load(Ordering::Relaxed));
    // 後から追加された写真のデータも、R2 に残さない。
    assert!(backend.objects.lock().unwrap().is_empty());
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_first_r2_delete_keeps_rows_so_it_can_be_retried() {
    let (backend, recipe) = setup().await;
    let id = upload(&backend, WORKSPACE_A, &recipe, jpeg(10))
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let broken = FakeBackend {
        broken_r2_delete: true,
        ..backend.clone()
    };
    // 写真を 1 枚消すときも、レシピを消すときも、D1 の行を消す前に R2 を消すので、そこで失敗すれば D1 の行が残る。
    // （レシピの D1 を消した後の、2 回目の R2 の削除の失敗は、やり直せない。PR のコメントに書いた。）
    // 写真を 1 枚消すときは、印を外して元に戻す。
    let photo = format!("/api/recipes/{recipe}/photos/{id}");
    let response = call(&broken, Method::DELETE, &photo, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(listed_photos(&backend, &recipe).await, 1);
    // レシピを消すときは、写真を隠したまま残す（R2 のデータが一部だけ消えていても、データの無い写真を見せない）。
    let path = format!("/api/recipes/{recipe}");
    let response = call(&broken, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
    assert_eq!(backend.objects.lock().unwrap().len(), 1);
    assert_eq!(listed_photos(&backend, &recipe).await, 0);

    // R2 が直れば、やり直して消せる。
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(backend.objects.lock().unwrap().is_empty());
    assert!(backend.photos.lock().unwrap().is_empty());
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn failed_r2_write_leaves_no_photo_row() {
    let (backend, recipe) = setup().await;
    let backend = FakeBackend {
        broken_r2_put: true,
        ..backend
    };
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(backend.photos.lock().unwrap().is_empty());
    // 追加の記録は残し、1 日の回数に数える。
    assert_eq!(backend.photo_uploads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn photo_task_dropped_by_the_runtime_is_an_internal_error() {
    let (backend, recipe) = setup().await;
    let backend = FakeBackend {
        task_dropped: true,
        ..backend
    };
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response.json()["error"],
        "internal_error",
        "{}",
        response.body
    );
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn photo_whose_row_vanishes_while_writing_is_removed_from_r2() {
    let (backend, recipe) = setup().await;
    let backend = FakeBackend {
        photo_rows_vanish_on_put: true,
        ..backend
    };
    // R2 に書いている間に、別のリクエストが写真の行を消した（レシピは残っている）。
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response), "photo_not_found");
    // 行がたどれないデータを R2 に残さない（アプリ全体の合計に数えられない量を作らせない）。
    assert!(backend.objects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn photo_writes_use_the_write_limit() {
    let (backend, recipe) = setup().await;
    let backend = FakeBackend {
        write_limited: true,
        ..backend
    };
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    // 読むだけの API は、書き込みの回数の制限を使わない。
    let path = format!("/api/recipes/{recipe}/photos");
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::OK);
}

#[tokio::test]
async fn deleting_a_workspace_deletes_its_recipes_and_photos() {
    let (backend, recipe) = setup().await;
    let other = create_recipe(&backend, WORKSPACE_A).await;
    let kept = create_recipe(&backend, WORKSPACE_B).await;
    upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    upload(&backend, WORKSPACE_A, &other, jpeg(10)).await;
    upload(&backend, WORKSPACE_B, &kept, jpeg(10)).await;

    let response = call(
        &backend,
        Method::DELETE,
        "/api/workspaces",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT, "{}", response.body);
    // ワークスペース A のレシピと写真だけが、D1 と R2 から消える。
    assert!(!backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
    assert!(
        backend
            .recipes
            .lock()
            .unwrap()
            .keys()
            .all(|(workspace, _)| workspace != WORKSPACE_A)
    );
    {
        let objects = backend.objects.lock().unwrap();
        assert_eq!(objects.len(), 1);
        assert!(objects.keys().all(|key| key.contains(&kept)));
    }
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
    // 消したワークスペースの ID は、もう使えない。
    let response = call(
        &backend,
        Method::DELETE,
        "/api/workspaces",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response), "workspace_not_found");
}

#[tokio::test]
async fn demo_workspace_cannot_be_deleted() {
    let (backend, _) = setup().await;
    let response = call(
        &backend,
        Method::DELETE,
        "/api/workspaces",
        Some(DEMO),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&response), "demo_workspace_protected");
    assert!(backend.workspaces.lock().unwrap().contains_key(DEMO));
}

#[tokio::test]
async fn photo_added_while_deleting_its_workspace_is_removed_from_r2() {
    let (backend, recipe) = setup().await;
    upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    // 最初の R2 の削除の後、D1 でワークスペースを消す前に、別のリクエストが写真を追加した。
    let backend = FakeBackend {
        photo_added_after_first_delete: true,
        ..backend
    };
    let response = call(
        &backend,
        Method::DELETE,
        "/api/workspaces",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(backend.injected_photo.load(Ordering::Relaxed));
    assert!(backend.objects.lock().unwrap().is_empty());
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_first_r2_delete_keeps_the_workspace_so_it_can_be_retried() {
    let (backend, recipe) = setup().await;
    upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    let broken = FakeBackend {
        broken_r2_delete: true,
        ..backend.clone()
    };
    let response = call(
        &broken,
        Method::DELETE,
        "/api/workspaces",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
    assert_eq!(backend.objects.lock().unwrap().len(), 1);

    // R2 が直れば、やり直して消せる。
    let response = call(
        &backend,
        Method::DELETE,
        "/api/workspaces",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(backend.objects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn deleting_a_workspace_uses_the_write_limit() {
    let (backend, _) = setup().await;
    let backend = FakeBackend {
        write_limited: true,
        ..backend
    };
    let response = call(
        &backend,
        Method::DELETE,
        "/api/workspaces",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
    // 作るほうは、専用のレート制限だけを使う。
    let response = send(
        router(backend.clone()),
        Request::post("/api/workspaces")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED);
}

// 書き込みか削除の途中で失敗して、印（pending）の付いた行が残る場合（Issue #48）。

/// 自動削除が、印の付いた行を片付けるまでの時間を過ぎるよう、時計を進める。
fn pass_cleanup_time(backend: &FakeBackend) {
    backend.advance(crate::photo::PENDING_CLEANUP_AFTER_MILLIS + 1);
}

/// レシピの写真の一覧（API）の写真の数。
async fn listed_photos(backend: &FakeBackend, recipe: &str) -> usize {
    let path = format!("/api/recipes/{recipe}/photos");
    let response = call(backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    response.json()["photos"].as_array().unwrap().len()
}

/// 写真の行の ID（行は 1 つだけの前提）。
fn only_photo_id(backend: &FakeBackend) -> String {
    let photos = backend.photos.lock().unwrap();
    assert_eq!(photos.len(), 1);
    photos[0].2.id.clone()
}

#[tokio::test]
async fn row_left_by_failed_write_is_hidden_and_cleaned_up() {
    let (backend, recipe) = setup().await;
    // R2 の書き込みが失敗し、後始末の D1 の削除も失敗した。
    let broken = FakeBackend {
        broken_r2_put: true,
        broken_photo_delete: true,
        ..backend.clone()
    };
    let response = upload(&broken, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    let id = only_photo_id(&backend);
    assert!(backend.is_pending(&id));
    // 読めない写真は、一覧にも出さず、データも返さない。
    assert_eq!(listed_photos(&backend, &recipe).await, 0);
    let path = format!("/api/recipes/{recipe}/photos/{id}");
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    // 利用者からは消せない（見えない）が、自動削除が片付ける。
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);

    pass_cleanup_time(&backend);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 1);
    assert!(!summary.has_failure());
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn row_left_by_failed_finish_is_cleaned_up_with_its_data() {
    let (backend, recipe) = setup().await;
    // R2 には書けたが、D1 の印を外すのが失敗した（Worker が止められたときも、同じく印が残る）。
    let broken = FakeBackend {
        broken_photo_pending: true,
        ..backend.clone()
    };
    let response = upload(&broken, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    let id = only_photo_id(&backend);
    assert!(backend.is_pending(&id));
    assert_eq!(backend.objects.lock().unwrap().len(), 1);
    assert_eq!(listed_photos(&backend, &recipe).await, 0);

    pass_cleanup_time(&backend);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 1);
    assert!(backend.photos.lock().unwrap().is_empty());
    assert!(backend.objects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn photo_whose_delete_failed_is_hidden_and_cleaned_up() {
    let (backend, recipe) = setup().await;
    let id = upload(&backend, WORKSPACE_A, &recipe, jpeg(10))
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!backend.is_pending(&id));
    // R2 は消せたが、D1 の行を消すのが失敗した。
    let broken = FakeBackend {
        broken_photo_delete: true,
        ..backend.clone()
    };
    let path = format!("/api/recipes/{recipe}/photos/{id}");
    let response = call(&broken, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(backend.objects.lock().unwrap().is_empty());
    assert!(backend.is_pending(&id));
    assert_eq!(listed_photos(&backend, &recipe).await, 0);

    pass_cleanup_time(&backend);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 1);
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pending_rows_count_toward_limits_until_cleaned_up() {
    let (backend, recipe) = setup().await;
    // 途中で残った行で、レシピの写真が上限まである。
    for i in 0..MAX_PHOTOS_PER_RECIPE {
        let id = format!("{i:032x}");
        backend.add_photo_row(WORKSPACE_A, &recipe, &id, 1);
        backend.mark_pending(&id, NOW);
    }
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    // 片付けた後は、また追加できる。
    pass_cleanup_time(&backend);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(
        summary.pending_photos.cleaned,
        usize::try_from(MAX_PHOTOS_PER_RECIPE).unwrap()
    );
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
}

#[tokio::test]
async fn recent_pending_rows_are_not_cleaned_up() {
    let (backend, recipe) = setup().await;
    // 追加の途中かもしれない行（印を付けてから 1 時間たっていない）は片付けない。
    backend.add_photo_row(WORKSPACE_A, &recipe, "0123456789abcdef0123456789abcdef", 1);
    backend.mark_pending("0123456789abcdef0123456789abcdef", NOW);
    backend.advance(crate::photo::PENDING_CLEANUP_AFTER_MILLIS);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 0);
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn failed_cleanup_is_reported_and_retried_next_time() {
    let (backend, recipe) = setup().await;
    backend.add_photo_row(WORKSPACE_A, &recipe, "0123456789abcdef0123456789abcdef", 1);
    backend.mark_pending("0123456789abcdef0123456789abcdef", NOW);
    pass_cleanup_time(&backend);
    let broken = FakeBackend {
        broken_r2_delete: true,
        ..backend.clone()
    };
    let summary = crate::workspace::delete_unused(&broken).await;
    assert_eq!(summary.pending_photos.failed, 1);
    assert!(summary.has_failure());
    // R2 を消せなければ、行を残し、次の日にもう一度試す。
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 1);
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn photo_whose_r2_delete_failed_stays_visible_and_can_be_deleted_again() {
    let (backend, recipe) = setup().await;
    let id = upload(&backend, WORKSPACE_A, &recipe, jpeg(10))
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let broken = FakeBackend {
        broken_r2_delete: true,
        ..backend.clone()
    };
    let path = format!("/api/recipes/{recipe}/photos/{id}");
    let response = call(&broken, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    // R2 のデータが残っているので、印を外して元に戻し、もう一度消せるようにする。
    assert!(!backend.is_pending(&id));
    assert_eq!(listed_photos(&backend, &recipe).await, 1);
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(backend.objects.lock().unwrap().is_empty());
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cleanup_counts_time_from_when_the_row_was_marked() {
    let (backend, recipe) = setup().await;
    // 何日も前に追加した写真を、今まさに消している（印を付けたのは今）。
    let id = "0123456789abcdef0123456789abcdef";
    backend.add_photo_row_at(WORKSPACE_A, &recipe, id, 1, NOW - 3 * DAY_MILLIS);
    backend.mark_pending(id, NOW);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 0);
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn deleting_a_recipe_removes_the_data_of_its_pending_photos() {
    let (backend, recipe) = setup().await;
    // R2 には書けたが、印を外せなかった写真（画面と API には出ない）。
    let broken = FakeBackend {
        broken_photo_pending: true,
        ..backend.clone()
    };
    let response = upload(&broken, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(backend.objects.lock().unwrap().len(), 1);
    // レシピを消すと、印の付いた写真の行とデータも消える。
    let path = format!("/api/recipes/{recipe}");
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(backend.objects.lock().unwrap().is_empty());
    assert!(backend.photos.lock().unwrap().is_empty());
    assert!(backend.pending_photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_row_deletion_in_cleanup_keeps_all_rows_for_the_next_run() {
    let (backend, recipe) = setup().await;
    for i in 0..3 {
        let id = format!("{i:032x}");
        backend.add_photo_row(WORKSPACE_A, &recipe, &id, 1);
        backend.mark_pending(&id, NOW);
    }
    pass_cleanup_time(&backend);
    // R2 は消せたが、D1 の行をまとめて消すのが失敗した。まとめて消すので、どの行も消えない。
    let broken = FakeBackend {
        broken_photo_delete: true,
        ..backend.clone()
    };
    let summary = crate::workspace::delete_unused(&broken).await;
    assert_eq!(summary.pending_photos.failed, 3);
    assert_eq!(summary.pending_photos.cleaned, 0);
    assert_eq!(backend.photos.lock().unwrap().len(), 3);
    // 次の日に、もう一度試して片付ける。
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 3);
    assert!(backend.photos.lock().unwrap().is_empty());
    assert!(backend.pending_photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cleanup_handles_more_rows_than_one_delete_can_take() {
    let (backend, recipe) = setup().await;
    // R2 と D1 を 1,000 行ずつまとめて消すので、2 回に分かれる数を入れる。
    for i in 0..1_001 {
        let id = format!("{i:032x}");
        backend.add_photo_row(WORKSPACE_A, &recipe, &id, 1);
        backend.mark_pending(&id, NOW);
    }
    pass_cleanup_time(&backend);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 1_001);
    assert_eq!(summary.pending_photos.failed, 0);
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cleanup_keeps_rows_whose_mark_changed_after_listing() {
    let (backend, recipe) = setup().await;
    let id = "0123456789abcdef0123456789abcdef";
    backend.add_photo_row(WORKSPACE_A, &recipe, id, 1);
    backend.mark_pending(id, NOW);
    pass_cleanup_time(&backend);
    // 選んだ後に印が外れ（長い追加が終わった）、付け直された（今、消している）行は、まとめて消すときに消さない。
    let photo = crate::photo::PhotoId::parse(id).unwrap();
    let rows = [(
        crate::workspace::WorkspaceId::parse(WORKSPACE_A).unwrap(),
        crate::recipe::RecipeId::parse(&recipe).unwrap(),
        photo,
    )];
    backend.mark_pending(id, crate::backend::Backend::now_millis(&backend));
    crate::backend::Backend::delete_photos(&backend, &rows, NOW + 1)
        .await
        .unwrap();
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
    assert!(backend.is_pending(id));
}

// レシピかワークスペースの削除の途中で失敗した場合。写真を先に隠すので、データの無い写真は見えない。

#[tokio::test]
async fn recipe_whose_row_deletion_failed_hides_its_photos_until_cleanup() {
    let (backend, recipe) = setup().await;
    upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    // R2 は消せたが、D1 でレシピを消すのが失敗した。
    let broken = FakeBackend {
        broken_owner_delete: true,
        ..backend.clone()
    };
    let path = format!("/api/recipes/{recipe}");
    let response = call(&broken, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    // レシピは残り、R2 のデータの無い写真は、一覧にも写真の API にも出ない。
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(backend.objects.lock().unwrap().is_empty());
    assert_eq!(listed_photos(&backend, &recipe).await, 0);
    let id = only_photo_id(&backend);
    let photo = format!("/api/recipes/{recipe}/photos/{id}");
    let response = call(&backend, Method::GET, &photo, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    // 隠れた行は、時間がたった後の自動削除が片付ける。
    pass_cleanup_time(&backend);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 1);
    assert!(backend.photos.lock().unwrap().is_empty());
    // レシピは、もう一度消せる。
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn recipe_deleted_again_after_failure_removes_hidden_photos_at_once() {
    let (backend, recipe) = setup().await;
    upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    let broken = FakeBackend {
        broken_owner_delete: true,
        ..backend.clone()
    };
    let path = format!("/api/recipes/{recipe}");
    let response = call(&broken, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    // 片付けを待たずに消し直しても、隠れた写真の行とデータが残らない。
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(backend.photos.lock().unwrap().is_empty());
    assert!(backend.pending_photos.lock().unwrap().is_empty());
    assert!(backend.objects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn workspace_whose_row_deletion_failed_hides_its_photos_until_cleanup() {
    let (backend, recipe) = setup().await;
    upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    let kept = create_recipe(&backend, WORKSPACE_B).await;
    upload(&backend, WORKSPACE_B, &kept, jpeg(10)).await;
    // R2 は消せたが、D1 でワークスペースを消すのが失敗した。
    let broken = FakeBackend {
        broken_owner_delete: true,
        ..backend.clone()
    };
    let response = call(
        &broken,
        Method::DELETE,
        "/api/workspaces",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    // ワークスペースとレシピは残り、データの無い写真は見えない。ほかのワークスペースの写真には触れない。
    assert!(backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
    assert_eq!(listed_photos(&backend, &recipe).await, 0);
    {
        let objects = backend.objects.lock().unwrap();
        assert_eq!(objects.len(), 1);
        assert!(objects.keys().all(|key| key.contains(&kept)));
    }
    assert_eq!(backend.pending_photos.lock().unwrap().len(), 1);
    pass_cleanup_time(&backend);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 1);
    // ワークスペースは、もう一度消せる。
    let response = call(
        &backend,
        Method::DELETE,
        "/api/workspaces",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(!backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
}

#[tokio::test]
async fn delete_task_dropped_by_the_runtime_is_an_internal_error() {
    let (backend, recipe) = setup().await;
    let dropped = FakeBackend {
        task_dropped: true,
        ..backend.clone()
    };
    let path = format!("/api/recipes/{recipe}");
    let response = call(&dropped, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error_code(&response), "internal_error");
    let response = call(
        &dropped,
        Method::DELETE,
        "/api/workspaces",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error_code(&response), "internal_error");
}

#[tokio::test]
async fn photo_remarked_by_a_deletion_while_writing_stays_hidden() {
    let (backend, recipe) = setup().await;
    // R2 に書いている間に、レシピの削除が始まり、写真の行に印を付け直した（その削除は、まだ終わっていない）。
    let racing = FakeBackend {
        photo_rows_remarked_on_put: true,
        ..backend.clone()
    };
    let response = upload(&racing, WORKSPACE_A, &recipe, jpeg(10)).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response), "photo_not_found");
    // 追加の側は、付け直された印を外さず、R2 のデータを消す。削除が途中で失敗しても、データの無い写真は見えない。
    assert!(backend.objects.lock().unwrap().is_empty());
    assert_eq!(listed_photos(&backend, &recipe).await, 0);
    assert_eq!(backend.pending_photos.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn photo_whose_delete_failed_stays_hidden_if_a_deletion_remarked_it() {
    let (backend, recipe) = setup().await;
    upload(&backend, WORKSPACE_A, &recipe, jpeg(10)).await;
    let id = only_photo_id(&backend);
    // 写真を 1 枚消している間に、レシピの削除が始まって印を付け直し、こちらの R2 の削除は失敗した。
    let racing = FakeBackend {
        photo_rows_remarked_on_delete: true,
        broken_r2_delete: true,
        ..backend.clone()
    };
    let path = format!("/api/recipes/{recipe}/photos/{id}");
    let response = call(&racing, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    // 付け直された印は外さない（レシピの削除が途中で失敗しても、データの無い写真を見せない）。
    assert!(backend.is_pending(&id));
    assert_eq!(listed_photos(&backend, &recipe).await, 0);
}

#[tokio::test]
async fn pending_rows_that_keep_failing_do_not_block_the_others() {
    let (backend, recipe) = setup().await;
    // 1 回に片付ける 2 つのまとまり（2,000 行）の、それぞれの先頭の行が、消すたびに失敗する。
    for i in 0..5_000 {
        let id = format!("{i:032x}");
        backend.add_photo_row(WORKSPACE_A, &recipe, &id, 1);
        backend.mark_pending(&id, NOW);
    }
    let backend = FakeBackend {
        undeletable_photos: vec![format!("{:032x}", 0), format!("{:032x}", 1_000)],
        ..backend
    };
    pass_cleanup_time(&backend);
    // 実行ごとに選び始める位置をずらすので、ほかの行も片付く。
    for _ in 0..6 {
        crate::workspace::delete_unused(&backend).await;
        backend.advance(crate::workspace::CLEANUP_INTERVAL_MILLIS);
    }
    // 位置をずらさないと、毎回先頭の 2,000 行（失敗する 2 つのまとまり）だけを選び、後ろの行がずっと残る。
    // 失敗する行と同じ batch に入った行は、batch ごと残る（1 回の呼び出しで、まとめて消すため）。
    let rows = backend.photos.lock().unwrap();
    let later = format!("{:032x}", 2_000);
    assert!(rows.iter().all(|(_, _, p)| p.id < later), "{}", rows.len());
    assert!(rows.iter().any(|(_, _, p)| p.id == format!("{:032x}", 0)));
}
