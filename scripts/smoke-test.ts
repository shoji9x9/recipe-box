#!/usr/bin/env node
// ローカルで動かした Worker（wrangler dev）に API を呼び、実際の D1 と R2 を通して動くかを確かめる。
// 単体テスト（cargo test）は D1 と R2 の代わりに偽物を使うので、SQL と R2 の読み書きそのものはここで確かめる。
// Node が型の注釈を取り除いて、そのまま動かす（ビルドしない。docs/development.md の「言語」）。
//
// 使い方: node scripts/smoke-test.ts [ベースの URL]（省略すると http://localhost:8787）
// ワークスペースとレシピを作るので、本番には使わない。
// 環境変数 CHECK_RATE_LIMIT=1 のときだけ、最後に回数の制限を確かめる（CI で使う）。
// この確認は 60 秒の枠を使い切るので、続けて実行すると最初のワークスペースの作成が 429 になる。
// 1 回の実行でワークスペースを最大 22 個作りうるので、同じローカルの D1 で同じ日に 2 回以上実行すると、
// アプリ全体の 1 日の上限（20 個）に当たることがある。そのときは 503 で止まり、作り直し方を示す。
import { request as httpRequest } from "node:http";
import { request as httpsRequest } from "node:https";

// 応答の本文の形（docs/spec.md の API）。確かめに使う項目だけを書く。
type Recipe = { id: string; name: string; servings: number; ingredients: string; steps: string };
type RecipeList = { recipes: Recipe[]; total: number };
type ErrorBody = { error: string; field?: string };
type Photo = { id: string; content_type: string };
type PhotoList = { photos: Photo[] };

/** 確かめた結果が違ったときに投げる。最後の応答の本文を添えて止まる。 */
class SmokeFailure extends Error {}

const args = process.argv.slice(2);
if (args.length > 1) {
  console.error("使い方: node scripts/smoke-test.ts [ベースの URL]");
  process.exit(2);
}
const baseUrl = args[0] ?? "http://localhost:8787";

// 書き込む API が回数の制限（60 秒に 30 回）で 429 になったら、待ってからやり直すか。
// 制限そのものを確かめるときだけ false にする。
let retryOnWriteLimit = true;

// 最後の応答。fail は、その本文を出す。
let status = 0;
let body = Buffer.alloc(0);

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function fail(message: string): never {
  throw new SmokeFailure(message);
}

type RequestOptions = {
  /** X-Workspace-Id に入れる ID。 */
  workspace?: string;
  /** JSON の本文。文字列はそのまま送る（2.0 のような、JSON.stringify が作れない書き方を送るため）。 */
  json?: unknown;
  /** バイト列のまま送る本文（写真の API）。 */
  bytes?: Buffer;
};

// API を呼び、状態コードを status に、本文を body に入れる。
async function request(method: string, path: string, options: RequestOptions = {}): Promise<void> {
  const headers: Record<string, string> = {};
  if (options.workspace !== undefined) {
    headers["X-Workspace-Id"] = options.workspace;
  }
  let payload: string | Buffer | undefined;
  // fetch は GET に本文を付けられないので、GET では送らない（サーバーも GET の本文を読まない）。
  if (method !== "GET") {
    if (options.bytes !== undefined) {
      headers["Content-Type"] = "application/octet-stream";
      payload = options.bytes;
    } else if (options.json !== undefined) {
      headers["Content-Type"] = "application/json";
      payload = typeof options.json === "string" ? options.json : JSON.stringify(options.json);
    }
  }
  await withWriteRetry(method, path, async () => {
    const response = await fetch(`${baseUrl}${path}`, {
      method,
      headers,
      ...(payload === undefined ? {} : { body: payload }),
      // curl と同じく、リダイレクトをたどらない（3xx をそのまま確かめる）。
      redirect: "manual",
      signal: AbortSignal.timeout(30_000),
    });
    status = response.status;
    body = Buffer.from(await response.arrayBuffer());
  });
}

// send で 1 回送る。書き込む API（/api/recipes の下の GET 以外）が回数の制限で 429 になったら、10 秒待って 8 回まで送り直す。
async function withWriteRetry(method: string, path: string, send: () => Promise<void>): Promise<void> {
  for (let attempt = 1; attempt <= 8; attempt++) {
    await send();
    if (status !== 429 || !retryOnWriteLimit || method === "GET" || !path.startsWith("/api/recipes")) {
      return;
    }
    console.log(`  書き込みの回数の制限に当たったので、10 秒待ってやり直す（${attempt} 回目）`);
    await sleep(10_000);
  }
}

// 本文を送らずに、Content-Length だけを大きく宣言して送る。
// 本当に大きな本文を送ると、サーバーが途中で断って接続を閉じ、送信の失敗で止まるため。
// fetch は Content-Length を本文に合わせて決めるので、node:http（https の URL なら node:https）を使う。
function requestWithDeclaredLength(path: string, workspace: string, length: number): Promise<void> {
  const url = new URL(`${baseUrl}${path}`);
  const requestFn = url.protocol === "https:" ? httpsRequest : httpRequest;
  return withWriteRetry("POST", path, () => sendWithDeclaredLength(requestFn, url, workspace, length));
}

function sendWithDeclaredLength(
  requestFn: typeof httpRequest,
  url: URL,
  workspace: string,
  length: number,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const req = requestFn(
      url,
      {
        method: "POST",
        headers: {
          "X-Workspace-Id": workspace,
          "Content-Type": "application/octet-stream",
          "Content-Length": String(length),
        },
        timeout: 30_000,
      },
      (response) => {
        const chunks: Buffer[] = [];
        response.on("data", (chunk: Buffer) => chunks.push(chunk));
        response.on("end", () => {
          status = response.statusCode ?? 0;
          body = Buffer.concat(chunks);
          // 宣言した長さを送り終えていないので、接続を閉じる。
          req.destroy();
          resolve();
        });
        response.on("error", reject);
      },
    );
    req.on("timeout", () => req.destroy(new Error("応答が 30 秒で返らない")));
    req.on("error", reject);
    req.write("x");
  });
}

function expect(expected: number, what: string): void {
  if (status !== expected) {
    fail(`${what}: 状態コードが ${expected} でなく ${status}`);
  }
}

// 最後の応答の本文を JSON として読む。形は API の決まり（docs/spec.md）を信じる。
function json<T>(): T {
  return JSON.parse(body.toString("utf8")) as T;
}

function text(): string {
  return body.toString("utf8");
}

function names(): string {
  return json<RecipeList>()
    .recipes.map((recipe) => recipe.name)
    .join(",");
}

function query(q: string, page?: number): string {
  return `/api/recipes?q=${encodeURIComponent(q)}${page === undefined ? "" : `&page=${page}`}`;
}

async function main(): Promise<void> {
  console.log("ヘルスチェックと画面");
  await request("GET", "/api/health");
  expect(200, "GET /api/health");
  if (text() !== '{"status":"ok"}') fail("/api/health の本文が違う");
  await request("GET", "/");
  expect(200, "GET /");
  // デモ（.dev.vars の DEMO_WORKSPACE_ID）があればデモを、無ければ「はじめる」画面を返す。
  if (!text().includes("はじめる") && !text().includes("共有のデモ")) {
    fail("Cookie が無いのに / が「はじめる」画面もデモも返さない");
  }
  // CSS と htmx は静的アセットとして配る（wrangler.toml の [assets] と、ビルドでの htmx のコピーを確かめる）。
  // 画面の動きは単体テスト（src/tests/screens.rs）で確かめるので、ここでは操作しない。
  await request("GET", "/assets/style.css");
  expect(200, "GET /assets/style.css");
  await request("GET", "/assets/htmx.min.js");
  expect(200, "GET /assets/htmx.min.js");
  if (!text().includes("htmx")) fail("/assets/htmx.min.js が htmx でない");
  // 解説（/about）も静的アセットで、Worker が動かないので、CSP などのヘッダーは public/_headers で付ける。
  const about = await fetch(`${baseUrl}/about/`, { redirect: "manual", signal: AbortSignal.timeout(30_000) });
  await about.body?.cancel();
  if (about.status !== 200) fail(`GET /about/: 状態コードが 200 でなく ${about.status}`);
  if (!about.headers.get("content-security-policy")?.startsWith("default-src 'none'")) {
    fail("/about/ に CSP が付いていない（public/_headers）");
  }
  if (about.headers.get("x-content-type-options") !== "nosniff") fail("/about/ に nosniff が付いていない");

  console.log("ワークスペースを 2 つ作る");
  await request("POST", "/api/workspaces");
  if (status === 429) {
    fail("ワークスペースを作る回数の制限に当たった。60 秒待ってから、もう一度実行する");
  }
  if (status === 503) {
    fail(
      "アプリ全体のワークスペースの上限（1 日 20 個）に当たった。ローカルの D1 を作り直す（apps/recipe-box-rs/.wrangler/state/v3/d1 を消す）か、UTC の翌日に実行する",
    );
  }
  expect(201, "POST /api/workspaces");
  const workspaceA = json<{ id: string }>().id;
  if (!/^[0-9a-f]{32}$/.test(workspaceA)) fail("ワークスペースの ID の形が違う");
  await request("POST", "/api/workspaces");
  expect(201, "POST /api/workspaces（2 つ目）");
  const workspaceB = json<{ id: string }>().id;
  const a = { workspace: workspaceA };
  const b = { workspace: workspaceB };

  console.log("レシピを登録する");
  await request("POST", "/api/recipes", {
    ...a,
    json: { name: "Beef Curry", servings: 4, ingredients: "牛肉\n玉ねぎ", steps: "煮る" },
  });
  expect(201, "POST /api/recipes");
  const created = json<Recipe>();
  const recipeId = created.id;
  if (created.ingredients !== "牛肉\n玉ねぎ") fail("材料の改行が保たれていない");
  // 一覧の並び順（作った日時の新しい順）を確かめるため、作った日時が同じミリ秒にならないよう少し待つ。
  await sleep(10);
  await request("POST", "/api/recipes", { ...a, json: { name: "100% Juice", servings: 1 } });
  expect(201, "POST /api/recipes（2 件目）");

  console.log("一覧と検索");
  await request("GET", "/api/recipes", a);
  expect(200, "GET /api/recipes");
  const listA = json<RecipeList>();
  if (listA.recipes.length !== 2) fail("一覧の件数が 2 でない");
  if (listA.recipes[0]?.name !== "100% Juice") fail("一覧が新しい順でない");
  await request("GET", query("CURRY"), a);
  expect(200, "GET /api/recipes?q=CURRY");
  if (names() !== "Beef Curry") fail("大文字の検索が部分一致しない");
  await request("GET", query("%"), a);
  expect(200, "GET /api/recipes?q=%");
  if (names() !== "100% Juice") fail("% を文字として検索しない");
  // _ と \ を含む料理名も、文字そのものとして探せる（instr はワイルドカードを持たない）。
  for (const name of ["A_B", "AxB", "a\\b"]) {
    await request("POST", "/api/recipes", { ...a, json: { name, servings: 1 } });
    expect(201, `POST /api/recipes（${name}）`);
  }
  await request("GET", query("_"), a);
  expect(200, "GET /api/recipes?q=_");
  if (names() !== "A_B") fail("_ を文字として検索しない");
  await request("GET", query("\\"), a);
  expect(200, "GET /api/recipes?q=\\");
  if (names() !== "a\\b") fail("\\ を文字として検索しない");
  // 上限いっぱい（100 文字）の日本語でも検索できる（instr は D1 の LIKE のパターンの上限 50 バイトを受けない）。
  const longName = "あ".repeat(100);
  await request("POST", "/api/recipes", { ...a, json: { name: longName, servings: 1 } });
  expect(201, "POST /api/recipes（100 文字の料理名）");
  await request("GET", query(longName), a);
  expect(200, "GET /api/recipes?q=<100 文字の日本語>");
  if (json<RecipeList>().recipes.length !== 1) fail("100 文字の日本語で検索できない");

  console.log("ページ分けと、レシピの数の上限（80 件）");
  // 別のワークスペースに、項目の長さも上限いっぱい（材料 2,000 文字・手順 5,000 文字）のレシピを 80 件作る。
  const ingredients = "あ".repeat(2000);
  const steps = "い".repeat(5000);
  for (let n = 1; n <= 80; n++) {
    await request("POST", "/api/recipes", { ...b, json: { name: `料理 ${n}`, servings: 1, ingredients, steps } });
    expect(201, `POST /api/recipes（料理 ${n}）`);
    // 並びを確かめるため、作った日時が同じミリ秒にならないよう少し待つ。
    await sleep(10);
  }
  await request("POST", "/api/recipes", { ...b, json: { name: "81 件目", servings: 1 } });
  expect(409, "81 件目のレシピ");
  if (json<ErrorBody>().error !== "recipe_limit_reached") fail("81 件目のエラーのコードが recipe_limit_reached でない");
  await request("GET", "/api/recipes", b);
  expect(200, "GET /api/recipes（1 ページ目）");
  const page1 = json<RecipeList>();
  if (page1.recipes.length !== 20) fail("1 ページ目が 20 件でない");
  if (page1.total !== 80) fail("総数が 80 でない");
  if (page1.recipes[0]?.name !== "料理 80") fail("1 ページ目が新しい順でない");
  await request("GET", "/api/recipes?page=4", b);
  expect(200, "GET /api/recipes?page=4");
  const page4 = json<RecipeList>();
  if (page4.recipes.length !== 20) fail("4 ページ目が 20 件でない");
  if (page4.recipes[19]?.name !== "料理 1") fail("4 ページ目の最後が最も古いレシピでない");
  await request("GET", query("料理 1", 1), b);
  expect(200, "GET /api/recipes?q=料理 1");
  if (json<RecipeList>().total !== 11) fail("検索の総数が 11（料理 1・10〜19）でない");
  await request("GET", "/api/recipes?page=5", b);
  expect(400, "page が 5（上限の 4 を超える）");
  if (json<ErrorBody>().field !== "page") fail("page が 5 のエラーの項目が page でない");

  console.log("取得・更新");
  await request("GET", `/api/recipes/${recipeId}`, a);
  expect(200, "GET /api/recipes/{id}");
  await request("PUT", `/api/recipes/${recipeId}`, { ...a, json: { name: "辛口カレー", servings: 2 } });
  expect(200, "PUT /api/recipes/{id}");
  const updated = json<Recipe>();
  if (updated.name !== "辛口カレー") fail("更新した料理名が返らない");
  if (updated.ingredients !== "") fail("PUT で省いた材料が空にならない");
  // 更新で検索用の列（search_name）も作り直される（偽物の Backend では確かめられないので、ここで確かめる）。
  await request("GET", query("辛口"), a);
  expect(200, "GET /api/recipes?q=辛口（A、更新の後）");
  if (json<RecipeList>().total !== 1) fail("更新した料理名で検索できない（search_name が更新されていない）");
  await request("GET", query("beef"), a);
  expect(200, "GET /api/recipes?q=beef（A、更新の後）");
  if (json<RecipeList>().total !== 0) fail("更新前の料理名で検索に当たる");

  console.log("入力の検証");
  await request("POST", "/api/recipes", { ...a, json: { name: "", servings: 2 } });
  expect(400, "空の料理名");
  if (json<ErrorBody>().field !== "name") fail("空の料理名のエラーの項目が name でない");
  await request("POST", "/api/recipes", { ...a, json: { name: "カレー", servings: 0 } });
  expect(400, "servings が 0");
  // servings は書き方ではなく値で決める（2.0 は 2 として受け付け、桁あふれは invalid_field）。
  // JSON.stringify は 2.0 を 2 と書くので、本文をそのまま書く。
  await request("POST", "/api/recipes", { ...a, json: '{"name":"小数の書き方","servings":2.0}' });
  expect(201, "servings が 2.0");
  const decimal = json<Recipe>();
  if (decimal.servings !== 2) fail("servings の 2.0 が 2 にならない");
  await request("DELETE", `/api/recipes/${decimal.id}`, a);
  expect(204, "servings が 2.0 のレシピを消す");
  await request("POST", "/api/recipes", { ...a, json: '{"name":"カレー","servings":99999999999999999999}' });
  expect(400, "servings が桁あふれ");
  if (json<ErrorBody>().field !== "servings") fail("桁あふれのエラーの項目が servings でない");

  console.log("ほかのワークスペースからは見えない");
  await request("GET", query("辛口"), b);
  expect(200, "GET /api/recipes?q=辛口（B）");
  if (json<RecipeList>().total !== 0) fail("B の一覧に A のレシピが出る");
  for (const method of ["GET", "PUT", "DELETE"]) {
    await request(method, `/api/recipes/${recipeId}`, { ...b, json: { name: "乗っ取り", servings: 1 } });
    expect(404, `${method} /api/recipes/{id}（B から）`);
  }
  await request("GET", `/api/recipes/${recipeId}`, a);
  expect(200, "GET /api/recipes/{id}（A から、B の操作の後）");
  if (json<Recipe>().name !== "辛口カレー") fail("B の操作で A のレシピが変わった");

  console.log("ワークスペースの ID の誤り");
  await request("GET", "/api/recipes");
  expect(400, "X-Workspace-Id が無い");
  await request("GET", "/api/recipes", { workspace: "f".repeat(32) });
  expect(404, "無いワークスペース");

  console.log("写真（R2）");
  // PNG の先頭のバイトで始まる、小さなデータ。
  const photo = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), Buffer.from("smoke-test")]);
  await request("POST", `/api/recipes/${recipeId}/photos`, { ...a, bytes: photo });
  expect(201, "POST /api/recipes/{id}/photos");
  const added = json<Photo>();
  if (added.content_type !== "image/png") fail("写真の形式が image/png でない");
  const photoId = added.id;
  await request("GET", `/api/recipes/${recipeId}/photos`, a);
  expect(200, "GET /api/recipes/{id}/photos");
  if (json<PhotoList>().photos.length !== 1) fail("写真の一覧が 1 件でない");
  await request("GET", `/api/recipes/${recipeId}/photos/${photoId}`, a);
  expect(200, "GET /api/recipes/{id}/photos/{photo_id}");
  if (!body.equals(photo)) fail("R2 から読んだ写真が、送ったものと違う");
  await request("GET", `/api/recipes/${recipeId}/photos/${photoId}`, b);
  expect(404, "GET /api/recipes/{id}/photos/{photo_id}（B から）");
  // 4,000,000 バイトを 1 バイト超える写真は、本文を読む前に Content-Length で断る。
  await requestWithDeclaredLength(`/api/recipes/${recipeId}/photos`, workspaceA, 4_000_001);
  expect(413, "Content-Length が 4,000,001 バイトの写真");

  console.log("削除");
  await request("DELETE", `/api/recipes/${recipeId}`, a);
  expect(204, "DELETE /api/recipes/{id}");
  await request("GET", `/api/recipes/${recipeId}`, a);
  expect(404, "消したレシピ");
  // レシピを消すと、写真の行も（D1 の ON DELETE CASCADE で）消える。
  await request("GET", `/api/recipes/${recipeId}/photos/${photoId}`, a);
  expect(404, "消したレシピの写真");

  console.log("ワークスペースの削除");
  // 写真の付いたレシピのあるワークスペースを、レシピ 80 件と写真ごと消す（D1 の ON DELETE CASCADE と R2 の削除）。
  await request("GET", "/api/recipes", b);
  expect(200, "GET /api/recipes（B、削除の前）");
  const recipeB = json<RecipeList>().recipes[0]?.id ?? fail("B の一覧が空");
  await request("POST", `/api/recipes/${recipeB}/photos`, { ...b, bytes: photo });
  expect(201, "POST /api/recipes/{id}/photos（B）");
  await request("DELETE", "/api/workspaces", b);
  expect(204, "DELETE /api/workspaces");
  await request("GET", "/api/recipes", b);
  expect(404, "消したワークスペース");
  if (json<ErrorBody>().error !== "workspace_not_found") fail("消したワークスペースのエラーが workspace_not_found でない");
  await request("GET", `/api/recipes/${recipeId}`, a);
  expect(404, "B を消した後も、A は使える（消したレシピは 404）");
  if (json<ErrorBody>().error !== "recipe_not_found") fail("B を消したら A まで消えた");

  // 最後に、回数の制限（Rate Limiting）を確かめる。
  if (process.env["CHECK_RATE_LIMIT"] === "1") {
    // 数え始めが 60 秒の枠の途中でも確かめられるよう、枠 2 つ分より 1 回多く試す。
    console.log("書き込む API の回数の制限（60 秒に 30 回）");
    retryOnWriteLimit = false;
    let limited = false;
    for (let i = 0; i < 61 && !limited; i++) {
      await request("DELETE", `/api/recipes/${recipeId}`, a);
      limited = status === 429;
      if (!limited) expect(404, "DELETE /api/recipes/{id}（制限の確認）");
    }
    if (!limited) fail("書き込みの回数の制限を超えても 429 にならない");

    // ワークスペースを作る回数の制限（60 秒に 10 回）。上と同じく、枠 2 つ分より 1 回多く試す。
    console.log("ワークスペースを作る回数の制限");
    limited = false;
    for (let i = 0; i < 21 && !limited; i++) {
      await request("POST", "/api/workspaces");
      limited = status === 429;
      if (!limited) expect(201, "POST /api/workspaces（制限の確認）");
    }
    if (!limited) fail("回数の制限を超えても 429 にならない");
  }

  console.log("すべて通った");
}

try {
  await main();
} catch (error) {
  // 確かめた結果の違いは、GitHub Actions の注釈（::error::）にし、最後の応答の本文を添える。
  // 接続の失敗など、ほかの例外もそのまま出して止まる。
  // fetch の接続の失敗は、理由（ECONNREFUSED など）を cause に持つ。
  const message =
    error instanceof SmokeFailure || !(error instanceof Error)
      ? String(error instanceof Error ? error.message : error)
      : `${error.message}${error.cause === undefined ? "" : `（${String(error.cause)}）`}`;
  console.error(`::error::${message}`);
  console.error("--- 最後の応答の本文 ---");
  console.error(body.toString("utf8"));
  process.exit(1);
}
