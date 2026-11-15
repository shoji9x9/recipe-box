//! 写真の API（`/api/recipes/{id}/photos`）のテスト。

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};

use super::{
    TestResponse, WORKSPACE_A, WORKSPACE_B, call,
    fake::{FakeBackend, NOW},
    send,
};
use crate::{
    photo::{
        MAX_PHOTO_BYTES, MAX_PHOTO_BYTES_PER_WORKSPACE, MAX_PHOTO_BYTES_TOTAL,
        MAX_PHOTO_UPLOADS_PER_DAY, MAX_PHOTOS_PER_RECIPE,
    },
    router,
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

#[tokio::test]
async fn app_wide_photo_bytes_boundary() {
    let (backend, recipe) = setup().await;
    let recipe_b = create_recipe(&backend, WORKSPACE_B).await;
    // ほかのワークスペースの写真で、アプリ全体の合計を上限の 10 バイト手前まで埋める。
    backend.add_photo_row(
        WORKSPACE_B,
        &recipe_b,
        "11111111111111111111111111111111",
        MAX_PHOTO_BYTES_TOTAL - 10,
    );
    let response = upload(&backend, WORKSPACE_A, &recipe, jpeg(11)).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&response), "capacity_reached");
    assert!(backend.objects.lock().unwrap().is_empty());
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
    assert!(
        backend
            .injected_photo
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    // 後から追加された写真のデータも、R2 に残さない。
    assert!(backend.objects.lock().unwrap().is_empty());
    assert!(backend.photos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_r2_delete_keeps_rows_so_it_can_be_retried() {
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
    // 写真を 1 枚消すときも、レシピを消すときも、R2 を先に消すので、失敗すれば D1 の行が残る。
    let photo = format!("/api/recipes/{recipe}/photos/{id}");
    let response = call(&broken, Method::DELETE, &photo, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    let path = format!("/api/recipes/{recipe}");
    let response = call(&broken, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(backend.photos.lock().unwrap().len(), 1);
    assert_eq!(backend.objects.lock().unwrap().len(), 1);

    // R2 が直れば、やり直して消せる。
    let response = call(&backend, Method::DELETE, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(backend.objects.lock().unwrap().is_empty());
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
