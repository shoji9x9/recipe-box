//! メモリの中で動く、テスト用の [`Backend`]。
//!
//! D1 の代わりに `HashMap` にデータを持つ。レシピはワークスペースの ID とレシピの ID の組で持ち、
//! 本物の SQL と同じく、指定したワークスペースのレシピだけを読み書きする。

use std::{
    collections::HashMap,
    future::{Future, ready},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicU8, AtomicUsize, Ordering},
    },
};

use crate::{
    backend::{Backend, BackendError, BackendResult, RecipePage, WorkspaceLimits},
    recipe::{Recipe, RecipeFields, RecipeId, search_name},
    workspace::WorkspaceId,
};

/// テストの中の「今」の時刻の初めの値。
pub const NOW: i64 = 1_800_000_000_000;

/// ワークスペースの ID とレシピの ID の組。
type RecipeKey = (String, String);

#[derive(Clone)]
pub struct FakeBackend {
    /// ワークスペースの ID と最終利用日時。
    pub workspaces: Arc<Mutex<HashMap<String, i64>>>,
    /// ワークスペースの ID と作った日時。
    pub workspace_created_at: Arc<Mutex<HashMap<String, i64>>>,
    /// レシピ。
    pub recipes: Arc<Mutex<HashMap<RecipeKey, Recipe>>>,
    /// 次に返す乱数の 1 バイト目。呼ぶたびに 1 増やし、毎回ちがう ID になるようにする。
    pub next_random: Arc<AtomicU8>,
    /// 「今」の時刻。テストから進められる。
    pub now: Arc<AtomicI64>,
    /// `true` なら、ワークスペースを作る回数の制限を超えたことにする。
    pub rate_limited: bool,
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
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self {
            workspaces: Arc::default(),
            workspace_created_at: Arc::default(),
            recipes: Arc::default(),
            next_random: Arc::default(),
            now: Arc::new(AtomicI64::new(NOW)),
            rate_limited: false,
            broken: false,
            write_limited: false,
            rate_limit_keys: Arc::default(),
            write_limit_keys: Arc::default(),
            last_used_writes: Arc::default(),
        }
    }
}

impl FakeBackend {
    /// 最終利用日時が `last_used_at` のワークスペースを足し、その ID を返す。作った日時も `last_used_at` にする。
    pub fn add_workspace(&self, id: &str, last_used_at: i64) -> String {
        self.workspace_created_at
            .lock()
            .unwrap()
            .insert(id.to_owned(), last_used_at);
        self.workspaces
            .lock()
            .unwrap()
            .insert(id.to_owned(), last_used_at);
        id.to_owned()
    }

    /// 「今」の時刻を `millis` だけ進める。
    pub fn advance(&self, millis: i64) {
        self.now.fetch_add(millis, Ordering::Relaxed);
    }

    /// DB が壊れたことにしているなら、失敗を返す。
    fn check(&self) -> BackendResult<()> {
        if self.broken {
            Err(BackendError("壊れた DB".into()))
        } else {
            Ok(())
        }
    }

    fn key(workspace: &WorkspaceId, id: &str) -> RecipeKey {
        (workspace.to_string(), id.to_owned())
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
            let mut created_at = self.workspace_created_at.lock().unwrap();
            let total = i64::try_from(created_at.len()).unwrap();
            let recent =
                i64::try_from(created_at.values().filter(|&&t| t >= limits.since).count()).unwrap();
            if total >= limits.max_total || recent >= limits.max_created_since {
                return false;
            }
            created_at.insert(id.to_string(), now);
            self.workspaces.lock().unwrap().insert(id.to_string(), now);
            true
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
            let recipes = recipes
                .into_iter()
                .skip(usize::try_from(offset).unwrap())
                .take(usize::try_from(limit).unwrap())
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
        ready(self.check().map(|()| {
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
        ready(self.check().map(|()| {
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
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        ready(self.check().map(|()| {
            self.recipes
                .lock()
                .unwrap()
                .remove(&Self::key(workspace, id.as_str()))
                .is_some()
        }))
    }
}
