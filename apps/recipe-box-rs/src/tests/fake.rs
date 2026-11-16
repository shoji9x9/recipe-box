//! メモリの中で動く、テスト用の [`Backend`]。
//!
//! D1 と R2 の代わりに `HashMap` などにデータを持つ。レシピはワークスペースの ID とレシピの ID の組で持ち、
//! 本物の SQL と同じく、指定したワークスペースのレシピだけを読み書きする。

use std::{
    collections::{HashMap, HashSet},
    future::{Future, ready},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicUsize, Ordering},
    },
};

use crate::{
    backend::{
        Backend, BackendError, BackendResult, ListedRecipe, PendingPhoto, PhotoInsert, PhotoKey,
        PhotoLimits, RecipePage, UnusedWorkspace, WorkspaceLimits, WorkspaceStatus,
    },
    photo::{Photo, PhotoBody, PhotoId, object_key},
    recipe::{Recipe, RecipeFields, RecipeId, search_name},
    workspace::WorkspaceId,
};

/// テストの中の「今」の時刻の初めの値。
pub const NOW: i64 = 1_800_000_000_000;

/// ワークスペースの ID とレシピの ID の組。
type RecipeKey = (String, String);
/// R2 のオブジェクトのキーと、データ・形式。
type Objects = HashMap<String, (Vec<u8>, String)>;

// 失敗させるかどうかの切り替えを、テストごとに `bool` で持つ。
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone)]
pub struct FakeBackend {
    /// ワークスペースの ID と最終利用日時。
    pub workspaces: Arc<Mutex<HashMap<String, i64>>>,
    /// 自動削除と削除から外す印（`pinned`）の付いたワークスペースの ID。
    pub pinned: Arc<Mutex<HashSet<String>>>,
    /// ワークスペースを作った記録（作った日時）。ワークスペースを消しても残す。
    pub workspace_creations: Arc<Mutex<Vec<i64>>>,
    /// レシピ。
    pub recipes: Arc<Mutex<HashMap<RecipeKey, Recipe>>>,
    /// 次に返す乱数の 1 バイト目。呼ぶたびに 1 増やし、毎回ちがう ID になるようにする。
    pub next_random: Arc<AtomicU8>,
    /// 「今」の時刻。テストから進められる。
    pub now: Arc<AtomicI64>,
    /// `true` なら、ワークスペースを作る回数の制限を超えたことにする。
    pub rate_limited: bool,
    /// 共有のデモのワークスペースの ID。
    pub demo: Option<String>,
    /// `true` なら、DB が失敗したことにする。
    pub broken: bool,
    /// `true` なら、書き込む API の回数の制限を超えたことにする。
    pub write_limited: bool,
    /// レート制限に渡されたキー。
    pub rate_limit_keys: Arc<Mutex<Vec<String>>>,
    /// 書き込む API の回数の制限に渡されたキー。
    pub write_limit_keys: Arc<Mutex<Vec<String>>>,
    /// 最終利用日時を書き換えた回数。
    pub last_used_writes: Arc<AtomicUsize>,
    /// 写真の行。ワークスペースの ID・レシピの ID・写真の情報。
    pub photos: Arc<Mutex<Vec<(String, String, Photo)>>>,
    /// 書き込みか削除が終わっていない印（`pending_since`）の付いた写真の ID と、印を付けた日時
    /// （写真の ID は乱数なので、ID だけで見分ける。行を消すときは、印も消す）。
    pub pending_photos: Arc<Mutex<HashMap<String, i64>>>,
    /// `true` なら、写真の印を外す・付けるのが失敗したことにする（D1 の失敗）。
    pub broken_photo_pending: bool,
    /// `true` なら、写真の行を消すのが失敗したことにする（D1 の失敗）。
    pub broken_photo_delete: bool,
    /// 写真を追加した記録（追加した日時）。
    pub photo_uploads: Arc<Mutex<Vec<i64>>>,
    /// R2 のオブジェクト。キーと、データ・形式。
    pub objects: Arc<Mutex<Objects>>,
    /// `true` なら、R2 への書き込みが失敗したことにする。
    pub broken_r2_put: bool,
    /// `true` なら、`run_to_completion` の task を動かさずに捨てたことにする（`None` を返す）。
    pub task_dropped: bool,
    /// `true` なら、R2 に書き込んでいる間に、別のリクエストがすべての写真の行を消したことにする。
    pub photo_rows_vanish_on_put: bool,
    /// `true` なら、R2 に書き込んでいる間に、別のリクエスト（レシピかワークスペースの削除）が、すべての写真の行に印を付け直したことにする。
    pub photo_rows_remarked_on_put: bool,
    /// `true` なら、R2 の削除の直前に、別のリクエスト（レシピかワークスペースの削除）が、すべての写真の行に印を付け直したことにする。
    pub photo_rows_remarked_on_delete: bool,
    /// これらの写真の行を含む batch（`delete_photos`）は失敗させる（片付けで失敗し続ける行）。
    pub undeletable_photos: Vec<String>,
    /// `true` なら、R2 の削除が失敗したことにする。
    pub broken_r2_delete: bool,
    /// `true` なら、最初の R2 の削除の直後に、別のリクエストが同じレシピに写真を 1 枚追加し（D1 の行と R2 のデータ）、
    /// 消したばかりのキーにも書き込んだことにする。レシピの削除と写真の追加が同時に起きた場合を確かめる。
    pub photo_added_after_first_delete: bool,
    /// `true` なら、ワークスペースを消す直前に、別のリクエストが同じワークスペースを消したことにする。
    pub workspace_vanishes_on_delete: bool,
    /// `true` なら、古い記録の削除だけが失敗したことにする。
    pub broken_records: bool,
    /// `true` なら、自動削除の対象を選ぶところだけが失敗したことにする。
    pub broken_list: bool,
    /// `true` なら、レシピの登録と更新だけが失敗したことにする（D1 の一時的な失敗）。
    pub broken_recipe_write: bool,
    /// 自動削除の対象を選んだ直後に、別のリクエストがこのワークスペースを使った（最終利用日時を今にした）ことにする。
    pub used_after_listing: Option<String>,
    /// `photo_added_after_first_delete` の写真を、もう追加したか。
    pub injected_photo: Arc<AtomicBool>,
    /// `true` なら、レシピかワークスペースの D1 の削除（`delete_recipe`・`delete_workspace`）が失敗したことにする。
    pub broken_owner_delete: bool,
    /// ワークスペースの写真を隠した直後に、別のリクエストがこのワークスペースを使った（最終利用日時を今にした）ことにする。
    pub used_after_hiding: Option<String>,
    /// D1 を呼んだ回数（batch は 1 回）。本物の 1 回の実行のクエリの数の上限を確かめるのに使う。
    pub d1_calls: Arc<AtomicUsize>,
    /// R2 を呼んだ回数。
    pub r2_calls: Arc<AtomicUsize>,
    /// D1 と R2 を合わせて、この回数を超えて呼ぶと失敗させる（本物の 1 回の実行の上限をまねる）。
    pub max_calls: Option<usize>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self {
            workspaces: Arc::default(),
            workspace_creations: Arc::default(),
            pinned: Arc::default(),
            recipes: Arc::default(),
            next_random: Arc::default(),
            now: Arc::new(AtomicI64::new(NOW)),
            rate_limited: false,
            demo: None,
            broken: false,
            write_limited: false,
            rate_limit_keys: Arc::default(),
            write_limit_keys: Arc::default(),
            last_used_writes: Arc::default(),
            photos: Arc::default(),
            pending_photos: Arc::default(),
            broken_photo_pending: false,
            broken_photo_delete: false,
            photo_uploads: Arc::default(),
            objects: Arc::default(),
            broken_r2_put: false,
            task_dropped: false,
            photo_rows_vanish_on_put: false,
            photo_rows_remarked_on_put: false,
            photo_rows_remarked_on_delete: false,
            undeletable_photos: Vec::new(),
            broken_r2_delete: false,
            photo_added_after_first_delete: false,
            workspace_vanishes_on_delete: false,
            used_after_listing: None,
            broken_list: false,
            broken_records: false,
            broken_recipe_write: false,
            injected_photo: Arc::default(),
            broken_owner_delete: false,
            used_after_hiding: None,
            d1_calls: Arc::default(),
            r2_calls: Arc::default(),
            max_calls: None,
        }
    }
}

impl FakeBackend {
    /// 最終利用日時が `last_used_at` のワークスペースを足し、その ID を返す。作った日時も `last_used_at` にする。
    pub fn add_workspace(&self, id: &str, last_used_at: i64) -> String {
        self.workspace_creations.lock().unwrap().push(last_used_at);
        self.workspaces
            .lock()
            .unwrap()
            .insert(id.to_owned(), last_used_at);
        id.to_owned()
    }

    /// 写真の行だけを足す（R2 のデータは入れない）。上限の境界を、大きなデータを作らずに確かめるために使う。
    pub fn add_photo_row(&self, workspace: &str, recipe: &str, id: &str, size: i64) {
        self.add_photo_row_at(workspace, recipe, id, size, self.now_millis());
    }

    /// [`Self::add_photo_row`] の、追加した日時を決めて入れるもの（並び順を確かめるテストで使う）。
    pub fn add_photo_row_at(
        &self,
        workspace: &str,
        recipe: &str,
        id: &str,
        size: i64,
        created_at: i64,
    ) {
        self.photos.lock().unwrap().push((
            workspace.to_owned(),
            recipe.to_owned(),
            Photo {
                id: id.to_owned(),
                content_type: "image/jpeg".to_owned(),
                size,
                created_at,
            },
        ));
    }

    /// 写真の行に、書き込みか削除が終わっていない印が付いているか。
    pub fn is_pending(&self, id: &str) -> bool {
        self.pending_photos.lock().unwrap().contains_key(id)
    }

    /// 写真の行に、日時 `since` の印を付ける（途中で残った行を作るテストで使う）。
    pub fn mark_pending(&self, id: &str, since: i64) {
        self.pending_photos
            .lock()
            .unwrap()
            .insert(id.to_owned(), since);
    }

    /// 消した写真の行の印を消す（本物では、印は行の列なので、行と一緒に消える）。
    fn forget_pending<'a>(&self, ids: impl IntoIterator<Item = &'a String>) {
        let mut pending = self.pending_photos.lock().unwrap();
        for id in ids {
            pending.remove(id);
        }
    }

    /// 写真の行に、印が付いていないか（見せる写真か）。
    fn is_visible(&self, photo: &Photo) -> bool {
        !self.is_pending(&photo.id)
    }

    /// 写真の印を付ける（`mark`）か外す。`at` は、付ける印の日時か、外す印の日時。
    /// 付けるときは印の無い行に、外すときは日時が `at` の印の付いた行にだけ効く。
    fn set_pending(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
        mark: bool,
        at: i64,
    ) -> BackendResult<bool> {
        self.check()?;
        if self.broken_photo_pending {
            return Err(BackendError("壊れた DB（写真の印）".into()));
        }
        let exists = self.photos.lock().unwrap().iter().any(|(w, r, p)| {
            w == workspace.as_str() && r == recipe.as_str() && p.id == id.as_str()
        });
        if !exists {
            return Ok(false);
        }
        let mut pending = self.pending_photos.lock().unwrap();
        Ok(if mark {
            if pending.contains_key(id.as_str()) {
                false
            } else {
                pending.insert(id.as_str().to_owned(), at);
                true
            }
        } else if pending.get(id.as_str()) == Some(&at) {
            pending.remove(id.as_str());
            true
        } else {
            false
        })
    }

    /// ワークスペースのすべての写真の、レシピの ID と写真の ID。
    fn workspace_photo_keys(&self, workspace: &WorkspaceId) -> Vec<PhotoKey> {
        self.photos
            .lock()
            .unwrap()
            .iter()
            .filter(|(w, _, _)| w == workspace.as_str())
            .map(|(_, r, p)| PhotoKey {
                recipe_id: r.clone(),
                id: p.id.clone(),
            })
            .collect()
    }

    /// すべての写真の行に、今より 1 ミリ秒後の日時で印を付け直す（別のリクエストのレシピかワークスペースの削除をまねる）。
    fn remark_all_photos(&self) {
        let at = self.now_millis() + 1;
        let mut pending = self.pending_photos.lock().unwrap();
        for (_, _, photo) in self.photos.lock().unwrap().iter() {
            pending.insert(photo.id.clone(), at);
        }
    }

    /// 「今」の時刻を `millis` だけ進める。
    pub fn advance(&self, millis: i64) {
        self.now.fetch_add(millis, Ordering::Relaxed);
    }

    /// D1 か R2 の呼び出しを 1 回数える。上限（`max_calls`）を超えたら失敗を返す。
    fn count_call(&self, counter: &AtomicUsize) -> BackendResult<()> {
        counter.fetch_add(1, Ordering::Relaxed);
        let total = self.d1_calls.load(Ordering::Relaxed) + self.r2_calls.load(Ordering::Relaxed);
        if self.max_calls.is_some_and(|max| total > max) {
            return Err(BackendError("1 回の実行の呼び出しの上限を超えた".into()));
        }
        Ok(())
    }

    /// D1 の呼び出しを 1 回数える。
    fn count_d1(&self) -> BackendResult<()> {
        self.count_call(&self.d1_calls)
    }

    /// R2 の呼び出しを 1 回数える。
    fn count_r2(&self) -> BackendResult<()> {
        self.count_call(&self.r2_calls)
    }

    /// D1 の呼び出しを数え、DB が壊れたことにしているなら、失敗を返す。D1 のメソッドは、初めに 1 回だけ呼ぶ。
    fn check(&self) -> BackendResult<()> {
        self.count_d1()?;
        self.check_broken()
    }

    /// レシピかワークスペースの D1 の削除を失敗させることにしているなら、失敗を返す。
    fn check_owner_delete(&self) -> BackendResult<()> {
        if self.broken_owner_delete {
            Err(BackendError(
                "壊れた DB（レシピかワークスペースの削除）".into(),
            ))
        } else {
            Ok(())
        }
    }

    /// DB が壊れたことにしているなら、失敗を返す（呼び出しは数えない）。
    fn check_broken(&self) -> BackendResult<()> {
        if self.broken {
            Err(BackendError("壊れた DB".into()))
        } else {
            Ok(())
        }
    }

    /// レシピの登録と更新の前の [`Self::check`]。`broken_recipe_write` でも失敗を返す。
    fn check_recipe_write(&self) -> BackendResult<()> {
        if self.broken_recipe_write {
            return Err(BackendError("壊れた D1 の書き込み".into()));
        }
        self.check()
    }

    fn key(workspace: &WorkspaceId, id: &str) -> RecipeKey {
        (workspace.to_string(), id.to_owned())
    }
}

/// R2 のキーから、ワークスペースとレシピの ID を取り出す。キーが [`object_key`] の形でなければ panic する。
fn workspace_and_recipe_of(key: &str) -> (WorkspaceId, RecipeId) {
    let mut parts = key.split('/').skip(1);
    let workspace = parts.next().and_then(WorkspaceId::parse);
    let recipe = parts.next().and_then(RecipeId::parse);
    match (workspace, recipe) {
        (Some(workspace), Some(recipe)) => (workspace, recipe),
        _ => panic!("R2 のキーの形が photo::object_key と違う: {key}"),
    }
}

/// 偽物は待つ必要が無いので、`ready` ですぐに終わる `Future` を返す。
impl Backend for FakeBackend {
    fn random_128(&self) -> [u8; 16] {
        let mut bytes = [0xab; 16];
        bytes[0] = self.next_random.fetch_add(1, Ordering::Relaxed);
        bytes
    }

    fn now_millis(&self) -> i64 {
        self.now.load(Ordering::Relaxed)
    }

    fn demo_workspace_id(&self) -> Option<WorkspaceId> {
        self.demo.as_deref().and_then(WorkspaceId::parse)
    }

    fn allow_workspace_creation(
        &self,
        client_key: &str,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        self.rate_limit_keys
            .lock()
            .unwrap()
            .push(client_key.to_owned());
        ready(Ok(!self.rate_limited))
    }

    fn allow_write(&self, client_key: &str) -> impl Future<Output = BackendResult<bool>> + Send {
        self.write_limit_keys
            .lock()
            .unwrap()
            .push(client_key.to_owned());
        ready(Ok(!self.write_limited))
    }

    fn insert_workspace(
        &self,
        id: &WorkspaceId,
        now: i64,
        limits: &WorkspaceLimits,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        ready(self.check().map(|()| {
            // 本物の SQL と同じく、全体の数と since 以後に作った数を数えてから保存する。
            // 1 日に作った数は、消しても残る作った記録で数える。
            let mut workspaces = self.workspaces.lock().unwrap();
            let mut creations = self.workspace_creations.lock().unwrap();
            let total = i64::try_from(workspaces.len()).unwrap();
            let recent =
                i64::try_from(creations.iter().filter(|&&t| t >= limits.since).count()).unwrap();
            if total >= limits.max_total || recent >= limits.max_created_since {
                return false;
            }
            creations.push(now);
            workspaces.insert(id.to_string(), now);
            true
        }))
    }

    fn workspace_status(
        &self,
        id: &WorkspaceId,
    ) -> impl Future<Output = BackendResult<Option<WorkspaceStatus>>> + Send {
        ready(self.check().map(|()| {
            let last_used_at = *self.workspaces.lock().unwrap().get(id.as_str())?;
            Some(WorkspaceStatus {
                last_used_at,
                pinned: self.pinned.lock().unwrap().contains(id.as_str()),
            })
        }))
    }

    fn list_unused_workspaces(
        &self,
        before: i64,
    ) -> impl Future<Output = BackendResult<Vec<UnusedWorkspace>>> + Send {
        ready(self.check().and_then(|()| {
            if self.broken_list {
                return Err(BackendError("壊れた一覧".into()));
            }
            let pinned = self.pinned.lock().unwrap();
            let mut rows: Vec<UnusedWorkspace> = self
                .workspaces
                .lock()
                .unwrap()
                .iter()
                .filter(|(id, last_used_at)| **last_used_at < before && !pinned.contains(*id))
                .map(|(id, &last_used_at)| UnusedWorkspace {
                    id: id.clone(),
                    last_used_at,
                })
                .collect();
            // 本物の SQL は順番を決めないので、呼び出し側が並べることを確かめるため、新しい順に返す。
            rows.sort_by(|a, b| (b.last_used_at, &b.id).cmp(&(a.last_used_at, &a.id)));
            if let Some(used) = &self.used_after_listing {
                let now = self.now_millis();
                self.workspaces.lock().unwrap().insert(used.clone(), now);
            }
            Ok(rows)
        }))
    }

    fn delete_old_records(&self, before: i64) -> impl Future<Output = BackendResult<()>> + Send {
        ready(self.check().and_then(|()| {
            if self.broken_records {
                return Err(BackendError("壊れた記録".into()));
            }
            self.photo_uploads.lock().unwrap().retain(|&t| t >= before);
            self.workspace_creations
                .lock()
                .unwrap()
                .retain(|&t| t >= before);
            Ok(())
        }))
    }

    fn workspace_last_used_at(
        &self,
        id: &WorkspaceId,
    ) -> impl Future<Output = BackendResult<Option<i64>>> + Send {
        ready(
            self.check()
                .map(|()| self.workspaces.lock().unwrap().get(id.as_str()).copied()),
        )
    }

    fn set_workspace_last_used_at(
        &self,
        id: &WorkspaceId,
        now: i64,
    ) -> impl Future<Output = BackendResult<()>> + Send {
        ready(self.check().map(|()| {
            self.last_used_writes.fetch_add(1, Ordering::Relaxed);
            if let Some(last_used_at) = self.workspaces.lock().unwrap().get_mut(id.as_str()) {
                *last_used_at = now;
            }
        }))
    }

    fn list_recipes(
        &self,
        workspace: &WorkspaceId,
        query: Option<&str>,
        limit: i64,
        offset: i64,
        with_first_photo: bool,
    ) -> impl Future<Output = BackendResult<RecipePage>> + Send {
        ready(self.check().map(|()| {
            // 本物の `instr(search_name, ?) > 0` と同じく、検索に使う形の料理名が query を含むかで絞る。
            let mut recipes: Vec<Recipe> = self
                .recipes
                .lock()
                .unwrap()
                .iter()
                .filter(|((ws, _), _)| ws == workspace.as_str())
                .map(|(_, recipe)| recipe.clone())
                .filter(|recipe| query.is_none_or(|q| search_name(&recipe.name).contains(q)))
                .collect();
            recipes.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
            let total = i64::try_from(recipes.len()).unwrap();
            // 本物の副問い合わせと同じく、写真の一覧（追加した日時の古い順、同じ日時なら ID の小さい順）の最初を先頭にする。
            let photos = self.photos.lock().unwrap();
            let recipes = recipes
                .into_iter()
                .skip(usize::try_from(offset).unwrap())
                .take(usize::try_from(limit).unwrap())
                .map(|recipe| {
                    let first_photo_id = photos
                        .iter()
                        .filter(|_| with_first_photo)
                        .filter(|(w, r, _)| w == workspace.as_str() && *r == recipe.id)
                        .map(|(_, _, photo)| photo)
                        .filter(|photo| self.is_visible(photo))
                        .min_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)))
                        .map(|photo| photo.id.clone());
                    ListedRecipe {
                        recipe,
                        first_photo_id,
                    }
                })
                .collect();
            RecipePage { recipes, total }
        }))
    }

    fn insert_recipe(
        &self,
        workspace: &WorkspaceId,
        recipe: &Recipe,
        max_recipes: i64,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        ready(self.check_recipe_write().map(|()| {
            let mut recipes = self.recipes.lock().unwrap();
            let count = recipes
                .keys()
                .filter(|(ws, _)| ws == workspace.as_str())
                .count();
            if i64::try_from(count).unwrap() >= max_recipes {
                return false;
            }
            recipes.insert(Self::key(workspace, &recipe.id), recipe.clone());
            true
        }))
    }

    fn get_recipe(
        &self,
        workspace: &WorkspaceId,
        id: &RecipeId,
    ) -> impl Future<Output = BackendResult<Option<Recipe>>> + Send {
        ready(self.check().map(|()| {
            self.recipes
                .lock()
                .unwrap()
                .get(&Self::key(workspace, id.as_str()))
                .cloned()
        }))
    }

    fn update_recipe(
        &self,
        workspace: &WorkspaceId,
        id: &RecipeId,
        fields: &RecipeFields,
        now: i64,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        ready(self.check_recipe_write().map(|()| {
            let mut recipes = self.recipes.lock().unwrap();
            let Some(recipe) = recipes.get_mut(&Self::key(workspace, id.as_str())) else {
                return false;
            };
            recipe.name.clone_from(&fields.name);
            recipe.servings = fields.servings;
            recipe.ingredients.clone_from(&fields.ingredients);
            recipe.steps.clone_from(&fields.steps);
            recipe.updated_at = now;
            true
        }))
    }

    fn delete_recipe(
        &self,
        workspace: &WorkspaceId,
        id: &RecipeId,
    ) -> impl Future<Output = BackendResult<Option<Vec<String>>>> + Send {
        ready(
            self.check()
                .and_then(|()| self.check_owner_delete())
                .map(|()| {
                    self.recipes
                        .lock()
                        .unwrap()
                        .remove(&Self::key(workspace, id.as_str()))?;
                    // D1 の ON DELETE CASCADE と同じく、レシピの写真の行も消す。
                    let mut photos = self.photos.lock().unwrap();
                    let removed = photos
                        .iter()
                        .filter(|(w, r, _)| w == workspace.as_str() && r == id.as_str())
                        .map(|(_, _, p)| p.id.clone())
                        .collect();
                    photos.retain(|(w, r, _)| !(w == workspace.as_str() && r == id.as_str()));
                    self.forget_pending(&removed);
                    Some(removed)
                }),
        )
    }

    fn hide_workspace_photos(
        &self,
        workspace: &WorkspaceId,
        now: i64,
        last_used_before: Option<i64>,
    ) -> impl Future<Output = BackendResult<Vec<PhotoKey>>> + Send {
        ready(self.check().and_then(|()| {
            if self.broken_photo_pending {
                return Err(BackendError("壊れた DB（写真の印）".into()));
            }
            // 本物の SQL と同じく、delete_workspace で消す条件のワークスペースのときだけ、読んで隠す。
            let Some(&last_used_at) = self.workspaces.lock().unwrap().get(workspace.as_str())
            else {
                return Ok(Vec::new());
            };
            if self.pinned.lock().unwrap().contains(workspace.as_str())
                || last_used_before.is_some_and(|before| last_used_at >= before)
            {
                return Ok(Vec::new());
            }
            let keys = self.workspace_photo_keys(workspace);
            let mut pending = self.pending_photos.lock().unwrap();
            for key in &keys {
                pending.insert(key.id.clone(), now);
            }
            if self.used_after_hiding.as_deref() == Some(workspace.as_str()) {
                let now = self.now_millis();
                self.workspaces
                    .lock()
                    .unwrap()
                    .insert(workspace.to_string(), now);
            }
            Ok(keys)
        }))
    }

    fn delete_workspace(
        &self,
        workspace: &WorkspaceId,
        last_used_before: Option<i64>,
    ) -> impl Future<Output = BackendResult<Option<Vec<PhotoKey>>>> + Send {
        ready(
            self.check()
                .and_then(|()| self.check_owner_delete())
                .map(|()| {
                    // 本物の SQL と同じく、印のあるワークスペースと、last_used_before より後に使われたワークスペースは消さない。
                    let mut workspaces = self.workspaces.lock().unwrap();
                    let last_used_at = *workspaces.get(workspace.as_str())?;
                    if self.pinned.lock().unwrap().contains(workspace.as_str())
                        || last_used_before.is_some_and(|before| last_used_at >= before)
                    {
                        return None;
                    }
                    workspaces.remove(workspace.as_str());
                    drop(workspaces);
                    // D1 の ON DELETE CASCADE と同じく、レシピと写真の行も消す。
                    let removed = self.workspace_photo_keys(workspace);
                    self.recipes
                        .lock()
                        .unwrap()
                        .retain(|(w, _), _| w != workspace.as_str());
                    self.photos
                        .lock()
                        .unwrap()
                        .retain(|(w, _, _)| w != workspace.as_str());
                    self.forget_pending(removed.iter().map(|key| &key.id));
                    // 別のリクエストが先に（同じように）消していたなら、この削除では何も消えていない。
                    if self.workspace_vanishes_on_delete {
                        return None;
                    }
                    Some(removed)
                }),
        )
    }

    fn insert_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        photo: &Photo,
        limits: &PhotoLimits,
    ) -> impl Future<Output = BackendResult<PhotoInsert>> + Send {
        ready(self.check().map(|()| {
            if !self
                .recipes
                .lock()
                .unwrap()
                .contains_key(&Self::key(workspace, recipe.as_str()))
            {
                return PhotoInsert::RecipeNotFound;
            }
            let mut photos = self.photos.lock().unwrap();
            let mut uploads = self.photo_uploads.lock().unwrap();
            let in_recipe = photos
                .iter()
                .filter(|(w, r, _)| w == workspace.as_str() && r == recipe.as_str())
                .count();
            let workspace_bytes: i64 = photos
                .iter()
                .filter(|(w, _, _)| w == workspace.as_str())
                .map(|(_, _, p)| p.size)
                .sum();
            let uploads_since = uploads.iter().filter(|&&at| at >= limits.since).count();
            if i64::try_from(in_recipe).unwrap() >= limits.max_per_recipe {
                return PhotoInsert::RecipeFull;
            }
            if workspace_bytes + photo.size > limits.max_bytes_per_workspace {
                return PhotoInsert::WorkspaceFull;
            }
            if i64::try_from(uploads_since).unwrap() >= limits.max_uploads_since {
                return PhotoInsert::CapacityReached;
            }
            photos.push((workspace.to_string(), recipe.to_string(), photo.clone()));
            // 本物と同じく、R2 への書き込みが終わっていない印を付けて入れる。
            self.mark_pending(&photo.id, photo.created_at);
            uploads.push(photo.created_at);
            PhotoInsert::Inserted
        }))
    }

    fn hide_recipe_photos(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        now: i64,
    ) -> impl Future<Output = BackendResult<Vec<String>>> + Send {
        ready(self.check().and_then(|()| {
            if self.broken_photo_pending {
                return Err(BackendError("壊れた DB（写真の印）".into()));
            }
            let ids: Vec<String> = self
                .photos
                .lock()
                .unwrap()
                .iter()
                .filter(|(w, r, _)| w == workspace.as_str() && r == recipe.as_str())
                .map(|(_, _, p)| p.id.clone())
                .collect();
            // 本物と同じく、印の付いた行も付け直す。
            let mut pending = self.pending_photos.lock().unwrap();
            for id in &ids {
                pending.insert(id.clone(), now);
            }
            Ok(ids)
        }))
    }

    fn list_photos(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
    ) -> impl Future<Output = BackendResult<Vec<Photo>>> + Send {
        ready(self.check().map(|()| {
            let mut photos: Vec<Photo> = self
                .photos
                .lock()
                .unwrap()
                .iter()
                .filter(|(w, r, _)| w == workspace.as_str() && r == recipe.as_str())
                .map(|(_, _, p)| p.clone())
                .filter(|p| self.is_visible(p))
                .collect();
            photos.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
            photos
        }))
    }

    fn get_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
    ) -> impl Future<Output = BackendResult<Option<Photo>>> + Send {
        ready(self.check().map(|()| {
            self.photos
                .lock()
                .unwrap()
                .iter()
                .find(|(w, r, p)| {
                    w == workspace.as_str() && r == recipe.as_str() && p.id == id.as_str()
                })
                .map(|(_, _, p)| p.clone())
                .filter(|p| self.is_visible(p))
        }))
    }

    fn delete_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        if let Err(error) = self.count_d1() {
            return ready(Err(error));
        }
        if self.broken_photo_delete {
            return ready(Err(BackendError("壊れた DB（写真の削除）".into())));
        }
        ready(self.check_broken().map(|()| {
            let mut photos = self.photos.lock().unwrap();
            let before = photos.len();
            photos.retain(|(w, r, p)| {
                !(w == workspace.as_str() && r == recipe.as_str() && p.id == id.as_str())
            });
            let deleted = photos.len() < before;
            if deleted {
                self.forget_pending([&id.as_str().to_owned()]);
            }
            deleted
        }))
    }

    fn finish_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
        marked_at: i64,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        ready(self.set_pending(workspace, recipe, id, false, marked_at))
    }

    fn hide_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
        now: i64,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        ready(self.set_pending(workspace, recipe, id, true, now))
    }

    fn list_pending_photos(
        &self,
        before: i64,
    ) -> impl Future<Output = BackendResult<Vec<PendingPhoto>>> + Send {
        ready(self.check().map(|()| {
            self.photos
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, _, p)| {
                    self.pending_photos
                        .lock()
                        .unwrap()
                        .get(&p.id)
                        .is_some_and(|&since| since < before)
                })
                .map(|(w, r, p)| PendingPhoto {
                    workspace_id: w.clone(),
                    recipe_id: r.clone(),
                    id: p.id.clone(),
                })
                .collect()
        }))
    }

    fn delete_photos(
        &self,
        photos: &[(WorkspaceId, RecipeId, PhotoId)],
        marked_before: i64,
    ) -> impl Future<Output = BackendResult<()>> + Send {
        if let Err(error) = self.count_d1() {
            return ready(Err(error));
        }
        if self.broken_photo_delete
            || photos
                .iter()
                .any(|(_, _, id)| self.undeletable_photos.iter().any(|u| u == id.as_str()))
        {
            return ready(Err(BackendError("壊れた DB（写真の削除）".into())));
        }
        ready(self.check_broken().map(|()| {
            // 本物の batch と同じく、まとめて消す（途中で失敗して一部だけが消えることは無い）。
            // 印を付けた日時が marked_before より前の行だけを消し、消した行の印だけを消す。
            let marked = self.pending_photos.lock().unwrap().clone();
            let mut rows = self.photos.lock().unwrap();
            let mut deleted = Vec::new();
            rows.retain(|(w, r, p)| {
                let target = photos.iter().any(|(workspace, recipe, id)| {
                    w == workspace.as_str() && r == recipe.as_str() && p.id == id.as_str()
                }) && marked
                    .get(&p.id)
                    .is_some_and(|&since| since < marked_before);
                if target {
                    deleted.push(p.id.clone());
                }
                !target
            });
            drop(rows);
            self.forget_pending(&deleted);
        }))
    }

    // 偽物には接続が無いので、そのまま待つ。
    async fn run_to_completion<T: Send + 'static>(
        &self,
        task: impl Future<Output = T> + Send + 'static,
    ) -> Option<T> {
        if self.task_dropped {
            return None;
        }
        Some(task.await)
    }

    fn put_object(
        &self,
        key: &str,
        body: PhotoBody,
        content_type: &str,
    ) -> impl Future<Output = BackendResult<()>> + Send {
        // テストの本文は、いつもバイト列で届く（ストリームは本番の wasm でだけ作る）。
        let PhotoBody::Bytes(bytes) = body else {
            panic!("テストでストリームの本文は作らない");
        };
        if let Err(error) = self.count_r2() {
            return ready(Err(error));
        }
        if self.photo_rows_vanish_on_put {
            self.photos.lock().unwrap().clear();
        }
        if self.photo_rows_remarked_on_put {
            self.remark_all_photos();
        }
        ready(if self.broken_r2_put {
            Err(BackendError("壊れた R2".into()))
        } else {
            self.objects
                .lock()
                .unwrap()
                .insert(key.to_owned(), (bytes, content_type.to_owned()));
            Ok(())
        })
    }

    fn get_object(&self, key: &str) -> impl Future<Output = BackendResult<Option<Vec<u8>>>> + Send {
        if let Err(error) = self.count_r2() {
            return ready(Err(error));
        }
        ready(Ok(self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .map(|(bytes, _)| bytes.clone())))
    }

    fn delete_objects(&self, keys: Vec<String>) -> impl Future<Output = BackendResult<()>> + Send {
        if let Err(error) = self.count_r2() {
            return ready(Err(error));
        }
        if self.photo_rows_remarked_on_delete {
            self.remark_all_photos();
        }
        if self.broken_r2_delete {
            return ready(Err(BackendError("壊れた R2".into())));
        }
        {
            let mut objects = self.objects.lock().unwrap();
            for key in &keys {
                objects.remove(key);
            }
        }
        if self.photo_added_after_first_delete
            && !keys.is_empty()
            && !self.injected_photo.swap(true, Ordering::Relaxed)
        {
            // 消したキーと同じレシピに、別の写真を足す（キーの形は photo::object_key）。
            let (workspace, recipe) = workspace_and_recipe_of(&keys[0]);
            let id = "99999999999999999999999999999999";
            self.add_photo_row(workspace.as_str(), recipe.as_str(), id, 1);
            let mut objects = self.objects.lock().unwrap();
            objects.insert(
                object_key(&workspace, &recipe, id),
                (vec![0xff], "image/jpeg".to_owned()),
            );
            // 一覧に載っていた写真のデータが、消した後に書き込まれた場合（追加の途中の写真）。
            objects.insert(keys[0].clone(), (vec![0xff], "image/jpeg".to_owned()));
        }
        ready(Ok(()))
    }
}
