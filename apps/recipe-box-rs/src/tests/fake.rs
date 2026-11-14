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
    backend::{Backend, BackendError, BackendResult},
    recipe::{Recipe, RecipeFields, RecipeId},
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
    /// レート制限に渡されたキー。
    pub rate_limit_keys: Arc<Mutex<Vec<String>>>,
    /// 最終利用日時を書き換えた回数。
    pub last_used_writes: Arc<AtomicUsize>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self {
            workspaces: Arc::default(),
            recipes: Arc::default(),
            next_random: Arc::default(),
            now: Arc::new(AtomicI64::new(NOW)),
            rate_limited: false,
            broken: false,
            rate_limit_keys: Arc::default(),
            last_used_writes: Arc::default(),
        }
    }
}

impl FakeBackend {
    /// 最終利用日時が `last_used_at` のワークスペースを足し、その ID を返す。
    pub fn add_workspace(&self, id: &str, last_used_at: i64) -> String {
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

    fn insert_workspace(
        &self,
        id: &WorkspaceId,
        now: i64,
    ) -> impl Future<Output = BackendResult<()>> + Send {
        ready(self.check().map(|()| {
            self.workspaces.lock().unwrap().insert(id.to_string(), now);
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
        name_pattern: Option<&str>,
    ) -> impl Future<Output = BackendResult<Vec<Recipe>>> + Send {
        ready(self.check().map(|()| {
            // 本物と同じく、ASCII の大文字と小文字だけを区別しない部分一致にする。
            let pattern = name_pattern.map(str::to_ascii_lowercase);
            let mut recipes: Vec<Recipe> = self
                .recipes
                .lock()
                .unwrap()
                .iter()
                .filter(|((ws, _), _)| ws == workspace.as_str())
                .map(|(_, recipe)| recipe.clone())
                .filter(|recipe| {
                    pattern
                        .as_ref()
                        .is_none_or(|p| recipe.name.to_ascii_lowercase().contains(p.as_str()))
                })
                .collect();
            recipes.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
            recipes
        }))
    }

    fn insert_recipe(
        &self,
        workspace: &WorkspaceId,
        recipe: &Recipe,
    ) -> impl Future<Output = BackendResult<()>> + Send {
        ready(self.check().map(|()| {
            self.recipes
                .lock()
                .unwrap()
                .insert(Self::key(workspace, &recipe.id), recipe.clone());
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
