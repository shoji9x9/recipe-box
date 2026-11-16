//! Cloudflare の機能（D1・R2・Rate Limiting・Web Crypto）を使う [`Backend`] の実装。
//!
//! JavaScript の値は `Send` でないが、Workers は 1 つのスレッドで動くので、
//! workers-rs の `SendWrapper`・`SendFuture` で包んで axum に渡しても問題が無い。
//!
//! SQL は標準の範囲で書き、レシピを扱うクエリは必ず `workspace_id` で絞り込む（AGENTS.md）。

use std::{future::Future, rc::Rc};

use wasm_bindgen::prelude::wasm_bindgen;
use worker::{
    Bucket, Context, D1Database, D1Result, Data, Env, HttpMetadata,
    send::{SendFuture, SendWrapper},
    wasm_bindgen::JsValue,
};

use crate::{
    backend::{
        Backend, BackendError, BackendResult, ListedRecipe, PendingPhoto, PhotoInsert, PhotoKey,
        PhotoLimits, RecipePage, WorkspaceLimits, WorkspaceStatus,
    },
    photo::{Photo, PhotoBody, PhotoId},
    recipe::{Recipe, RecipeFields, RecipeId, search_name},
    workspace::WorkspaceId,
};

/// `wrangler.toml` の D1 の binding の名前。
const DB_BINDING: &str = "DB";
/// `wrangler.toml` の R2 の binding の名前（写真の置き場）。
const PHOTOS_BINDING: &str = "PHOTOS";
/// 共有のデモのワークスペースの ID を置く secret の名前。
const DEMO_WORKSPACE_SECRET: &str = "DEMO_WORKSPACE_ID";
/// `wrangler.toml` のレート制限の binding の名前（ワークスペースを作る回数）。
const WORKSPACE_CREATION_LIMITER: &str = "WORKSPACE_CREATION_LIMITER";
/// `wrangler.toml` のレート制限の binding の名前（書き込む API の回数）。
const WRITE_LIMITER: &str = "WRITE_LIMITER";

/// ワークスペースのすべての写真を読む SQL（[`PhotoKey`] の形）。
const WORKSPACE_PHOTOS_SQL: &str = "SELECT recipe_id, id FROM photos WHERE workspace_id = ?";

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
// guide:start cloudflare-backend
pub struct CloudflareBackend {
    env: SendWrapper<Env>,
    /// リクエストの context（`waitUntil` に使う）。定期実行には無い。
    ctx: Option<SendWrapper<Rc<Context>>>,
}
// guide:end cloudflare-backend

impl CloudflareBackend {
    /// 定期実行に使う。リクエストが無いので、[`Backend::run_to_completion`] は `task` をそのまま待つ。
    pub fn new(env: Env) -> Self {
        Self {
            env: SendWrapper::new(env),
            ctx: None,
        }
    }

    /// リクエストを処理するときに使う。
    pub fn with_context(env: Env, ctx: Context) -> Self {
        Self {
            env: SendWrapper::new(env),
            ctx: Some(SendWrapper::new(Rc::new(ctx))),
        }
    }

    /// 写真の行の、書き込みか削除が終わっていない印を付ける（`since` が日時）か外す（`None`）。
    /// 付けるときは印の無い行に、外すときは印の付いた行にだけ効く。効いた行が無ければ `false` を返す。
    fn set_photo_pending(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
        since: Option<i64>,
        action: &'static str,
    ) -> impl Future<Output = BackendResult<bool>> + Send + use<> {
        let env = self.env.clone();
        let values = [
            workspace.to_string(),
            recipe.to_string(),
            id.as_str().to_owned(),
        ];
        let since = since.map(int_to_js).map(SendWrapper::new);
        SendFuture::new(async move {
            let db = database(&env)?;
            let ids = values.map(JsValue::from);
            let result = match since {
                Some(since) => {
                    let mut binds = vec![since.0];
                    binds.extend(ids);
                    run(
                        &db,
                        "UPDATE photos SET pending_since = ? \
                         WHERE workspace_id = ? AND recipe_id = ? AND id = ? AND pending_since IS NULL",
                        &binds,
                        action,
                    )
                    .await?
                }
                None => {
                    run(
                        &db,
                        "UPDATE photos SET pending_since = NULL \
                         WHERE workspace_id = ? AND recipe_id = ? AND id = ? AND pending_since IS NOT NULL",
                        &ids,
                        action,
                    )
                    .await?
                }
            };
            Ok(changed_rows(&result, action)? > 0)
        })
    }
}

/// workers-rs のエラーを [`BackendError`] にする。どの操作で失敗したかを添える。
/// エラーの文に入りうる ID（R2 のキーなど）は、ここで伏せる。ログに出すときにも、もう一度伏せる（[`crate::error::log_error`]）。
fn backend_error(action: &str, error: &worker::Error) -> BackendError {
    BackendError(format!(
        "{action}: {}",
        crate::id::redact(&error.to_string())
    ))
}

/// 整数（時刻や人数）を JavaScript の数値にする。
/// JavaScript の数値は 2^53 までの整数を正確に表せるので、ミリ秒の時刻や人数なら誤差は出ない。
#[allow(clippy::cast_precision_loss)]
fn int_to_js(value: i64) -> JsValue {
    JsValue::from_f64(value as f64)
}

/// 写真を読むときの列。`Photo` の項目と同じ名前・順番にする。
const PHOTO_COLUMNS: &str = "id, content_type, size, created_at";

/// 一覧のクエリの行。レシピの列と、先頭の写真の ID。
#[derive(serde::Deserialize)]
struct ListedRow {
    #[serde(flatten)]
    recipe: Recipe,
    /// 先頭の写真を求めないクエリ（API の一覧）では、列が無いので `None` になる。
    #[serde(default)]
    first_photo_id: Option<String>,
}

/// 一覧のクエリで、レシピごとの先頭の写真（追加した日時の古い順、同じ日時なら ID の小さい順の最初）の ID を求める列。
/// 写真の一覧（`list_photos`）の `ORDER BY created_at, id` の最初と同じになる。
/// 標準の SQL で書くため、`ORDER BY ... LIMIT 1` ではなく、最も古い日時の写真のうち ID の最も小さいものを `MIN` で選ぶ。
/// どちらの副問い合わせも、主キー（`workspace_id`・`recipe_id`・`id`）の先頭の 2 列で写真を絞る（1 レシピ 5 枚まで）。
/// 書き込みか削除が終わっていない印の付いた写真は、写真の一覧と同じく除く。
const FIRST_PHOTO_COLUMN: &str = "(SELECT MIN(p.id) FROM photos p \
     WHERE p.workspace_id = recipes.workspace_id AND p.recipe_id = recipes.id AND p.pending_since IS NULL \
     AND p.created_at = (SELECT MIN(q.created_at) FROM photos q \
     WHERE q.workspace_id = recipes.workspace_id AND q.recipe_id = recipes.id AND q.pending_since IS NULL)) \
     AS first_photo_id";

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

/// R2 のバケットを取る。
fn bucket(env: &Env) -> BackendResult<Bucket> {
    env.bucket(PHOTOS_BINDING)
        .map_err(|e| backend_error("R2 の binding を取れない", &e))
}

/// `SELECT id` の行。
#[derive(serde::Deserialize)]
struct IdRow {
    id: String,
}

/// `SELECT last_used_at, pinned` の行。
#[derive(serde::Deserialize)]
struct StatusRow {
    last_used_at: i64,
    pinned: i64,
}

/// 写真を保存できなかったときに、どの上限に当たったかを調べるための行。
#[derive(serde::Deserialize)]
struct PhotoUsage {
    /// そのワークスペースに、そのレシピが在れば 1。
    recipes: i64,
    /// レシピの写真の数。
    recipe_photos: i64,
    /// ワークスペースの写真の大きさの合計。
    workspace_bytes: i64,
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
            // guide:start insert-workspace
            // 上限の判定と保存を 1 つの文にして、同時に作られても上限を超えないようにする。
            // FROM の (VALUES (0)) は、1 行だけの表として使う（標準の SQL の書き方）。
            // 全体を数えるので、workspace_id では絞り込まない（AGENTS.md のワークスペースを作るクエリ）。
            // 1 日に作った数は、消しても残る workspace_creations で数える（作ると消すを繰り返して超えさせない）。
            let insert = db
                .prepare(
                    "INSERT INTO workspaces (id, created_at, last_used_at) \
                     SELECT ?, ?, ? FROM (VALUES (0)) AS one \
                     WHERE (SELECT COUNT(*) FROM workspaces) < ? \
                     AND (SELECT COUNT(*) FROM workspace_creations WHERE created_at >= ?) < ?",
                )
                .bind(&values.0)
                .map_err(|e| backend_error(action, &e))?;
            // 作れたときだけ、作った記録を入れる（上の文で行が入ったかを、同じ ID の行が在るかで見る）。
            let record = db
                .prepare(
                    "INSERT INTO workspace_creations (created_at) \
                     SELECT created_at FROM workspaces WHERE id = ?",
                )
                .bind(&[values.0[0].clone()])
                .map_err(|e| backend_error(action, &e))?;
            // batch は 1 つのトランザクションとして、順に実行される。
            let results = db
                .batch(vec![insert, record])
                .await
                .map_err(|e| backend_error(action, &e))?;
            // guide:end insert-workspace
            let Some(inserted) = results.first() else {
                return Err(BackendError(format!("{action}: batch の結果が無い")));
            };
            Ok(changed_rows(inserted, action)? > 0)
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

    fn workspace_status(
        &self,
        id: &WorkspaceId,
    ) -> impl Future<Output = BackendResult<Option<WorkspaceStatus>>> + Send {
        let env = self.env.clone();
        let id = id.to_string();
        SendFuture::new(async move {
            let action = "ワークスペースの状態を読めない";
            let db = database(&env)?;
            // D1（SQLite）は BOOLEAN を 0 と 1 の整数で返すので、整数で受け取る。
            let row = db
                .prepare("SELECT last_used_at, pinned FROM workspaces WHERE id = ?")
                .bind(&[id.into()])
                .map_err(|e| backend_error(action, &e))?
                .first::<StatusRow>(None)
                .await
                .map_err(|e| backend_error(action, &e))?;
            Ok(row.map(|row| WorkspaceStatus {
                last_used_at: row.last_used_at,
                pinned: row.pinned != 0,
            }))
        })
    }

    fn list_unused_workspaces(
        &self,
        before: i64,
    ) -> impl Future<Output = BackendResult<Vec<String>>> + Send {
        let env = self.env.clone();
        let before = SendWrapper::new(int_to_js(before));
        SendFuture::new(async move {
            let action = "使われていないワークスペースを読めない";
            let db = database(&env)?;
            // 自動削除のクエリなので、全体を対象にする（AGENTS.md）。ワークスペースは多くても 100 個なので、索引は使わない。
            let rows = db
                .prepare("SELECT id FROM workspaces WHERE pinned = FALSE AND last_used_at < ?")
                .bind(&[before.0])
                .map_err(|e| backend_error(action, &e))?
                .all()
                .await
                .map_err(|e| backend_error(action, &e))?
                .results::<IdRow>()
                .map_err(|e| backend_error(action, &e))?;
            Ok(rows.into_iter().map(|row| row.id).collect())
        })
    }

    fn delete_old_records(&self, before: i64) -> impl Future<Output = BackendResult<()>> + Send {
        let env = self.env.clone();
        let before = SendWrapper::new(int_to_js(before));
        SendFuture::new(async move {
            let action = "古い記録を消せない";
            let db = database(&env)?;
            // 1 日の数を数えるためだけの記録なので、全体を対象にする（AGENTS.md の、全体の上限を数えるクエリ）。
            let mut statements = Vec::new();
            for sql in [
                "DELETE FROM photo_uploads WHERE created_at < ?",
                "DELETE FROM workspace_creations WHERE created_at < ?",
            ] {
                statements.push(
                    db.prepare(sql)
                        .bind(std::slice::from_ref(&before.0))
                        .map_err(|e| backend_error(action, &e))?,
                );
            }
            db.batch(statements)
                .await
                .map_err(|e| backend_error(action, &e))?;
            Ok(())
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
        with_first_photo: bool,
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
            // 先頭の写真は、画面の一覧のときだけ求める（API の一覧では、返さない値のために photos を読まない）。
            let first_photo = if with_first_photo {
                format!(", {FIRST_PHOTO_COLUMN}")
            } else {
                String::new()
            };
            let page = db
                .prepare(format!(
                    "SELECT {RECIPE_COLUMNS}{first_photo} FROM recipes WHERE {condition} \
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
                .results::<ListedRow>()
                .map_err(|e| backend_error(action, &e))?
                .into_iter()
                .map(|row| ListedRecipe {
                    recipe: row.recipe,
                    first_photo_id: row.first_photo_id,
                })
                .collect();
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
    ) -> impl Future<Output = BackendResult<Option<Vec<String>>>> + Send {
        let env = self.env.clone();
        let workspace = workspace.to_string();
        let id = id.to_string();
        SendFuture::new(async move {
            let action = "レシピを消せない";
            let db = database(&env)?;
            let values = [JsValue::from(workspace.as_str()), id.as_str().into()];
            let photos = db
                .prepare("SELECT id FROM photos WHERE workspace_id = ? AND recipe_id = ?")
                .bind(&values)
                .map_err(|e| backend_error(action, &e))?;
            // 写真の行は ON DELETE CASCADE で一緒に消える。
            let recipe = db
                .prepare("DELETE FROM recipes WHERE workspace_id = ? AND id = ?")
                .bind(&values)
                .map_err(|e| backend_error(action, &e))?;
            // batch は 1 つのトランザクションなので、読んだ写真の ID と、消した写真の行が一致する。
            let results = db
                .batch(vec![photos, recipe])
                .await
                .map_err(|e| backend_error(action, &e))?;
            let [photos, recipe] = results.as_slice() else {
                return Err(BackendError(format!("{action}: batch の結果の数が違う")));
            };
            if changed_rows(recipe, action)? == 0 {
                return Ok(None);
            }
            let ids = photos
                .results::<IdRow>()
                .map_err(|e| backend_error(action, &e))?
                .into_iter()
                .map(|row| row.id)
                .collect();
            Ok(Some(ids))
        })
    }

    fn list_workspace_photos(
        &self,
        workspace: &WorkspaceId,
    ) -> impl Future<Output = BackendResult<Vec<PhotoKey>>> + Send {
        let env = self.env.clone();
        let workspace = workspace.to_string();
        SendFuture::new(async move {
            let action = "ワークスペースの写真を読めない";
            let db = database(&env)?;
            db.prepare(WORKSPACE_PHOTOS_SQL)
                .bind(&[workspace.into()])
                .map_err(|e| backend_error(action, &e))?
                .all()
                .await
                .map_err(|e| backend_error(action, &e))?
                .results::<PhotoKey>()
                .map_err(|e| backend_error(action, &e))
        })
    }

    fn delete_workspace(
        &self,
        workspace: &WorkspaceId,
        last_used_before: Option<i64>,
    ) -> impl Future<Output = BackendResult<Option<Vec<PhotoKey>>>> + Send {
        let env = self.env.clone();
        let workspace = workspace.to_string();
        let last_used_before = last_used_before.map(int_to_js).map(SendWrapper::new);
        SendFuture::new(async move {
            let action = "ワークスペースを消せない";
            let db = database(&env)?;
            let photos = db
                .prepare(WORKSPACE_PHOTOS_SQL)
                .bind(&[workspace.as_str().into()])
                .map_err(|e| backend_error(action, &e))?;
            // レシピと写真の行は ON DELETE CASCADE で一緒に消える。印の付いたワークスペースは消さない。
            let deleted = match last_used_before {
                None => db
                    .prepare("DELETE FROM workspaces WHERE id = ? AND pinned = FALSE")
                    .bind(&[workspace.as_str().into()]),
                Some(before) => db
                    .prepare(
                        "DELETE FROM workspaces WHERE id = ? AND pinned = FALSE AND last_used_at < ?",
                    )
                    .bind(&[workspace.as_str().into(), before.0]),
            }
            .map_err(|e| backend_error(action, &e))?;
            // batch は 1 つのトランザクションなので、読んだ写真と、消した写真の行が一致する。
            let results = db
                .batch(vec![photos, deleted])
                .await
                .map_err(|e| backend_error(action, &e))?;
            let [photos, deleted] = results.as_slice() else {
                return Err(BackendError(format!("{action}: batch の結果の数が違う")));
            };
            if changed_rows(deleted, action)? == 0 {
                return Ok(None);
            }
            photos
                .results::<PhotoKey>()
                .map(Some)
                .map_err(|e| backend_error(action, &e))
        })
    }

    fn insert_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        photo: &Photo,
        limits: &PhotoLimits,
    ) -> impl Future<Output = BackendResult<PhotoInsert>> + Send {
        let env = self.env.clone();
        let workspace = workspace.to_string();
        let recipe = recipe.to_string();
        let photo = photo.clone();
        let limits = *limits;
        SendFuture::new(async move {
            let action = "写真を保存できない";
            let db = database(&env)?;
            let w = || JsValue::from(workspace.as_str());
            let r = || JsValue::from(recipe.as_str());
            let size = || int_to_js(photo.size);
            // 上限の判定と保存を 1 つの文にして、同時に追加されても上限を超えないようにする。
            // 行は、R2 への書き込みが終わっていない印（日時）を付けて入れる。上限は、印の付いた行も数える。
            // 1 日の回数は、全体を数えるので workspace_id では絞り込まない（AGENTS.md の全体の上限のクエリ）。
            // アプリ全体の写真の合計は数えない（photos の全体を読むことになるため）。ワークスペースの数の上限と、
            // ワークスペースごとの合計の上限で守る（photo::MAX_PHOTO_BYTES_TOTAL）。
            let insert = db
                .prepare(
                    "INSERT INTO photos \
                     (workspace_id, recipe_id, id, content_type, size, created_at, pending_since) \
                     SELECT ?, ?, ?, ?, ?, ?, ? FROM (VALUES (0)) AS one \
                     WHERE EXISTS (SELECT 1 FROM recipes WHERE workspace_id = ? AND id = ?) \
                     AND (SELECT COUNT(*) FROM photos WHERE workspace_id = ? AND recipe_id = ?) < ? \
                     AND (SELECT COUNT(*) FROM photo_uploads WHERE created_at >= ?) < ? \
                     AND (SELECT COALESCE(SUM(size), 0) FROM photos WHERE workspace_id = ?) + ? <= ?",
                )
                .bind(&[
                    w(),
                    r(),
                    photo.id.as_str().into(),
                    photo.content_type.as_str().into(),
                    size(),
                    int_to_js(photo.created_at),
                    int_to_js(photo.created_at),
                    w(),
                    r(),
                    w(),
                    r(),
                    int_to_js(limits.max_per_recipe),
                    int_to_js(limits.since),
                    int_to_js(limits.max_uploads_since),
                    w(),
                    size(),
                    int_to_js(limits.max_bytes_per_workspace),
                ])
                .map_err(|e| backend_error(action, &e))?;
            // 写真の行が入ったときだけ、追加の記録を入れる。
            let record = db
                .prepare(
                    "INSERT INTO photo_uploads (id, created_at) \
                     SELECT id, created_at FROM photos WHERE workspace_id = ? AND recipe_id = ? AND id = ?",
                )
                .bind(&[w(), r(), photo.id.as_str().into()])
                .map_err(|e| backend_error(action, &e))?;
            // batch は 1 つのトランザクションとして、順に実行される。
            let results = db
                .batch(vec![insert, record])
                .await
                .map_err(|e| backend_error(action, &e))?;
            let Some(inserted) = results.first() else {
                return Err(BackendError(format!("{action}: batch の結果が無い")));
            };
            if changed_rows(inserted, action)? > 0 {
                return Ok(PhotoInsert::Inserted);
            }
            // 保存しなかったときだけ、どの上限に当たったかを調べる。
            let action = "写真の上限を調べられない";
            let usage = db
                .prepare(
                    "SELECT \
                     (SELECT COUNT(*) FROM recipes WHERE workspace_id = ? AND id = ?) AS recipes, \
                     (SELECT COUNT(*) FROM photos WHERE workspace_id = ? AND recipe_id = ?) AS recipe_photos, \
                     (SELECT COALESCE(SUM(size), 0) FROM photos WHERE workspace_id = ?) AS workspace_bytes \
                     FROM (VALUES (0)) AS one",
                )
                .bind(&[w(), r(), w(), r(), w()])
                .map_err(|e| backend_error(action, &e))?
                .first::<PhotoUsage>(None)
                .await
                .map_err(|e| backend_error(action, &e))?
                .ok_or_else(|| BackendError(format!("{action}: 結果が無い")))?;
            Ok(if usage.recipes == 0 {
                PhotoInsert::RecipeNotFound
            } else if usage.recipe_photos >= limits.max_per_recipe {
                PhotoInsert::RecipeFull
            } else if usage.workspace_bytes + photo.size > limits.max_bytes_per_workspace {
                PhotoInsert::WorkspaceFull
            } else {
                PhotoInsert::CapacityReached
            })
        })
    }

    fn finish_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        self.set_photo_pending(workspace, recipe, id, None, "写真の印を外せない")
    }

    fn hide_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
        now: i64,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        self.set_photo_pending(workspace, recipe, id, Some(now), "写真を隠せない")
    }

    fn list_pending_photos(
        &self,
        before: i64,
    ) -> impl Future<Output = BackendResult<Vec<PendingPhoto>>> + Send {
        let env = self.env.clone();
        let before = SendWrapper::new(int_to_js(before));
        SendFuture::new(async move {
            let action = "片付ける写真を選べない";
            let db = database(&env)?;
            // 自動削除のクエリなので、ワークスペースでは絞り込まない（AGENTS.md の「SQL」）。
            db.prepare("SELECT workspace_id, recipe_id, id FROM photos WHERE pending_since < ?")
                .bind(&[before.0])
                .map_err(|e| backend_error(action, &e))?
                .all()
                .await
                .map_err(|e| backend_error(action, &e))?
                .results::<PendingPhoto>()
                .map_err(|e| backend_error(action, &e))
        })
    }

    fn list_recipe_photo_ids(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
    ) -> impl Future<Output = BackendResult<Vec<String>>> + Send {
        let env = self.env.clone();
        let values = [workspace.to_string(), recipe.to_string()];
        SendFuture::new(async move {
            let action = "レシピの写真を読めない";
            let db = database(&env)?;
            Ok(db
                .prepare("SELECT id FROM photos WHERE workspace_id = ? AND recipe_id = ?")
                .bind(&values.map(JsValue::from))
                .map_err(|e| backend_error(action, &e))?
                .all()
                .await
                .map_err(|e| backend_error(action, &e))?
                .results::<IdRow>()
                .map_err(|e| backend_error(action, &e))?
                .into_iter()
                .map(|row| row.id)
                .collect())
        })
    }

    fn list_photos(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
    ) -> impl Future<Output = BackendResult<Vec<Photo>>> + Send {
        let env = self.env.clone();
        let workspace = workspace.to_string();
        let recipe = recipe.to_string();
        SendFuture::new(async move {
            let action = "写真の一覧を読めない";
            let db = database(&env)?;
            db.prepare(format!(
                "SELECT {PHOTO_COLUMNS} FROM photos WHERE workspace_id = ? AND recipe_id = ? \
                 AND pending_since IS NULL ORDER BY created_at, id"
            ))
            .bind(&[workspace.into(), recipe.into()])
            .map_err(|e| backend_error(action, &e))?
            .all()
            .await
            .map_err(|e| backend_error(action, &e))?
            .results::<Photo>()
            .map_err(|e| backend_error(action, &e))
        })
    }

    fn get_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
    ) -> impl Future<Output = BackendResult<Option<Photo>>> + Send {
        let env = self.env.clone();
        let values = [
            workspace.to_string(),
            recipe.to_string(),
            id.as_str().to_owned(),
        ];
        SendFuture::new(async move {
            let action = "写真を読めない";
            let db = database(&env)?;
            db.prepare(format!(
                "SELECT {PHOTO_COLUMNS} FROM photos WHERE workspace_id = ? AND recipe_id = ? AND id = ? \
                 AND pending_since IS NULL"
            ))
            .bind(&values.map(JsValue::from))
            .map_err(|e| backend_error(action, &e))?
            .first::<Photo>(None)
            .await
            .map_err(|e| backend_error(action, &e))
        })
    }

    fn delete_photo(
        &self,
        workspace: &WorkspaceId,
        recipe: &RecipeId,
        id: &PhotoId,
    ) -> impl Future<Output = BackendResult<bool>> + Send {
        let env = self.env.clone();
        let values = [
            workspace.to_string(),
            recipe.to_string(),
            id.as_str().to_owned(),
        ];
        SendFuture::new(async move {
            let action = "写真を消せない";
            let db = database(&env)?;
            let result = run(
                &db,
                "DELETE FROM photos WHERE workspace_id = ? AND recipe_id = ? AND id = ?",
                &values.map(JsValue::from),
                action,
            )
            .await?;
            Ok(changed_rows(&result, action)? > 0)
        })
    }

    fn delete_photos(
        &self,
        photos: &[(WorkspaceId, RecipeId, PhotoId)],
        marked_before: i64,
    ) -> impl Future<Output = BackendResult<()>> + Send {
        let env = self.env.clone();
        let marked_before = SendWrapper::new(int_to_js(marked_before));
        let rows: Vec<[String; 3]> = photos
            .iter()
            .map(|(workspace, recipe, id)| {
                [
                    workspace.to_string(),
                    recipe.to_string(),
                    id.as_str().to_owned(),
                ]
            })
            .collect();
        SendFuture::new(async move {
            let action = "写真の行をまとめて消せない";
            if rows.is_empty() {
                return Ok(());
            }
            let db = database(&env)?;
            let statements = rows
                .into_iter()
                .map(|values| {
                    let [w, r, id] = values.map(JsValue::from);
                    // 選んだときの条件（印を付けた日時）を、消すときにも確かめ直す。
                    db.prepare(
                        "DELETE FROM photos WHERE workspace_id = ? AND recipe_id = ? AND id = ? \
                         AND pending_since < ?",
                    )
                    .bind(&[w, r, id, marked_before.0.clone()])
                    .map_err(|e| backend_error(action, &e))
                })
                .collect::<BackendResult<Vec<_>>>()?;
            // batch は 1 回の呼び出しで、1 つのトランザクションとして実行される。
            db.batch(statements)
                .await
                .map_err(|e| backend_error(action, &e))?;
            Ok(())
        })
    }

    fn run_to_completion<T: Send + 'static>(
        &self,
        task: impl Future<Output = T> + Send + 'static,
    ) -> impl Future<Output = Option<T>> + Send {
        // 呼んだ時点で waitUntil に登録する（返した Future を待つ前に接続が切れても、task は動く）。
        // 定期実行（context が無い）では、task をそのまま待つ。
        let (receiver, inline) = match &self.ctx {
            Some(ctx) => {
                let (sender, receiver) = futures_channel::oneshot::channel();
                ctx.wait_until(async move {
                    // 待っていた側が先に取り消されていれば（接続が切れたとき）、結果は捨てる。
                    let _ = sender.send(task.await);
                });
                (Some(receiver), None)
            }
            None => (None, Some(task)),
        };
        async move {
            match (receiver, inline) {
                (Some(receiver), _) => receiver.await.ok(),
                (None, Some(task)) => Some(task.await),
                (None, None) => None,
            }
        }
    }

    fn put_object(
        &self,
        key: &str,
        body: PhotoBody,
        content_type: &str,
    ) -> impl Future<Output = BackendResult<()>> + Send {
        let env = self.env.clone();
        let key = key.to_owned();
        let content_type = content_type.to_owned();
        SendFuture::new(async move {
            let action = "写真を R2 に書き込めない";
            let data = match body {
                PhotoBody::Bytes(bytes) => Data::Bytes(bytes),
                // 長さの分かるストリーム（FixedLengthStream の読む側）なので、R2 がそのまま読める。
                PhotoBody::Stream(stream) => Data::ReadableStream(stream.readable.0),
            };
            bucket(&env)?
                .put(key, data)
                .http_metadata(HttpMetadata {
                    content_type: Some(content_type),
                    ..HttpMetadata::default()
                })
                .execute()
                .await
                .map_err(|e| backend_error(action, &e))?;
            Ok(())
        })
    }

    fn get_object(&self, key: &str) -> impl Future<Output = BackendResult<Option<Vec<u8>>>> + Send {
        let env = self.env.clone();
        let key = key.to_owned();
        SendFuture::new(async move {
            let action = "写真を R2 から読めない";
            let Some(object) = bucket(&env)?
                .get(key)
                .execute()
                .await
                .map_err(|e| backend_error(action, &e))?
            else {
                return Ok(None);
            };
            let Some(body) = object.body() else {
                return Ok(None);
            };
            body.bytes()
                .await
                .map(Some)
                .map_err(|e| backend_error(action, &e))
        })
    }

    fn delete_objects(&self, keys: Vec<String>) -> impl Future<Output = BackendResult<()>> + Send {
        let env = self.env.clone();
        SendFuture::new(async move {
            bucket(&env)?
                .delete_multiple(keys)
                .await
                .map_err(|e| backend_error("写真を R2 から消せない", &e))
        })
    }
}
