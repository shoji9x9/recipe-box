//! 使われていないワークスペースの自動削除（`workspace::delete_unused`）のテスト。

use axum::http::{Method, StatusCode};

use std::sync::atomic::Ordering;

use super::{
    WORKSPACE_A, WORKSPACE_B, call,
    fake::{FakeBackend, NOW},
};
use crate::{
    limits::{DAY_MILLIS, start_of_utc_day},
    photo::{MAX_KEYS_PER_DELETE, MAX_PENDING_BATCHES_PER_RUN, PENDING_CLEANUP_AFTER_MILLIS},
    workspace::{
        CLEANUP_RUNS_PER_DAY, CLEANUP_SUBREQUESTS, CleanupSummary, MAX_SUBREQUESTS_PER_RUN,
        MAX_WORKSPACES_PER_DAY, MAX_WORKSPACES_PER_RUN, UNUSED_DELETE_AFTER_MILLIS, delete_unused,
    },
};

const WORKSPACE_C: &str = "cccccccccccccccccccccccccccccccc";
const DEMO: &str = "dddddddddddddddddddddddddddddddd";
const PINNED: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
/// 自動削除の境目。最終利用日時がこれより前なら消す。
const BEFORE: i64 = NOW - UNUSED_DELETE_AFTER_MILLIS;

/// ワークスペースに、レシピ 1 件と写真 1 枚（D1 の行と R2 のデータ）を足す。
fn add_recipe_with_photo(backend: &FakeBackend, workspace: &str, recipe: &str) {
    let id = crate::recipe::RecipeId::parse(recipe).unwrap();
    let workspace_id = crate::workspace::WorkspaceId::parse(workspace).unwrap();
    backend.recipes.lock().unwrap().insert(
        (workspace.to_owned(), recipe.to_owned()),
        crate::recipe::Recipe {
            id: id.to_string(),
            name: "カレー".to_owned(),
            servings: 2,
            ingredients: String::new(),
            steps: String::new(),
            created_at: NOW,
            updated_at: NOW,
        },
    );
    // 写真の ID は、テストで見分けやすいようレシピと同じにする。
    let photo = recipe;
    backend.add_photo_row(workspace, recipe, photo, 3);
    backend.objects.lock().unwrap().insert(
        crate::photo::object_key(&workspace_id, &id, photo),
        (vec![0xff, 0xd8, 0xff], "image/jpeg".to_owned()),
    );
}

/// 14 日を 1 ミリ秒だけ過ぎた A（写真あり）、ちょうど 14 日の B、使われている C、
/// 印のある PINNED、印の無いデモを持つ偽物。PINNED とデモも、ずっと使われていない。
fn setup() -> FakeBackend {
    let backend = FakeBackend {
        demo: Some(DEMO.to_owned()),
        ..FakeBackend::default()
    };
    backend.add_workspace(WORKSPACE_A, BEFORE - 1);
    backend.add_workspace(WORKSPACE_B, BEFORE);
    backend.add_workspace(WORKSPACE_C, NOW);
    backend.add_workspace(PINNED, BEFORE - 100 * DAY_MILLIS);
    backend.add_workspace(DEMO, BEFORE - 100 * DAY_MILLIS);
    backend.pinned.lock().unwrap().insert(PINNED.to_owned());
    add_recipe_with_photo(&backend, WORKSPACE_A, "11111111111111111111111111111111");
    add_recipe_with_photo(&backend, WORKSPACE_C, "22222222222222222222222222222222");
    add_recipe_with_photo(&backend, PINNED, "33333333333333333333333333333333");
    backend
}

#[tokio::test]
async fn unused_workspaces_are_deleted_with_recipes_and_photos() {
    let backend = setup();
    let summary = delete_unused(&backend).await;
    // A だけを消す。B はちょうど 14 日なので残す。デモは印が無くても残す。
    assert_eq!(
        summary,
        CleanupSummary {
            deleted: 1,
            skipped: 1,
            ..CleanupSummary::default()
        }
    );
    let workspaces = backend.workspaces.lock().unwrap();
    assert!(!workspaces.contains_key(WORKSPACE_A));
    for kept in [WORKSPACE_B, WORKSPACE_C, PINNED, DEMO] {
        assert!(workspaces.contains_key(kept), "{kept}");
    }
    drop(workspaces);
    // A のレシピと写真（D1 の行と R2 のデータ）も消え、ほかのワークスペースの写真は残る。
    assert!(
        backend
            .recipes
            .lock()
            .unwrap()
            .keys()
            .all(|(workspace, _)| workspace != WORKSPACE_A)
    );
    assert!(
        backend
            .photos
            .lock()
            .unwrap()
            .iter()
            .all(|(workspace, _, _)| workspace != WORKSPACE_A)
    );
    let objects = backend.objects.lock().unwrap();
    assert_eq!(objects.len(), 2);
    assert!(objects.keys().all(|key| !key.contains(WORKSPACE_A)));
}

#[tokio::test]
async fn failed_workspace_is_retried_next_time_and_others_are_deleted() {
    let backend = setup();
    backend.add_workspace(WORKSPACE_B, BEFORE - 1);
    let broken = FakeBackend {
        broken_r2_delete: true,
        ..backend.clone()
    };
    // A は R2 の削除で失敗するが、写真の無い B は消す。A は D1 の行が残るので、次の実行で消せる。
    let summary = delete_unused(&broken).await;
    assert_eq!((summary.deleted, summary.failed), (1, 1));
    assert!(summary.has_failure());
    assert!(backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
    assert!(!backend.workspaces.lock().unwrap().contains_key(WORKSPACE_B));

    let summary = delete_unused(&backend).await;
    assert_eq!((summary.deleted, summary.failed), (1, 0));
    assert!(!backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
}

#[tokio::test]
async fn records_before_today_are_deleted() {
    let backend = setup();
    let today = start_of_utc_day(NOW);
    *backend.photo_uploads.lock().unwrap() = vec![today - 1, today, NOW];
    *backend.workspace_creations.lock().unwrap() = vec![today - DAY_MILLIS, today - 1, today];
    delete_unused(&backend).await;
    // UTC の今日の 0 時より前の記録は、1 日の数に入らないので消す。
    assert_eq!(*backend.photo_uploads.lock().unwrap(), [today, NOW]);
    assert_eq!(*backend.workspace_creations.lock().unwrap(), [today]);
}

#[tokio::test]
async fn pinned_workspace_cannot_be_deleted_by_api() {
    let backend = setup();
    let response = call(
        &backend,
        Method::DELETE,
        "/api/workspaces",
        Some(PINNED),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(response.json()["error"], "demo_workspace_protected");
    // 断る前に、R2 の写真を消していない。
    assert!(backend.workspaces.lock().unwrap().contains_key(PINNED));
    assert!(
        backend
            .objects
            .lock()
            .unwrap()
            .keys()
            .any(|key| key.contains(PINNED))
    );
}

#[tokio::test]
async fn workspace_used_after_listing_keeps_its_photos() {
    // 対象を選んだ直後に、A の持ち主が A を開いた。
    let backend = FakeBackend {
        used_after_listing: Some(WORKSPACE_A.to_owned()),
        ..setup()
    };
    let summary = delete_unused(&backend).await;
    assert_eq!((summary.deleted, summary.skipped), (0, 2));
    // A は消さず、R2 の写真も消さない。
    assert!(backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
    assert!(
        backend
            .objects
            .lock()
            .unwrap()
            .keys()
            .any(|key| key.contains(WORKSPACE_A))
    );
}

#[tokio::test]
async fn records_are_deleted_even_if_listing_fails() {
    let backend = FakeBackend {
        broken_list: true,
        ..setup()
    };
    let today = start_of_utc_day(NOW);
    *backend.photo_uploads.lock().unwrap() = vec![today - 1, today];
    *backend.workspace_creations.lock().unwrap() = vec![today - 1, today];
    let summary = delete_unused(&backend).await;
    assert!(summary.list_failed);
    assert!(!summary.records_failed);
    assert!(summary.has_failure());
    // 対象を選べなくても、古い記録は消す。
    assert_eq!(*backend.photo_uploads.lock().unwrap(), [today]);
    assert_eq!(*backend.workspace_creations.lock().unwrap(), [today]);
}

#[tokio::test]
async fn failed_record_deletion_is_reported_and_workspaces_are_deleted() {
    let backend = FakeBackend {
        broken_records: true,
        ..setup()
    };
    let summary = delete_unused(&backend).await;
    assert!(summary.records_failed);
    assert!(summary.has_failure());
    assert_eq!(summary.deleted, 1);
}

#[tokio::test]
async fn malformed_workspace_id_is_counted_as_failure() {
    let backend = setup();
    // アプリは作らない、形の正しくない ID の行。
    backend.add_workspace("not-a-workspace-id", BEFORE - 1);
    let summary = delete_unused(&backend).await;
    assert_eq!((summary.deleted, summary.failed), (1, 1));
    assert!(summary.has_failure());
}

/// D1 と R2 を呼んだ回数の合計。
fn calls(backend: &FakeBackend) -> usize {
    backend.d1_calls.load(Ordering::Relaxed) + backend.r2_calls.load(Ordering::Relaxed)
}

fn to_usize(value: i64) -> usize {
    usize::try_from(value).unwrap()
}

#[tokio::test]
async fn many_due_workspaces_and_pending_photos_stay_within_the_call_limit() {
    // 本物と同じく、1 回の実行で D1 と R2 を合わせて上限を超えて呼ぶと、その後の呼び出しが失敗する偽物。
    let backend = FakeBackend {
        max_calls: Some(to_usize(MAX_SUBREQUESTS_PER_RUN)),
        ..FakeBackend::default()
    };
    // 1 日に作れる上限いっぱいのワークスペースが、同じ日に対象になる（写真付き）。最終利用日時は少しずつずらす。
    let due = to_usize(MAX_WORKSPACES_PER_DAY);
    let mut workspaces = Vec::new();
    for i in 0..due {
        let workspace = format!("{:032x}", 0x1000 + i);
        backend.add_workspace(&workspace, BEFORE - 1 - i64::try_from(i).unwrap());
        add_recipe_with_photo(&backend, &workspace, &format!("{:032x}", 0x2000 + i));
        workspaces.push(workspace);
    }
    // 1 回で片付けられる数より多い、書き込み中の印が残った写真の行（使われているワークスペースのもの）。
    let per_run = to_usize(MAX_PENDING_BATCHES_PER_RUN * MAX_KEYS_PER_DELETE);
    let pending = per_run + 500;
    backend.add_workspace(WORKSPACE_C, NOW);
    let recipe = "3".repeat(32);
    for i in 0..pending {
        let id = format!("{:032x}", 0x10_0000 + i);
        backend.add_photo_row(WORKSPACE_C, &recipe, &id, 1);
        backend.mark_pending(&id, NOW - PENDING_CLEANUP_AFTER_MILLIS - 1);
    }
    let today = start_of_utc_day(NOW);
    *backend.photo_uploads.lock().unwrap() = vec![today - 1, today];

    let summary = delete_unused(&backend).await;
    // 上限を超えず、どれも失敗しない。片付けも動く。
    assert!(!summary.has_failure(), "{summary:?}");
    assert!(calls(&backend) <= to_usize(CLEANUP_SUBREQUESTS));
    let per_run_workspaces = to_usize(MAX_WORKSPACES_PER_RUN);
    assert_eq!(summary.deleted, per_run_workspaces);
    assert_eq!(summary.deferred, due - per_run_workspaces);
    assert_eq!(summary.pending_photos.cleaned, per_run);
    assert_eq!(summary.pending_photos.deferred, pending - per_run);
    assert_eq!(*backend.photo_uploads.lock().unwrap(), [today]);
    // 長く使われていないものから消す（最終利用日時は、後ろほど古い）。
    let remaining = backend.workspaces.lock().unwrap().clone();
    for (i, workspace) in workspaces.iter().enumerate() {
        assert_eq!(
            remaining.contains_key(workspace),
            i < due - per_run_workspaces,
            "{i}"
        );
    }

    // 残りは次の実行に回り、1 日の実行の回数のうちに消し終える。
    let mut runs = 1;
    while backend.workspaces.lock().unwrap().len() > 1 {
        backend.d1_calls.store(0, Ordering::Relaxed);
        backend.r2_calls.store(0, Ordering::Relaxed);
        let summary = delete_unused(&backend).await;
        assert!(!summary.has_failure(), "{summary:?}");
        runs += 1;
    }
    assert!(runs <= CLEANUP_RUNS_PER_DAY, "{runs}");
    assert!(backend.photos.lock().unwrap().is_empty());
    assert!(backend.objects.lock().unwrap().is_empty());
    assert!(backend.workspaces.lock().unwrap().contains_key(WORKSPACE_C));
}

#[test]
fn cron_runs_as_often_as_the_cleanup_budget_assumes() {
    // 1 回に消す数の上限（MAX_WORKSPACES_PER_RUN）は、1 日の実行の回数（CLEANUP_RUNS_PER_DAY）と合わせて決めている。
    let wrangler = include_str!("../../wrangler.toml");
    // 6 時間ごと（0 */6 * * *）は 1 日 4 回。
    assert!(
        wrangler.contains(r#"crons = ["0 */6 * * *"]"#),
        "wrangler.toml の [triggers] を変えたら、CLEANUP_RUNS_PER_DAY も直す"
    );
    assert_eq!(CLEANUP_RUNS_PER_DAY, 4);
}

#[tokio::test]
async fn workspace_used_after_its_photos_were_hidden_is_kept_and_photos_are_cleaned_up() {
    // 写真を隠した直後に、A の持ち主が A を開いた（ごく短い間の競合）。
    let backend = FakeBackend {
        used_after_hiding: Some(WORKSPACE_A.to_owned()),
        ..setup()
    };
    let summary = delete_unused(&backend).await;
    assert_eq!((summary.deleted, summary.failed), (0, 0));
    // A は残る。写真は隠れたままで、データの無い写真は見えない。
    assert!(backend.workspaces.lock().unwrap().contains_key(WORKSPACE_A));
    let recipe = "1".repeat(32);
    let path = format!("/api/recipes/{recipe}/photos");
    let response = call(&backend, Method::GET, &path, Some(WORKSPACE_A), None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.json()["photos"].as_array().unwrap().len(), 0);
    // 隠れた行は、時間がたった後の自動削除が片付ける。
    let backend = FakeBackend {
        used_after_hiding: None,
        ..backend
    };
    backend.advance(PENDING_CLEANUP_AFTER_MILLIS + 1);
    let summary = delete_unused(&backend).await;
    assert_eq!(summary.pending_photos.cleaned, 1);
    assert!(
        backend
            .photos
            .lock()
            .unwrap()
            .iter()
            .all(|(workspace, _, _)| workspace != WORKSPACE_A)
    );
}
