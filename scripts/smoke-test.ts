#!/usr/bin/env node
// ローカルで動かした Worker（wrangler dev）に API を呼び、実際の D1 と R2 を通して動くかを確かめる。
// 単体テスト（cargo test）は D1 と R2 の代わりに偽物を使うので、SQL と R2 の読み書きそのものはここで確かめる。
// Node が型の注釈を取り除いて、そのまま動かす（ビルドしない。docs/development.md の「言語」）。
//
// 使い方: node scripts/smoke-test.ts [ベースの URL]（省略すると http://localhost:8787）
// ワークスペースとレシピを作るので、本番には使わない。
// 環境変数 CHECK_RATE_LIMIT=1 のときだけ、最後に回数の制限を確かめる（CI で使う）。
// この確認は 60 秒の枠を使い切るので、続けて実行すると最初のワークスペースの作成が 429 になる。
// 1 回の実行でワークスペースを最大 23 個（定期の自動削除の確認を含む）作りうるので、同じローカルの D1 で同じ日に 2 回以上実行すると、
// アプリ全体の 1 日の上限（20 個）に当たることがある。そのときは 503 で止まり、作り直し方を示す。
// 環境変数 SCHEDULED_LOG に wrangler dev --test-scheduled のログのファイルを渡したときだけ、定期の自動削除を動かして確かめる（CI で使う）。
// この確認は、wrangler でローカルの D1 を直接書き換える（ワークスペースを古くし、写真の行に印を付ける）。
// 環境変数 API_ONLY=1 のときは、API（docs/spec.md の「API」）だけを確かめ、今の作りに頼る画面と静的アセット
// （/ の文字・/assets/ のファイル・/about の _headers）を確かめない。リライトの後の新しいアプリに当てるときに使う。
// SCHEDULED_LOG の確認も、今の作り（wrangler・ローカルの D1 の表・ログの文）に頼るので、新しいアプリには付けない。
import { request as httpRequest } from "node:http";
import { request as httpsRequest } from "node:https";
import { readFileSync, statSync } from "node:fs";
import { d1Rows } from "./seed-lib.ts";
import { ID_PATTERN, wrangler } from "./wrangler.ts";

// 応答の本文の形（docs/spec.md の API）。確かめに使う項目だけを書く。
type Recipe = { id: string; name: string; servings: number; ingredients: string; steps: string };
type RecipeList = { recipes: Recipe[]; total: number; page: number; per_page: number };
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
const apiOnly = process.env["API_ONLY"] === "1";
const scheduledLog = process.env["SCHEDULED_LOG"] ?? "";
if (apiOnly && scheduledLog !== "") {
  // 定期の自動削除の確かめは、今の作りのローカルの D1 を書き換えるので、新しいアプリに当てる API_ONLY=1 とは一緒に使わない。
  console.error("API_ONLY=1 と SCHEDULED_LOG は一緒に使えない（SCHEDULED_LOG は今の作りのローカルの D1 を書き換える）");
  process.exit(2);
}

// 書き込む API が回数の制限（60 秒に 30 回）で 429 になったら、待ってからやり直すか。
// 制限そのものを確かめるときだけ false にする。
let retryOnWriteLimit = true;

// 最後の応答。fail は、その本文を出す。
let status = 0;
let body = Buffer.alloc(0);
let responseHeaders = new Headers();

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
    responseHeaders = response.headers;
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

// ローカルの D1 に SQL を流す。ID は形を確かめてから埋め込む（wrangler d1 execute は値を別に渡せない）。
async function localSql(sql: string): Promise<unknown[]> {
  const rows = d1Rows(await wrangler("--local", "d1", "execute", "DB", "--json", "--command", sql));
  if (rows === undefined) fail("wrangler の d1 execute --json の出力の形が、思ったものと違う");
  return rows;
}

function checkedId(id: string): string {
  if (!ID_PATTERN.test(id)) fail("ID の形が違う");
  return id;
}

// ログのファイルの、offset バイト目より後に、自動削除の結果の行が出るのを待って返す（src/lib.rs の scheduled）。
async function waitForCleanupLog(path: string, offset: number): Promise<string> {
  for (let i = 0; i < 30; i++) {
    const lines = readFileSync(path).subarray(offset).toString("utf8").split("\n");
    const line = lines.find((l) => l.includes("自動削除が終わった:") || l.includes("自動削除で失敗があった:"));
    if (line !== undefined) return line;
    await sleep(1000);
  }
  return fail("定期の自動削除の結果がログに出ない");
}

// 定期の自動削除（Cron Trigger）を /__scheduled で動かし、本物の D1 と R2 で、使われていないワークスペースの削除と、
// 書き込み中の印が残った写真の片付けが、失敗せずに動くことを確かめる。
// kept は、使っているワークスペース（印の残った写真を置く）。古くするワークスペースは、ここで 1 つだけ作る（1 日の作成の上限を使いすぎない）。
async function checkScheduled(logPath: string, photo: Buffer, keptWorkspace: string): Promise<void> {
  console.log("定期の自動削除（/__scheduled）");
  await request("POST", "/api/workspaces");
  expect(201, "POST /api/workspaces（古くする）");
  const unused = checkedId(json<{ id: string }>().id);
  const kept = checkedId(keptWorkspace);
  const photoIds: string[] = [];
  const recipeIds: string[] = [];
  for (const workspace of [unused, kept]) {
    await request("POST", "/api/recipes", { workspace, json: { name: "定期の削除", servings: 1 } });
    expect(201, "POST /api/recipes（定期の削除）");
    const recipe = json<Recipe>().id;
    recipeIds.push(recipe);
    await request("POST", `/api/recipes/${recipe}/photos`, { workspace, bytes: photo });
    expect(201, "POST /api/recipes/{id}/photos（定期の削除）");
    photoIds.push(checkedId(json<Photo>().id));
  }
  const keptPhoto = photoIds[1] ?? fail("写真が無い");
  // 14 日より前に使われたことにし、写真の行には 1 時間より前の印を付ける（日時は UNIX 時間のミリ秒）。
  await localSql(
    `UPDATE workspaces SET last_used_at = 0 WHERE id = '${unused}'; ` +
      `UPDATE photos SET pending_since = 0 WHERE workspace_id = '${kept}' AND id = '${keptPhoto}'`,
  );
  const offset = statSync(logPath).size;
  // cron の値は、wrangler.toml の [triggers] と同じにする（アプリは値を見ない）。
  await request("GET", "/__scheduled?cron=0+*+*+*+*");
  expect(200, "GET /__scheduled");
  const line = await waitForCleanupLog(logPath, offset);
  if (!line.includes("自動削除が終わった:")) fail(`定期の自動削除で失敗があった: ${line}`);
  console.log(`  ${line.trim()}`);
  // 古くしたワークスペースは、レシピと写真ごと消える。
  await request("GET", "/api/recipes", { workspace: unused });
  expect(404, "定期の自動削除で消したワークスペース");
  if (json<ErrorBody>().error !== "workspace_not_found") fail("自動削除で消したワークスペースのエラーが workspace_not_found でない");
  const unusedRows = await localSql(`SELECT id FROM photos WHERE workspace_id = '${unused}'`);
  if (unusedRows.length !== 0) fail("自動削除で消したワークスペースの写真の行が残っている");
  // 印の残った写真の行は片付けられ、ワークスペースとレシピは残る。
  await request("GET", `/api/recipes/${recipeIds[1] ?? ""}`, { workspace: kept });
  expect(200, "印の残った写真のレシピ（自動削除の後）");
  const keptRows = await localSql(`SELECT id FROM photos WHERE workspace_id = '${kept}'`);
  if (keptRows.length !== 0) fail("印の残った写真の行が、自動削除で片付けられていない");
}

// 画面と静的アセット。今の作り（画面の文字・public/ のファイル・_headers）に頼るので、API_ONLY=1 のときは確かめない。
async function checkScreens(): Promise<void> {
  console.log("画面と静的アセット");
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
  // /about は /about/ に移す応答で、どちらにもヘッダーを付ける。
  for (const [path, expected] of [["/about", 307], ["/about/", 200]] as const) {
    await request("GET", path);
    expect(expected, `GET ${path}`);
    if (!responseHeaders.get("content-security-policy")?.startsWith("default-src 'none'")) {
      fail(`${path} に CSP が付いていない（public/_headers）`);
    }
    if (responseHeaders.get("x-content-type-options") !== "nosniff") fail(`${path} に nosniff が付いていない`);
  }
  if (!text().includes("このアプリの作り")) fail("/about/ が解説でない");
}

// 無い API のパスと、受け付けないメソッドも、ほかの失敗と同じ JSON の形で答える（docs/spec.md の「API」）。
async function checkApiFallbacks(workspace: string): Promise<void> {
  console.log("無い API のパスと、受け付けないメソッド");
  for (const path of ["/api", "/api/", "/api/nothing", "/api/recipes/x/y/z/w"]) {
    await request("GET", path, { workspace });
    expect(404, `GET ${path}`);
    if (json<ErrorBody>().error !== "not_found") fail(`${path} のエラーが not_found でない`);
  }
  for (const [method, path] of [
    ["PATCH", "/api/recipes"],
    ["DELETE", "/api/recipes"],
    ["GET", "/api/workspaces"],
    ["POST", "/api/health"],
  ] as const) {
    await request(method, path, { workspace });
    expect(405, `${method} ${path}`);
    if (json<ErrorBody>().error !== "method_not_allowed") fail(`${method} ${path} のエラーが method_not_allowed でない`);
  }
}

async function main(): Promise<void> {
  console.log("ヘルスチェック");
  await request("GET", "/api/health");
  expect(200, "GET /api/health");
  if (text() !== '{"status":"ok"}') fail("/api/health の本文が違う");
  if (apiOnly) {
    console.log("  画面と静的アセットは確かめない（API_ONLY=1）");
  } else {
    await checkScreens();
  }

  console.log("ワークスペースを 2 つ作る");
  await request("POST", "/api/workspaces");
  if (status === 429) {
    fail("ワークスペースを作る回数の制限に当たった。60 秒待ってから、もう一度実行する");
  }
  if (status === 503) {
    fail(
      apiOnly
        ? "アプリ全体のワークスペースの上限（1 日 20 個）に当たった。アプリのデータを作り直すか、UTC の翌日に実行する"
        : "アプリ全体のワークスペースの上限（1 日 20 個）に当たった。ローカルの D1 を作り直す（apps/recipe-box-rs/.wrangler/state/v3/d1 を消す）か、UTC の翌日に実行する",
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
  if (responseHeaders.get("location") !== `/api/recipes/${recipeId}`) fail("POST /api/recipes の Location が違う");
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
  if (page1.page !== 1 || page1.per_page !== 20) fail("1 ページ目の page と per_page が 1 と 20 でない");
  await request("GET", "/api/recipes?page=4", b);
  expect(200, "GET /api/recipes?page=4");
  const page4 = json<RecipeList>();
  if (page4.recipes.length !== 20) fail("4 ページ目が 20 件でない");
  if (page4.recipes[19]?.name !== "料理 1") fail("4 ページ目の最後が最も古いレシピでない");
  if (page4.page !== 4 || page4.per_page !== 20) fail("4 ページ目の page と per_page が 4 と 20 でない");
  await request("GET", query("料理 1", 1), b);
  expect(200, "GET /api/recipes?q=料理 1");
  if (json<RecipeList>().total !== 11) fail("検索の総数が 11（料理 1・10〜19）でない");
  // 最後のページより後ろの番号では、空の recipes と総数を返す。
  await request("GET", query("料理 1", 2), b);
  expect(200, "GET /api/recipes?q=料理 1&page=2");
  const pastLast = json<RecipeList>();
  if (pastLast.recipes.length !== 0 || pastLast.total !== 11 || pastLast.page !== 2) {
    fail("最後のページより後ろで、空の一覧と総数が返らない");
  }
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
  // 写真を配るときのヘッダー（docs/spec.md の「写真の API」）。
  for (const [name, value] of [
    ["content-type", "image/png"],
    ["cache-control", "private, max-age=86400"],
    ["vary", "X-Workspace-Id, Cookie"],
    ["x-content-type-options", "nosniff"],
    ["content-security-policy", "default-src 'none'; frame-ancestors 'none'; sandbox"],
  ] as const) {
    if (responseHeaders.get(name) !== value) fail(`写真の ${name} が ${value} でない`);
  }
  await request("GET", `/api/recipes/${recipeId}/photos/${photoId}`, b);
  expect(404, "GET /api/recipes/{id}/photos/{photo_id}（B から）");
  // 4,000,000 バイトを 1 バイト超える写真は、本文を読む前に Content-Length で断る。
  await requestWithDeclaredLength(`/api/recipes/${recipeId}/photos`, workspaceA, 4_000_001);
  expect(413, "Content-Length が 4,000,001 バイトの写真");
  // JPEG・PNG・WebP のどれでもない中身は、415 で断る（形式は先頭のバイトで決める）。
  await request("POST", `/api/recipes/${recipeId}/photos`, { ...a, bytes: Buffer.from("GIF89a smoke-test") });
  expect(415, "JPEG・PNG・WebP でない写真");
  if (json<ErrorBody>().error !== "unsupported_photo_type") fail("形式の誤りのエラーが unsupported_photo_type でない");
  // 1 つのレシピの写真は 5 枚まで。6 枚目は 409 で断る。
  for (let n = 2; n <= 5; n++) {
    await request("POST", `/api/recipes/${recipeId}/photos`, { ...a, bytes: photo });
    expect(201, `POST /api/recipes/{id}/photos（${n} 枚目）`);
  }
  await request("POST", `/api/recipes/${recipeId}/photos`, { ...a, bytes: photo });
  expect(409, "6 枚目の写真");
  if (json<ErrorBody>().error !== "photo_limit_reached") fail("6 枚目の写真のエラーが photo_limit_reached でない");
  // 写真を消すと、一覧から消え、データも読めなくなる。ほかのワークスペースからは消せない。
  await request("DELETE", `/api/recipes/${recipeId}/photos/${photoId}`, b);
  expect(404, "DELETE /api/recipes/{id}/photos/{photo_id}（B から）");
  await request("DELETE", `/api/recipes/${recipeId}/photos/${photoId}`, a);
  expect(204, "DELETE /api/recipes/{id}/photos/{photo_id}");
  if (body.length !== 0) fail("写真の削除の応答に本文がある");
  await request("GET", `/api/recipes/${recipeId}/photos/${photoId}`, a);
  expect(404, "消した写真");
  if (json<ErrorBody>().error !== "photo_not_found") fail("消した写真のエラーが photo_not_found でない");
  await request("DELETE", `/api/recipes/${recipeId}/photos/${photoId}`, a);
  expect(404, "消した写真をもう一度消す");
  if (json<ErrorBody>().error !== "photo_not_found") fail("消した写真をもう一度消したエラーが photo_not_found でない");
  await request("GET", `/api/recipes/${recipeId}/photos`, a);
  expect(200, "GET /api/recipes/{id}/photos（削除の後）");
  const remaining = json<PhotoList>().photos;
  if (remaining.length !== 4 || remaining.some((p) => p.id === photoId)) fail("写真の削除の後の一覧が 4 件でない");
  const keptPhotoId = remaining[0]?.id ?? fail("残った写真が無い");
  // 枠が空いたので、もう 1 枚追加できる。
  await request("POST", `/api/recipes/${recipeId}/photos`, { ...a, bytes: photo });
  expect(201, "POST /api/recipes/{id}/photos（削除の後）");

  await checkApiFallbacks(workspaceA);

  console.log("削除");
  await request("DELETE", `/api/recipes/${recipeId}`, a);
  expect(204, "DELETE /api/recipes/{id}");
  await request("GET", `/api/recipes/${recipeId}`, a);
  expect(404, "消したレシピ");
  // 消したレシピの写真は読めない（写真の行を、ワークスペースとレシピと写真の ID で探すので photo_not_found になる）。
  // 写真の行が D1 から消えたかは、API からは見分けられない（消す前に印を付けて隠すので、行が残っても同じ 404 になる）。
  await request("GET", `/api/recipes/${recipeId}/photos/${keptPhotoId}`, a);
  expect(404, "消したレシピの写真");
  if (json<ErrorBody>().error !== "photo_not_found") fail("消したレシピの写真のエラーが photo_not_found でない");

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

  if (scheduledLog !== "") {
    await checkScheduled(scheduledLog, photo, workspaceA);
  }

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
