//! ワークスペースの作成と、`X-Workspace-Id` による受け取り（`Workspace` extractor）のテスト。
//! 受け取りは、ワークスペースを使う API の `GET /api/recipes` を通して確かめる。

use std::sync::atomic::Ordering;

use axum::{
    body::Body,
    http::{HeaderValue, Method, Request, StatusCode},
};

use super::{
    WORKSPACE_A, call,
    fake::{FakeBackend, NOW},
    send,
};
use crate::{
    router,
    workspace::{
        LAST_USED_UPDATE_INTERVAL_MILLIS, MAX_WORKSPACES, MAX_WORKSPACES_PER_DAY, WorkspaceId,
    },
};

fn create_workspace_request() -> Request<Body> {
    Request::post("/api/workspaces")
        .header("cf-connecting-ip", "192.0.2.1")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn create_workspace_returns_new_id() {
    let backend = FakeBackend::default();
    let first = send(router(backend.clone()), create_workspace_request()).await;
    let second = send(router(backend.clone()), create_workspace_request()).await;

    assert_eq!(first.status, StatusCode::CREATED);
    assert_eq!(first.content_type, "application/json");
    let id = first.json()["id"].as_str().unwrap().to_owned();
    assert!(WorkspaceId::parse(&id).is_some(), "{id}");
    assert_ne!(second.json()["id"].as_str().unwrap(), id);
    // 作ったワークスペースが保存され、最終利用日時が今になっている。
    assert_eq!(backend.workspaces.lock().unwrap().get(&id), Some(&NOW));
    // 接続元の IP アドレスが、レート制限のキーとして渡っている。
    assert_eq!(
        *backend.rate_limit_keys.lock().unwrap(),
        ["192.0.2.1", "192.0.2.1"]
    );
}

#[tokio::test]
async fn create_workspace_without_client_ip_uses_unknown_key() {
    let backend = FakeBackend::default();
    let request = Request::post("/api/workspaces")
        .body(Body::empty())
        .unwrap();
    let response = send(router(backend.clone()), request).await;
    assert_eq!(response.status, StatusCode::CREATED);
    assert_eq!(*backend.rate_limit_keys.lock().unwrap(), ["unknown"]);
}

#[tokio::test]
async fn create_workspace_rejects_when_rate_limited() {
    let backend = FakeBackend {
        rate_limited: true,
        ..FakeBackend::default()
    };
    let response = send(router(backend.clone()), create_workspace_request()).await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.json()["error"], "too_many_requests");
    assert!(backend.workspaces.lock().unwrap().is_empty());
}

#[tokio::test]
async fn create_workspace_hides_backend_errors() {
    let backend = FakeBackend {
        broken: true,
        ..FakeBackend::default()
    };
    let response = send(router(backend), create_workspace_request()).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.json()["error"], "internal_error");
    assert!(!response.body.contains("壊れた DB"));
}

#[tokio::test]
async fn workspace_header_rejects_bad_requests() {
    let cases = [
        (None, StatusCode::BAD_REQUEST, "workspace_id_missing"),
        (
            Some("not-an-id"),
            StatusCode::BAD_REQUEST,
            "workspace_id_invalid",
        ),
        (
            Some("0123456789ABCDEF0123456789ABCDEF"),
            StatusCode::BAD_REQUEST,
            "workspace_id_invalid",
        ),
        (
            Some("ffffffffffffffffffffffffffffffff"),
            StatusCode::NOT_FOUND,
            "workspace_not_found",
        ),
    ];
    for (id, status, error) in cases {
        let response = call(
            &FakeBackend::default(),
            Method::GET,
            "/api/recipes",
            id,
            None,
        )
        .await;
        assert_eq!(response.status, status, "{id:?}");
        assert_eq!(response.content_type, "application/json", "{id:?}");
        assert_eq!(response.json()["error"], error, "{id:?}");
    }
}

#[tokio::test]
async fn workspace_header_rejects_unusual_values() {
    let backend = FakeBackend::default();
    let id = backend.add_workspace(WORKSPACE_A, NOW);
    let requests = [
        ("空", vec![HeaderValue::from_static("")]),
        (
            "ASCII でない",
            vec![HeaderValue::from_bytes(&[0xff; 32]).unwrap()],
        ),
        (
            "前の空白",
            vec![HeaderValue::from_str(&format!(" {id}")).unwrap()],
        ),
        (
            "後ろの空白",
            vec![HeaderValue::from_str(&format!("{id} ")).unwrap()],
        ),
        (
            "2 つ",
            vec![
                HeaderValue::from_str(&id).unwrap(),
                HeaderValue::from_str(&id).unwrap(),
            ],
        ),
    ];
    for (label, values) in requests {
        let mut request = Request::get("/api/recipes").body(Body::empty()).unwrap();
        for value in values {
            request.headers_mut().append("x-workspace-id", value);
        }
        let response = send(router(backend.clone()), request).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{label}");
        assert_eq!(response.json()["error"], "workspace_id_invalid", "{label}");
    }
}

#[tokio::test]
async fn workspace_last_used_at_is_updated_at_most_once_an_hour() {
    let backend = FakeBackend::default();
    let id = backend.add_workspace(WORKSPACE_A, NOW);

    // 前の書き換えから 1 時間たっていなければ、書き換えない。
    backend.advance(LAST_USED_UPDATE_INTERVAL_MILLIS - 1);
    let response = call(&backend, Method::GET, "/api/recipes", Some(&id), None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(backend.last_used_writes.load(Ordering::Relaxed), 0);
    assert_eq!(backend.workspaces.lock().unwrap().get(&id), Some(&NOW));

    // 1 時間たったら、今の時刻に書き換える。
    backend.advance(1);
    call(&backend, Method::GET, "/api/recipes", Some(&id), None).await;
    assert_eq!(backend.last_used_writes.load(Ordering::Relaxed), 1);
    assert_eq!(
        backend.workspaces.lock().unwrap().get(&id),
        Some(&(NOW + LAST_USED_UPDATE_INTERVAL_MILLIS))
    );
}

#[tokio::test]
async fn workspace_header_hides_backend_errors() {
    let backend = FakeBackend {
        broken: true,
        ..FakeBackend::default()
    };
    let response = call(
        &backend,
        Method::GET,
        "/api/recipes",
        Some(WORKSPACE_A),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.json()["error"], "internal_error");
    assert!(!response.body.contains("壊れた DB"));
}

/// `NOW` の `days` 日前の時刻。
const fn days_before(days: i64) -> i64 {
    NOW - days * 24 * 60 * 60 * 1000
}

/// `NOW` の日（UTC）の 0 時。
fn start_of_today() -> i64 {
    NOW - NOW.rem_euclid(24 * 60 * 60 * 1000)
}

#[tokio::test]
async fn create_workspace_stops_at_total_limit() {
    let backend = FakeBackend::default();
    // 昨日までに作った分で、全体の上限の 1 つ手前まで埋める（1 日の上限には数えない）。
    for n in 0..MAX_WORKSPACES - 1 {
        backend.add_workspace(&format!("{n:032x}"), days_before(1));
    }
    let response = send(router(backend.clone()), create_workspace_request()).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    // 上限に達したら、503 で断り、保存しない。
    let response = send(router(backend.clone()), create_workspace_request()).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.json()["error"], "capacity_reached");
    assert_eq!(
        i64::try_from(backend.workspaces.lock().unwrap().len()).unwrap(),
        MAX_WORKSPACES
    );
}

#[tokio::test]
async fn create_workspace_stops_at_daily_limit() {
    let backend = FakeBackend::default();
    // 昨日の最後の瞬間に作った分は、今日の数に入らない。
    backend.add_workspace(&format!("{:032x}", 9999), start_of_today() - 1);
    for _ in 0..MAX_WORKSPACES_PER_DAY {
        let response = send(router(backend.clone()), create_workspace_request()).await;
        assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    }
    let response = send(router(backend.clone()), create_workspace_request()).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.json()["error"], "capacity_reached");

    // UTC の日付が変われば、また作れる。
    backend.advance(start_of_today() + 24 * 60 * 60 * 1000 - NOW);
    let response = send(router(backend.clone()), create_workspace_request()).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
}

#[tokio::test]
async fn create_workspace_checks_rate_limit_before_capacity() {
    let backend = FakeBackend {
        rate_limited: true,
        ..FakeBackend::default()
    };
    for n in 0..MAX_WORKSPACES {
        backend.add_workspace(&format!("{n:032x}"), days_before(1));
    }
    let response = send(router(backend), create_workspace_request()).await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn deleting_workspaces_does_not_reset_daily_limit() {
    let backend = FakeBackend::default();
    for _ in 0..MAX_WORKSPACES_PER_DAY {
        let response = send(router(backend.clone()), create_workspace_request()).await;
        assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
        let id = response.json()["id"].as_str().unwrap().to_owned();
        let response = call(&backend, Method::DELETE, "/api/workspaces", Some(&id), None).await;
        assert_eq!(response.status, StatusCode::NO_CONTENT);
    }
    // 作ると消すを繰り返しても、その日に作った数は戻らない。
    assert!(backend.workspaces.lock().unwrap().is_empty());
    let response = send(router(backend.clone()), create_workspace_request()).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.json()["error"], "capacity_reached");
}
