//! Cloudflare の機能（D1・Rate Limiting・Web Crypto）を使う [`Backend`] の実装。
//!
//! JavaScript の値は `Send` でないが、Workers は 1 つのスレッドで動くので、
//! workers-rs の `SendWrapper`・`SendFuture` で包んで axum に渡しても問題が無い。
//!
//! SQL は標準の範囲で書き、レシピを扱うクエリは必ず `workspace_id` で絞り込む（AGENTS.md）。

use std::future::Future;

use wasm_bindgen::prelude::wasm_bindgen;
use worker::{
    D1Database, D1Result, Env,
    send::{SendFuture, SendWrapper},
    wasm_bindgen::JsValue,
};

use crate::{
    backend::{Backend, BackendError, BackendResult, RecipePage, WorkspaceLimits},
    recipe::{Recipe, RecipeFields, RecipeId, search_name},
    workspace::WorkspaceId,
};

/// `wrangler.toml` の D1 の binding の名前。
const DB_BINDING: &str = "DB";
/// 共有のデモのワークスペースの ID を置く secret の名前。
const DEMO_WORKSPACE_SECRET: &str = "DEMO_WORKSPACE_ID";
/// `wrangler.toml` のレート制限の binding の名前（ワークスペースを作る回数）。
const WORKSPACE_CREATION_LIMITER: &str = "WORKSPACE_CREATION_LIMITER";
/// `wrangler.toml` のレート制限の binding の名前（書き込む API の回数）。
const WRITE_LIMITER: &str = "WRITE_LIMITER";

/// レシピを読むときの列。`Recipe` の項目と同じ名前・順番にする。
const RECIPE_COLUMNS: &str = "id, name, servings, ingredients, steps, created_at, updated_at";

#[wasm_bindgen]
extern "C" {
    /// Web Crypto の `crypto.getRandomValues()`。暗号に使える質の乱数で `buf` を埋める。
    #[wasm_bindgen(js_namespace = crypto, js_name = getRandomValues)]
    fn get_random_values(buf: &mut [u8]);
}

/// Workers の環境（binding の入れ物）を持つ [`Backend`]。
#[derive(Clone)]
pub struct CloudflareBackend {
    env: SendWrapper<Env>,
}

impl CloudflareBackend {
    pub fn new(env: Env) -> Self {
        Self {
            env: SendWrapper::new(env),
        }
    }
}

/// workers-rs のエラーを [`BackendError`] にする。どの操作で失敗したかを添える。
fn backend_error(action: &str, error: &worker::Error) -> BackendError {
    BackendError(format!("{action}: {error}"))
}

/// 整数（時刻や人数）を JavaScript の数値にする。
/// JavaScript の数値は 2^53 までの整数を正確に表せるので、ミリ秒の時刻や人数なら誤差は出ない。
#[allow(clippy::cast_precision_loss)]
fn int_to_js(value: i64) -> JsValue {
    JsValue::from_f64(value as f64)
}

/// `SELECT COUNT(*) AS total` の行。
#[derive(serde::Deserialize)]
struct Total {
    total: i64,
}

/// レート制限の binding `binding` で、`key` の回数が制限の中かを確かめる。
fn limit(
    env: &SendWrapper<Env>,
    binding: &'static str,
    key: &str,
) -> impl Future<Output = BackendResult<bool>> + Send {
    let env = env.clone();
    let key = key.to_owned();
    SendFuture::new(async move {
        let limiter = env
            .rate_limiter(binding)
            .map_err(|e| backend_error("レート制限の binding を取れない", &e))?;
        let outcome = limiter
            .limit(key)
            .await
            .map_err(|e| backend_error("レート制限を確かめられない", &e))?;
        Ok(outcome.success)
    })
}

/// D1 のデータベースを取る。
fn database(env: &Env) -> BackendResult<D1Database> {
    env.d1(DB_BINDING)
        .map_err(|e| backend_error("D1 の binding を取れない", &e))
}

/// SQL に値を当てはめて実行する。`action` はエラーのときにログに出す説明。
async fn run(
    db: &D1Database,
    sql: &str,
    values: &[JsValue],
    action: &str,
) -> BackendResult<D1Result> {
    db.prepare(sql)
        .bind(values)
        .map_err(|e| backend_error(action, &e))?
        .run()
        .await
        .map_err(|e| backend_error(action, &e))
}

/// 更新・削除で変えた行の数を返す。
/// 取れないときに 0 とみなすと「無い」と取り違えるので、失敗にする。
fn changed_rows(result: &D1Result, action: &str) -> BackendResult<usize> {
    result
        .meta()
        .map_err(|e| backend_error(action, &e))?
        .and_then(|meta| meta.changes)
        .ok_or_else(|| BackendError(format!("{action}: 変えた行の数が結果に無い")))
}

impl Backend for CloudflareBackend {
    fn random_128(&self) -> [u8; 16] {
        let mut bytes = [0; 16];
        get_random_values(&mut bytes);
        bytes
    }

    fn now_millis(&self) -> i64 {
        // as_millis は u64 を返す。i64 に収まらないのは約 2 億 9 千万年後なので、収まらなければ最大値にする。
        i64::try_from(worker::Date::now().as_millis()).unwrap_or(i64::MAX)
    }

    fn demo_workspace_id(&self) -> Option<WorkspaceId> {
        // 形の正しくない値は、設定されていないものとして扱う。
        self.env
            .secret(DEMO_WORKSPACE_SECRET)
            .ok()
            .and_then(|value| WorkspaceId::parse(&value.to_string()))
    }

    fn allow_workspace_creation(
        &self,
        client_key: &str,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        limit(&self.env, WORKSPACE_CREATION_LIMITER, client_key)
    }

    fn allow_write(&self, client_key: &str) -> impl Future<Output = BackendResult<bool>> + Send {
        limit(&self.env, WRITE_LIMITER, client_key)
    }

    fn insert_workspace(
        &self,
        id: &WorkspaceId,
        now: i64,
        limits: &WorkspaceLimits,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        let env = self.env.clone();
        let values = SendWrapper::new([
            id.to_string().into(),
            int_to_js(now),
            int_to_js(now),
            int_to_js(limits.max_total),
            int_to_js(limits.since),
            int_to_js(limits.max_created_since),
        ]);
        SendFuture::new(async move {
            let action = "ワークスペースを保存できない";
            let db = database(&env)?;
            // 上限の判定と保存を 1 つの文にして、同時に作られても上限を超えないようにする。
            // FROM の (VALUES (0)) は、1 行だけの表として使う（標準の SQL の書き方）。
            // 全体を数えるので、workspace_id では絞り込まない（AGENTS.md のワークスペースを作るクエリ）。
            let result = run(
                &db,
                "INSERT INTO workspaces (id, created_at, last_used_at) \
                 SELECT ?, ?, ? FROM (VALUES (0)) AS one \
                 WHERE (SELECT COUNT(*) FROM workspaces) < ? \
                 AND (SELECT COUNT(*) FROM workspaces WHERE created_at >= ?) < ?",
                &values.0,
                action,
            )
            .await?;
            Ok(changed_rows(&result, action)? > 0)
        })
    }

    fn workspace_last_used_at(
        &self,
        id: &WorkspaceId,
    ) -> impl Future<Output = BackendResult<Option<i64>>> + Send {
        let env = self.env.clone();
        let id = id.to_string();
        SendFuture::new(async move {
            let action = "ワークスペースを読めない";
            let db = database(&env)?;
            db.prepare("SELECT last_used_at FROM workspaces WHERE id = ?")
                .bind(&[id.into()])
                .map_err(|e| backend_error(action, &e))?
                .first::<i64>(Some("last_used_at"))
                .await
                .map_err(|e| backend_error(action, &e))
        })
    }

    fn set_workspace_last_used_at(
        &self,
        id: &WorkspaceId,
        now: i64,
    ) -> impl Future<Output = BackendResult<()>> + Send {
        let env = self.env.clone();
        let id = id.to_string();
        SendFuture::new(async move {
            let db = database(&env)?;
            run(
                &db,
                "UPDATE workspaces SET last_used_at = ? WHERE id = ?",
                &[int_to_js(now), id.into()],
                "ワークスペースの最終利用日時を書き換えられない",
            )
            .await?;
            Ok(())
        })
    }

    fn list_recipes(
        &self,
        workspace: &WorkspaceId,
        query: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> impl Future<Output = BackendResult<RecipePage>> + Send {
        let env = self.env.clone();
        let workspace = workspace.to_string();
        let query = query.map(str::to_owned);
        SendFuture::new(async move {
            let action = "レシピの一覧を読めない";
            let db = database(&env)?;
            // 部分一致には LIKE ではなく instr を使う（AGENTS.md の SQL の例外）。
            // D1 は LIKE のパターンを 50 バイトまでに制限していて、日本語の検索の文字列が 16 文字ほどで通らなくなるため。
            let (condition, values): (&str, Vec<JsValue>) = match query {
                Some(query) => (
                    "workspace_id = ? AND instr(search_name, ?) > 0",
                    vec![workspace.into(), query.into()],
                ),
                None => ("workspace_id = ?", vec![workspace.into()]),
            };
            let mut page_values = values.clone();
            page_values.extend([int_to_js(limit), int_to_js(offset)]);
            // 並べ方は索引 (workspace_id, created_at, id) と同じ向きにして、並べ替えずに読む。
            // 同じ日時に作ったレシピの順番が毎回変わらないよう、ID でも並べる。
            // LIMIT と OFFSET は ISO の標準ではないが、PostgreSQL でも同じ意味で動く（AGENTS.md）。
            let page = db
                .prepare(format!(
                    "SELECT {RECIPE_COLUMNS} FROM recipes WHERE {condition} \
                     ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
                ))
                .bind(&page_values)
                .map_err(|e| backend_error(action, &e))?;
            let count = db
                .prepare(format!(
                    "SELECT COUNT(*) AS total FROM recipes WHERE {condition}"
                ))
                .bind(&values)
                .map_err(|e| backend_error(action, &e))?;
            // 1 つの batch にして、ページと総数を同じ時点のデータから読む。
            let results = db
                .batch(vec![page, count])
                .await
                .map_err(|e| backend_error(action, &e))?;
            let [page, count] = results.as_slice() else {
                return Err(BackendError(format!("{action}: batch の結果の数が違う")));
            };
            let recipes = page
                .results::<Recipe>()
                .map_err(|e| backend_error(action, &e))?;
            let total = count
                .results::<Total>()
                .map_err(|e| backend_error(action, &e))?
                .first()
                .map(|row| row.total)
                .ok_or_else(|| BackendError(format!("{action}: 総数が結果に無い")))?;
            Ok(RecipePage { recipes, total })
        })
    }

    fn insert_recipe(
        &self,
        workspace: &WorkspaceId,
        recipe: &Recipe,
        max_recipes: i64,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        let env = self.env.clone();
        let values = [
            workspace.to_string().into(),
            recipe.id.clone().into(),
            recipe.name.clone().into(),
            search_name(&recipe.name).into(),
            int_to_js(recipe.servings),
            recipe.ingredients.clone().into(),
            recipe.steps.clone().into(),
            int_to_js(recipe.created_at),
            int_to_js(recipe.updated_at),
            workspace.to_string().into(),
            int_to_js(max_recipes),
        ];
        // JsValue は Send でないので、値ごと SendWrapper で包んで async の中へ持ち込む。
        let values = SendWrapper::new(values);
        SendFuture::new(async move {
            let action = "レシピを保存できない";
            let db = database(&env)?;
            // 上限の判定と保存を 1 つの文にして、同時に登録されても上限を超えないようにする。
            let result = run(
                &db,
                "INSERT INTO recipes \
                 (workspace_id, id, name, search_name, servings, ingredients, steps, created_at, updated_at) \
                 SELECT ?, ?, ?, ?, ?, ?, ?, ?, ? FROM (VALUES (0)) AS one \
                 WHERE (SELECT COUNT(*) FROM recipes WHERE workspace_id = ?) < ?",
                &values.0,
                action,
            )
            .await?;
            Ok(changed_rows(&result, action)? > 0)
        })
    }

    fn get_recipe(
        &self,
        workspace: &WorkspaceId,
        id: &RecipeId,
    ) -> impl Future<Output = BackendResult<Option<Recipe>>> + Send {
        let env = self.env.clone();
        let workspace = workspace.to_string();
        let id = id.to_string();
        SendFuture::new(async move {
            let action = "レシピを読めない";
            let db = database(&env)?;
            db.prepare(format!(
                "SELECT {RECIPE_COLUMNS} FROM recipes WHERE workspace_id = ? AND id = ?"
            ))
            .bind(&[workspace.into(), id.into()])
            .map_err(|e| backend_error(action, &e))?
            .first::<Recipe>(None)
            .await
            .map_err(|e| backend_error(action, &e))
        })
    }

    fn update_recipe(
        &self,
        workspace: &WorkspaceId,
        id: &RecipeId,
        fields: &RecipeFields,
        now: i64,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        let env = self.env.clone();
        let values = SendWrapper::new([
            fields.name.clone().into(),
            search_name(&fields.name).into(),
            int_to_js(fields.servings),
            fields.ingredients.clone().into(),
            fields.steps.clone().into(),
            int_to_js(now),
            workspace.to_string().into(),
            id.to_string().into(),
        ]);
        SendFuture::new(async move {
            let action = "レシピを書き換えられない";
            let db = database(&env)?;
            let result = run(
                &db,
                "UPDATE recipes SET name = ?, search_name = ?, servings = ?, ingredients = ?, steps = ?, updated_at = ? \
                 WHERE workspace_id = ? AND id = ?",
                &values.0,
                action,
            )
            .await?;
            Ok(changed_rows(&result, action)? > 0)
        })
    }

    fn delete_recipe(
        &self,
        workspace: &WorkspaceId,
        id: &RecipeId,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        let env = self.env.clone();
        let workspace = workspace.to_string();
        let id = id.to_string();
        SendFuture::new(async move {
            let action = "レシピを消せない";
            let db = database(&env)?;
            let result = run(
                &db,
                "DELETE FROM recipes WHERE workspace_id = ? AND id = ?",
                &[workspace.into(), id.into()],
                action,
            )
            .await?;
            Ok(changed_rows(&result, action)? > 0)
        })
    }
}
