//! アプリが外の世界（DB・オブジェクトストレージ・レート制限・乱数・時計）に頼る部分。
//!
//! 本番では Cloudflare の機能を使う実装（`crate::cloudflare`）を、テストではメモリの中で動く偽物を使う。
//! D1 などは wasm の上でしか動かないので、こうして分けておくと、普通の `cargo test` で API を確かめられる。

use std::fmt;
use std::future::Future;

use crate::{
    photo::{Photo, PhotoId},
    recipe::{Recipe, RecipeFields, RecipeId},
    workspace::WorkspaceId,
};

/// 外の世界とのやり取りで起きた失敗。中身はログに出すだけで、利用者には見せない。
#[derive(Debug)]
pub struct BackendError(pub String);

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `Backend` のメソッドの戻り値。
pub type BackendResult<T> = Result<T, BackendError>;

/// ワークスペースを作るときに確かめる、アプリ全体の上限。
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceLimits {
    /// ワークスペースの数の上限。
    pub max_total: i64,
    /// `since` 以後に作ったワークスペースの数の上限。
    pub max_created_since: i64,
    /// 数え始める日時（UNIX 時間のミリ秒）。
    pub since: i64,
}

/// 写真を追加するときに確かめる上限。
#[derive(Debug, Clone, Copy)]
pub struct PhotoLimits {
    /// 1 つのレシピの写真の数の上限。
    pub max_per_recipe: i64,
    /// 1 つのワークスペースの写真の大きさの合計の上限（バイト）。
    pub max_bytes_per_workspace: i64,
    /// アプリ全体の写真の大きさの合計の上限（バイト）。
    pub max_bytes_total: i64,
    /// `since` 以後にアプリ全体で写真を追加した回数の上限。
    pub max_uploads_since: i64,
    /// 数え始める日時（UNIX 時間のミリ秒）。
    pub since: i64,
}

/// 写真を指す、レシピの ID と写真の ID の組（R2 のキーを作るのに使う）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PhotoKey {
    pub recipe_id: String,
    pub id: String,
}

/// 写真の行を保存した結果。保存しなかったときは、その理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoInsert {
    Inserted,
    /// そのワークスペースに、そのレシピが無い。
    RecipeNotFound,
    /// レシピの写真の数が上限に達している。
    RecipeFull,
    /// 足すと、ワークスペースの写真の大きさの合計が上限を超える。
    WorkspaceFull,
    /// 足すと、アプリ全体の写真の大きさの合計か、1 日の追加の回数が上限を超える。
    CapacityReached,
}

/// レシピの一覧の 1 ページ分。
#[derive(Debug)]
pub struct RecipePage {
    /// このページのレシピ。
    pub recipes: Vec<Recipe>,
    /// 条件に当てはまるレシピの総数（すべてのページの合計）。
    pub total: i64,
}

/// ハンドラーが使う外の機能。
///
/// axum はハンドラーの `Future` に `Send`（別のスレッドに渡せること）を求めるので、戻り値に `+ Send` を付ける。
/// `Clone` は、axum がリクエストごとに状態を複製するために要る。
///
/// レシピを扱うメソッドは、必ずワークスペースの ID を受け取る。
/// 実装は、そのワークスペースのレシピだけを読み書きする（ほかのワークスペースのレシピには触れない）。
pub trait Backend: Clone + Send + Sync + 'static {
    /// 暗号に使える質の 128 ビットの乱数を返す。
    fn random_128(&self) -> [u8; 16];

    /// 今の時刻を UNIX 時間のミリ秒で返す。
    fn now_millis(&self) -> i64;

    /// 共有のデモのワークスペースの ID。設定されていなければ `None`。
    ///
    /// ID を知れば誰でも API で書き込めるので、画面には出さない。
    fn demo_workspace_id(&self) -> Option<WorkspaceId>;

    /// ワークスペースを作ってよいかを、作る人を表すキー（IP アドレスなど）ごとの回数で決める。
    fn allow_workspace_creation(
        &self,
        client_key: &str,
    ) -> impl Future<Output = BackendResult<bool>> + Send;

    /// 書き込む API を呼んでよいかを、呼ぶ人を表すキー（IP アドレスなど）ごとの回数で決める。
    fn allow_write(&self, client_key: &str) -> impl Future<Output = BackendResult<bool>> + Send;

    /// ワークスペースを保存する。`limits` のどれかに達していれば保存せずに `false` を返す。
    /// 数えることと保存することを 1 つの SQL で行い、同時に作られても上限を超えないようにする。
    fn insert_workspace(
        &self,
        id: &WorkspaceId,
        now: i64,
        limits: &WorkspaceLimits,
    ) -> impl Future<Output = BackendResult<bool>> + Send;

    /// ワークスペースの最終利用日時を返す。ワークスペースが無ければ `None` を返す。
    fn workspace_last_used_at(
        &self,
        id: &WorkspaceId,
    ) -> impl Future<Output = BackendResult<Option<i64>>> + Send;

    /// ワークスペースの最終利用日時を `now` にする。
    fn set_workspace_last_used_at(
        &self,
        id: &WorkspaceId,
        now: i64,
    ) -> impl Future<Output = BackendResult<()>> + Send;

    /// ワークスペースのレシピを、作った日時の新しい順（同じ日時なら ID の大きい順）に並べ、
    /// `offset` 件を飛ばした後の `limit` 件と、当てはまるレシピの総数を返す。
    /// `query` があれば、`search_name` 列がそれを含むレシピだけを対象にする。
    /// `query` は呼び出し側で [`crate::recipe::search_name`] の形にしてから渡す。
    fn list_recipes(
        &self,
        workspace: &WorkspaceId,
        query: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> impl Future<Output = BackendResult<RecipePage>> + Send;

    /// レシピを保存する。ワークスペースのレシピがすでに `max_recipes` 件あれば、保存せずに `false` を返す。
    /// 数えることと保存することを 1 つの SQL で行い、同時に登録されても上限を超えないようにする。
    fn insert_recipe(
        &self,
        workspace: &WorkspaceId,
        recipe: &Recipe,
        max_recipes: i64,
    ) -> impl Future<Output = BackendResult<bool>> + Send;

    /// レシピを 1 件返す。無ければ `None` を返す。
    fn get_recipe(
        &self,
        workspace: &WorkspaceId,
        id: &RecipeId,
    ) -> impl Future<Output = BackendResult<Option<Recipe>>> + Send;

    /// レシピの内容を `fields` で置き換え、最後に変えた日時を `now` にする。レシピが無ければ `false` を返す。
    fn update_recipe(
        &self,
        workspace: &WorkspaceId,
        id: &RecipeId,
        fields: &RecipeFields,
        now: i64,
    ) -> impl Future<Output = BackendResult<bool>> + Send;

    /// レシピと、その写真の行を消し、消した写真の ID を返す。レシピが無ければ `None` を返す。
    /// R2 のデータは消さない（呼び出し側が、返した ID で消す）。
    ///
    /// 写真の ID を読むことと消すことを 1 つのトランザクションで行う。間に写真が追加されて、
    /// 消した後の R2 に、どの行からもたどれないデータが残るのを防ぐため。
    fn delete_recipe(
        &self,
        workspace: &WorkspaceId,
        id: &RecipeId,
    ) -> impl Future<Output = BackendResult<Option<Vec<String>>>> + Send;

    /// ワークスペースのすべての写真を返す。
    fn list_workspace_photos(
        &self,
        workspace: &WorkspaceId,
    ) -> impl Future<Output = BackendResult<Vec<PhotoKey>>> + Send;

    /// ワークスペースと、そのレシピ・写真の行を消し、消した写真を返す。ワークスペースが無ければ `None` を返す。
    /// R2 のデータは消さない（呼び出し側が、返した写真で消す）。
    ///
    /// 写真を読むことと消すことを 1 つのトランザクションで行う（[`Backend::delete_recipe`] と同じ理由）。
    fn delete_workspace(
        &self,
        workspace: &WorkspaceId,
    ) -> impl Future<Output = BackendResult<Option<Vec<PhotoKey>>>> + Send;

    /// 写真の行と、追加の記録を保存する。`limits` のどれかに当たれば保存せず、その理由を返す。
    /// 上限の判定と保存を 1 つのトランザクションで行い、同時に追加されても上限を超えないようにする。
    fn insert_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        photo: &Photo,
        limits: &PhotoLimits,
    ) -> impl Future<Output = BackendResult<PhotoInsert>> + Send;

    /// レシピの写真の行を、追加した順（同じ日時なら ID の順）に返す。
    fn list_photos(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
    ) -> impl Future<Output = BackendResult<Vec<Photo>>> + Send;

    /// 写真の行を 1 件返す。無ければ `None` を返す。
    fn get_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
    ) -> impl Future<Output = BackendResult<Option<Photo>>> + Send;

    /// 写真の行を消す。行が無ければ `false` を返す。追加の記録は消さない。
    fn delete_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
    ) -> impl Future<Output = BackendResult<bool>> + Send;

    /// オブジェクトストレージ（R2）に、写真のデータを書き込む。
    fn put_object(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> impl Future<Output = BackendResult<()>> + Send;

    /// オブジェクトストレージから、写真のデータを読む。無ければ `None` を返す。
    fn get_object(&self, key: &str) -> impl Future<Output = BackendResult<Option<Vec<u8>>>> + Send;

    /// オブジェクトストレージから、写真のデータを消す。無いキーは無視する。
    fn delete_objects(&self, keys: Vec<String>) -> impl Future<Output = BackendResult<()>> + Send;
}
