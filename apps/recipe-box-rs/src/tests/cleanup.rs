//! 使われていないワークスペースの自動削除（`workspace::delete_unused`）のテスト。

use axum::http::{Method, StatusCode};

use super::{
    WORKSPACE_A, WORKSPACE_B, call,
    fake::{FakeBackend, NOW},
};
use crate::{
    limits::{DAY_MILLIS, start_of_utc_day},
    workspace::{CleanupSummary, UNUSED_DELETE_AFTER_MILLIS, delete_unused},
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
