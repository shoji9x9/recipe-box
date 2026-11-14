//! アプリが外の世界（DB・レート制限・乱数・時計）に頼る部分。
//!
//! 本番では Cloudflare の機能を使う実装（`crate::cloudflare`）を、テストではメモリの中で動く偽物を使う。
//! D1 などは wasm の上でしか動かないので、こうして分けておくと、普通の `cargo test` で API を確かめられる。

use std::fmt;
use std::future::Future;

use crate::{
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

    /// レシピを消す。レシピが無ければ `false` を返す。
    fn delete_recipe(
        &self,
        workspace: &WorkspaceId,
        id: &RecipeId,
    ) -> impl Future<Output = BackendResult<bool>> + Send;
}
