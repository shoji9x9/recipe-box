//! 写真の小さな版（画面だけ。Issue #52）のテスト。足す・出す・上限と形の誤り・消すときに一緒に消えること。

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};

use super::{
    TestResponse, WORKSPACE_A, WORKSPACE_B,
    fake::{FakeBackend, NOW},
    jpeg,
    screens::{
        DEMO, HOST, backend_with_workspaces, cookie, create_recipe, post_photo, post_raw_photo,
        screen_get, screen_post,
    },
    send,
};
use crate::{
    photo::{MAX_PHOTO_BYTES_PER_WORKSPACE, thumbnail_key},
    recipe::RecipeId,
    router,
    thumbnail::{MAX_THUMBNAIL_BYTES, tests::canvas_jpeg},
    workspace::WorkspaceId,
};

/// 元の写真（JPEG として読める 100 バイトのデータ）を、JavaScript と同じ形で足し、写真の ID を返す。
async fn add_photo(backend: &FakeBackend, recipe_path: &str) -> String {
    let photo = jpeg(100);
    let response = post_raw_photo(
        backend,
        recipe_path,
        &cookie(WORKSPACE_A),
        "image/jpeg",
        photo,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT, "{}", response.body);
    response.headers["x-photo-id"].to_str().unwrap().to_owned()
}

/// 小さな版を送る。
async fn post_thumbnail(
    backend: &FakeBackend,
    recipe_path: &str,
    photo: &str,
    query: &str,
    bytes: Vec<u8>,
    workspace: &str,
) -> TestResponse {
    let mut request = Request::post(format!("{recipe_path}/photos/{photo}/thumbnail{query}"))
        .header(header::HOST, HOST)
        .header(header::ORIGIN, format!("https://{HOST}"))
        .header("cf-connecting-ip", "192.0.2.1");
    // 共有のデモには、Cookie の無い人として送る（デモの ID の Cookie は、無い ID と同じく書き込みを断るため）。
    if workspace != DEMO {
        request = request.header(header::COOKIE, cookie(workspace));
    }
    let request = request
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes))
        .unwrap();
    send(router(backend.clone()), request).await
}

fn thumbnail_r2_key(recipe_path: &str, photo: &str) -> String {
    let recipe = RecipeId::parse(recipe_path.trim_start_matches("/recipes/")).unwrap();
    thumbnail_key(&WorkspaceId::parse(WORKSPACE_A).unwrap(), &recipe, photo)
}

#[tokio::test]
async fn thumbnail_is_added_and_shown_with_srcset() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let photo = add_photo(&backend, &path).await;

    // 小さな版の無い写真は、元の写真を出す。
    let detail = screen_get(&backend, &path, Some(&a)).await;
    assert!(
        detail
            .body
            .contains(&format!(r#"src="{path}/photos/{photo}""#)),
        "{}",
        detail.body
    );
    assert!(!detail.body.contains("srcset"));
    let response = screen_get(
        &backend,
        &format!("{path}/photos/{photo}/thumbnail"),
        Some(&a),
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);

    let thumbnail = canvas_jpeg(1280, 853, &[]);
    let response = post_thumbnail(
        &backend,
        &path,
        &photo,
        "?width=4000",
        thumbnail.clone(),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT, "{}", response.body);
    assert_eq!(
        backend.objects.lock().unwrap()[&thumbnail_r2_key(&path, &photo)],
        (thumbnail.clone(), "image/jpeg".to_owned())
    );
    // 小さな版は、1 日の追加の回数に数えない（元の写真と合わせて 1 回）。
    assert_eq!(backend.photo_uploads.lock().unwrap().len(), 1);

    let response = screen_get(
        &backend,
        &format!("{path}/photos/{photo}/thumbnail"),
        Some(&a),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.content_type, "image/jpeg");
    assert_eq!(response.bytes, thumbnail);
    assert_eq!(response.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");

    let srcset = format!(
        r#"src="{path}/photos/{photo}/thumbnail" srcset="{path}/photos/{photo}/thumbnail 1280w, {path}/photos/{photo} 4000w""#
    );
    let detail = screen_get(&backend, &path, Some(&a)).await;
    assert!(detail.body.contains(&srcset), "{}", detail.body);
    assert!(
        detail
            .body
            .contains(r#"sizes="(max-width: 359px) calc(100vw - 48px), 240px""#)
    );
    let list = screen_get(&backend, "/", Some(&a)).await;
    assert!(list.body.contains(&srcset), "{}", list.body);
    assert!(
        list.body
            .contains(r#"sizes="(max-width: 600px) calc(100vw - 88px), 300px""#)
    );

    // 1 枚の写真に 1 つまで。
    let response = post_thumbnail(
        &backend,
        &path,
        &photo,
        "?width=4000",
        thumbnail,
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn thumbnail_must_be_a_small_jpeg() {
    let backend = backend_with_workspaces();
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let photo = add_photo(&backend, &path).await;
    let mut too_large = canvas_jpeg(100, 100, &[]);
    too_large.resize(MAX_THUMBNAIL_BYTES + 1, 0);
    for (bytes, query, status) in [
        (
            b"\x89PNG\r\n\x1a\n".to_vec(),
            "?width=100",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            canvas_jpeg(1281, 100, &[]),
            "?width=2000",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (too_large, "?width=100", StatusCode::PAYLOAD_TOO_LARGE),
        (
            canvas_jpeg(100, 100, &[]),
            "?width=0",
            StatusCode::BAD_REQUEST,
        ),
        (canvas_jpeg(100, 100, &[]), "", StatusCode::BAD_REQUEST),
    ] {
        let response = post_thumbnail(&backend, &path, &photo, query, bytes, WORKSPACE_A).await;
        assert_eq!(response.status, status, "{query}");
    }
    // Content-Length の無い大きな本文も、小さな版の上限で止める。
    let request = Request::post(format!("{path}/photos/{photo}/thumbnail?width=100"))
        .header(header::HOST, HOST)
        .header(header::ORIGIN, format!("https://{HOST}"))
        .header("cf-connecting-ip", "192.0.2.1")
        .header(header::COOKIE, cookie(WORKSPACE_A))
        .body(Body::from(vec![0xff; MAX_THUMBNAIL_BYTES + 1]))
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
    // 無い写真には足せない。
    let response = post_thumbnail(
        &backend,
        &path,
        &"9".repeat(32),
        "?width=100",
        canvas_jpeg(100, 100, &[]),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(backend.objects.lock().unwrap().len(), 1);
    assert!(backend.thumbnail_sizes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn thumbnail_metadata_is_zeroed_before_saving() {
    let backend = backend_with_workspaces();
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let photo = add_photo(&backend, &path).await;
    // ブラウザが書く小さな Exif に見立てたもの（位置情報の文字を入れる）。
    let mut exif = vec![0xff, 0xe1, 0x00, 0x12];
    exif.extend_from_slice(b"Exif\0\0GPS-SECRET");
    let thumbnail = canvas_jpeg(100, 100, &[exif]);
    let response = post_thumbnail(
        &backend,
        &path,
        &photo,
        "?width=200",
        thumbnail.clone(),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT, "{}", response.body);
    let stored = backend.objects.lock().unwrap()[&thumbnail_r2_key(&path, &photo)]
        .0
        .clone();
    assert_eq!(stored.len(), thumbnail.len());
    assert!(!stored.windows(10).any(|w| w == b"GPS-SECRET"));
}

#[tokio::test]
async fn demo_is_checked_before_the_thumbnail_itself() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..backend_with_workspaces()
    };
    backend.add_workspace(DEMO, NOW);
    let recipe = create_recipe(&backend, DEMO, "デモ").await;
    // width が無く、本文も小さな版でなくても、デモなら 403 にする（docs/spec.md の「小さな版」の確かめる順）。
    let response = post_thumbnail(
        &backend,
        &recipe,
        &"1".repeat(32),
        "",
        b"not a jpeg".to_vec(),
        DEMO,
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn thumbnail_bytes_count_toward_the_workspace_total() {
    let backend = backend_with_workspaces();
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let photo = add_photo(&backend, &path).await;
    let thumbnail = canvas_jpeg(100, 100, &[]);
    let size = i64::try_from(thumbnail.len()).unwrap();
    // ほかのレシピの写真で、合計を、元の写真（100 バイト）と小さな版の分を残して埋める。
    let other = create_recipe(&backend, WORKSPACE_A, "シチュー").await;
    let other_id = other.trim_start_matches("/recipes/");
    backend.add_photo_row(
        WORKSPACE_A,
        other_id,
        "11111111111111111111111111111111",
        MAX_PHOTO_BYTES_PER_WORKSPACE - 100 - size + 1,
    );
    let response = post_thumbnail(
        &backend,
        &path,
        &photo,
        "?width=200",
        thumbnail.clone(),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    backend
        .photos
        .lock()
        .unwrap()
        .retain(|(_, r, _)| r != other_id);
    backend.add_photo_row(
        WORKSPACE_A,
        other_id,
        "11111111111111111111111111111111",
        MAX_PHOTO_BYTES_PER_WORKSPACE - 100 - size,
    );
    let response = post_thumbnail(
        &backend,
        &path,
        &photo,
        "?width=200",
        thumbnail,
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT, "{}", response.body);
    // 小さな版の分も数えるので、もう 1 バイトも足せない。
    let response =
        post_raw_photo(&backend, &path, &cookie(WORKSPACE_A), "image/jpeg", jpeg(4)).await;
    assert_eq!(response.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn deleting_a_photo_or_recipe_removes_its_thumbnail() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    for _ in 0..2 {
        let photo = add_photo(&backend, &path).await;
        let response = post_thumbnail(
            &backend,
            &path,
            &photo,
            "?width=200",
            canvas_jpeg(100, 100, &[]),
            WORKSPACE_A,
        )
        .await;
        assert_eq!(response.status, StatusCode::NO_CONTENT);
    }
    assert_eq!(backend.objects.lock().unwrap().len(), 4);
    let photo = backend.photos.lock().unwrap()[0].2.id.clone();
    let response = screen_post(
        &backend,
        &format!("{path}/photos/{photo}/delete"),
        Some(&a),
        "",
    )
    .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(backend.objects.lock().unwrap().len(), 2);
    let response = screen_post(&backend, &format!("{path}/delete"), Some(&a), "").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert!(backend.objects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pending_cleanup_removes_thumbnails_too() {
    let backend = backend_with_workspaces();
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let photo = add_photo(&backend, &path).await;
    let response = post_thumbnail(
        &backend,
        &path,
        &photo,
        "?width=200",
        canvas_jpeg(100, 100, &[]),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    backend.mark_pending(&photo, NOW);
    backend.advance(crate::photo::PENDING_CLEANUP_AFTER_MILLIS + 1);
    let summary = crate::workspace::delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 1);
    assert!(backend.objects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn thumbnail_is_removed_when_its_photo_vanishes_while_writing() {
    let backend = backend_with_workspaces();
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let photo = add_photo(&backend, &path).await;
    let racing = FakeBackend {
        photo_rows_vanish_on_put: true,
        ..backend.clone()
    };
    let response = post_thumbnail(
        &racing,
        &path,
        &photo,
        "?width=200",
        canvas_jpeg(100, 100, &[]),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(
        !backend
            .objects
            .lock()
            .unwrap()
            .contains_key(&thumbnail_r2_key(&path, &photo))
    );
}

#[tokio::test]
async fn failed_thumbnail_write_releases_its_bytes() {
    let backend = backend_with_workspaces();
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let photo = add_photo(&backend, &path).await;
    let broken = FakeBackend {
        broken_r2_put: true,
        ..backend.clone()
    };
    let response = post_thumbnail(
        &broken,
        &path,
        &photo,
        "?width=200",
        canvas_jpeg(100, 100, &[]),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(backend.thumbnail_sizes.lock().unwrap().is_empty());
    // もう一度送れる。
    let response = post_thumbnail(
        &backend,
        &path,
        &photo,
        "?width=200",
        canvas_jpeg(100, 100, &[]),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn multipart_photo_has_no_thumbnail_and_demo_cannot_add_one() {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..backend_with_workspaces()
    };
    backend.add_workspace(DEMO, NOW);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    // JavaScript の無いフォームでは、写真の ID を返さない（小さな版は作らない）。
    let photo = jpeg(10);
    let response = post_photo(&backend, &path, Some(&cookie(WORKSPACE_A)), Some(&photo)).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert!(response.headers.get("x-photo-id").is_none());
    // デモでは足せない。
    let demo_recipe = create_recipe(&backend, DEMO, "デモ").await;
    let response = post_thumbnail(
        &backend,
        &demo_recipe,
        &"1".repeat(32),
        "?width=100",
        canvas_jpeg(100, 100, &[]),
        DEMO,
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn failed_finish_removes_the_thumbnail_and_releases_its_bytes() {
    let backend = backend_with_workspaces();
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let photo = add_photo(&backend, &path).await;
    let broken = FakeBackend {
        broken_finish_thumbnail: true,
        ..backend.clone()
    };
    let response = post_thumbnail(
        &broken,
        &path,
        &photo,
        "?width=200",
        canvas_jpeg(100, 100, &[]),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        !backend
            .objects
            .lock()
            .unwrap()
            .contains_key(&thumbnail_r2_key(&path, &photo))
    );
    assert!(backend.thumbnail_sizes.lock().unwrap().is_empty());
    // 枠が返ったので、もう一度送れる。
    let response = post_thumbnail(
        &backend,
        &path,
        &photo,
        "?width=200",
        canvas_jpeg(100, 100, &[]),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn finish_that_committed_before_failing_keeps_the_thumbnail() {
    let backend = backend_with_workspaces();
    let a = cookie(WORKSPACE_A);
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let photo = add_photo(&backend, &path).await;
    let flaky = FakeBackend {
        finish_thumbnail_commits_then_errors: true,
        ..backend.clone()
    };
    let thumbnail = canvas_jpeg(100, 100, &[]);
    let response = post_thumbnail(
        &flaky,
        &path,
        &photo,
        "?width=200",
        thumbnail.clone(),
        WORKSPACE_A,
    )
    .await;
    // D1 では確定していたので、出している小さな版を消さない。
    assert_eq!(response.status, StatusCode::NO_CONTENT, "{}", response.body);
    let response = screen_get(
        &backend,
        &format!("{path}/photos/{photo}/thumbnail"),
        Some(&a),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.bytes, thumbnail);
}

#[tokio::test]
async fn other_workspace_cannot_read_or_add_thumbnails() {
    let backend = backend_with_workspaces();
    let path = create_recipe(&backend, WORKSPACE_A, "カレー").await;
    let photo = add_photo(&backend, &path).await;
    let response = post_thumbnail(
        &backend,
        &path,
        &photo,
        "?width=200",
        canvas_jpeg(100, 100, &[]),
        WORKSPACE_A,
    )
    .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);

    let b = cookie(WORKSPACE_B);
    let response = screen_get(
        &backend,
        &format!("{path}/photos/{photo}/thumbnail"),
        Some(&b),
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    // B のワークスペースに、そのレシピが無いので見つからない（ワークスペースで絞り込む）。
    assert!(
        response.body.contains("見つかりません"),
        "{}",
        response.body
    );
    // B のワークスペースからは、A の写真に小さな版を足せない（A の写真は、B には無い写真である）。
    let other = add_photo(&backend, &path).await;
    let response = post_thumbnail(
        &backend,
        &path,
        &other,
        "?width=200",
        canvas_jpeg(100, 100, &[]),
        WORKSPACE_B,
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(
        backend
            .objects
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.starts_with("thumbnails/"))
            .count(),
        1
    );
    assert_eq!(backend.thumbnail_sizes.lock().unwrap().len(), 1);
}
