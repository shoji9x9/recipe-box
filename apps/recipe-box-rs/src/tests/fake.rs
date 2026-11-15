//! メモリの中で動く、テスト用の [`Backend`]。
//!
//! D1 と R2 の代わりに `HashMap` などにデータを持つ。レシピはワークスペースの ID とレシピの ID の組で持ち、
//! 本物の SQL と同じく、指定したワークスペースのレシピだけを読み書きする。

use std::{
    collections::HashMap,
    future::{Future, ready},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicUsize, Ordering},
    },
};

use crate::{
    backend::{
        Backend, BackendError, BackendResult, PhotoInsert, PhotoKey, PhotoLimits, RecipePage,
        WorkspaceLimits,
    },
    photo::{Photo, PhotoId, object_key},
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
    /// 写真を追加した記録（追加した日時）。
    pub photo_uploads: Arc<Mutex<Vec<i64>>>,
    /// R2 のオブジェクト。キーと、データ・形式。
    pub objects: Arc<Mutex<Objects>>,
    /// `true` なら、R2 への書き込みが失敗したことにする。
    pub broken_r2_put: bool,
    /// `true` なら、R2 に書き込んでいる間に、別のリクエストがすべての写真の行を消したことにする。
    pub photo_rows_vanish_on_put: bool,
    /// `true` なら、R2 の削除が失敗したことにする。
    pub broken_r2_delete: bool,
    /// `true` なら、最初の R2 の削除の直後に、別のリクエストが同じレシピに写真を 1 枚追加し（D1 の行と R2 のデータ）、
    /// 消したばかりのキーにも書き込んだことにする。レシピの削除と写真の追加が同時に起きた場合を確かめる。
    pub photo_added_after_first_delete: bool,
    /// `photo_added_after_first_delete` の写真を、もう追加したか。
    pub injected_photo: Arc<AtomicBool>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self {
            workspaces: Arc::default(),
            workspace_creations: Arc::default(),
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
            photo_uploads: Arc::default(),
            objects: Arc::default(),
            broken_r2_put: false,
            photo_rows_vanish_on_put: false,
            broken_r2_delete: false,
            photo_added_after_first_delete: false,
            injected_photo: Arc::default(),
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
        self.photos.lock().unwrap().push((
            workspace.to_owned(),
            recipe.to_owned(),
            Photo {
                id: id.to_owned(),
                content_type: "image/jpeg".to_owned(),
                size,
                created_at: self.now_millis(),
            },
        ));
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
    ) -> impl Future<Output = BackendResult<Option<Vec<String>>>> + Send {
        ready(self.check().map(|()| {
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
            Some(removed)
        }))
    }

    fn list_workspace_photos(
        &self,
        workspace: &WorkspaceId,
    ) -> impl Future<Output = BackendResult<Vec<PhotoKey>>> + Send {
        ready(self.check().map(|()| self.workspace_photo_keys(workspace)))
    }

    fn delete_workspace(
        &self,
        workspace: &WorkspaceId,
    ) -> impl Future<Output = BackendResult<Option<Vec<PhotoKey>>>> + Send {
        ready(self.check().map(|()| {
            self.workspaces.lock().unwrap().remove(workspace.as_str())?;
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
            Some(removed)
        }))
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
            let total_bytes: i64 = photos.iter().map(|(_, _, p)| p.size).sum();
            let uploads_since = uploads.iter().filter(|&&at| at >= limits.since).count();
            if i64::try_from(in_recipe).unwrap() >= limits.max_per_recipe {
                return PhotoInsert::RecipeFull;
            }
            if workspace_bytes + photo.size > limits.max_bytes_per_workspace {
                return PhotoInsert::WorkspaceFull;
            }
            if total_bytes + photo.size > limits.max_bytes_total
                || i64::try_from(uploads_since).unwrap() >= limits.max_uploads_since
            {
                return PhotoInsert::CapacityReached;
            }
            photos.push((workspace.to_string(), recipe.to_string(), photo.clone()));
            uploads.push(photo.created_at);
            PhotoInsert::Inserted
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
        }))
    }

    fn delete_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        ready(self.check().map(|()| {
            let mut photos = self.photos.lock().unwrap();
            let before = photos.len();
            photos.retain(|(w, r, p)| {
                !(w == workspace.as_str() && r == recipe.as_str() && p.id == id.as_str())
            });
            photos.len() < before
        }))
    }

    fn put_object(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> impl Future<Output = BackendResult<()>> + Send {
        if self.photo_rows_vanish_on_put {
            self.photos.lock().unwrap().clear();
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
        ready(Ok(self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .map(|(bytes, _)| bytes.clone())))
    }

    fn delete_objects(&self, keys: Vec<String>) -> impl Future<Output = BackendResult<()>> + Send {
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
