#!/usr/bin/env node
// 共有のデモのワークスペースを、サンプルのレシピと写真の状態に戻す（docs/spec.md の「デモ用のデータ」）。
//
// 使い方: node scripts/seed.mjs (--local | --remote) < <デモの ID を書いたファイル>
//   --local:  ローカルの Worker（先に別の端末で mise run dev）と、そのローカルの D1・R2
//   --remote: 本番。写真を入れるのに wrangler の認証（wrangler login か CLOUDFLARE_API_TOKEN）が要る
// デモの ID は公開しないので、引数ではなく標準入力で受け取る。ID は表示しない。
//
// 手順:
//   1. seed/recipes.json と seed/photos/ を、API と同じ決まりで確かめる（入れ始めてから失敗しないように）。
//      wrangler で、ID が印（mise run pin）の付いたワークスペースのものか、アプリ全体の写真の合計の上限を超えないか、
//      R2 に書けて消せるかを確かめる。
//   2. 公開 API で、デモのレシピを全部消す（レシピを消すと、その写真も消える）。
//   3. 公開 API で、サンプルのレシピを入れる。
//   4. 写真は、デモでは API で追加できないので、wrangler で D1 の行と R2 のデータを直接入れる。
//      アプリと同じく D1 の行を先に入れる。書いた後に行が消えていれば（レシピが消された）、その写真の R2 のデータを消す。
//      ほかの失敗では、入れた写真を消してから止まる（R2 に書き始める前なら行だけを、後なら R2・D1 の順に）。
//      R2 の操作と、D1 の写真の行の読み取り・削除は、wrangler が失敗したら待ってやり直す。
//      どの行からもたどれないデータを R2 に残さないためである。R2 から消せず、行も無いデータは、手で消せるようキーを表示する。
//   5. 公開 API で、レシピの数と、写真をすべて読めて中身が同じことを確かめる。
// 公開 API は、書き込みの回数の制限（429）なら待ってやり直し、接続の失敗と 5xx なら GET と DELETE だけをやり直す。
// 何度実行しても同じ状態になる。途中で失敗したら、もう一度実行すれば入れ直せる。
import { execFile } from "node:child_process";
import { randomBytes } from "node:crypto";
import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify, stripVTControlCharacters } from "node:util";
import {
  pathPattern,
  regularFileWithoutLinks,
  SeedDataError,
  redact as redactWith,
  shortReason,
  validateRecipes,
} from "./seed-lib.mjs";

const run = promisify(execFile);
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const seedDir = join(root, "seed");
const photoDir = join(seedDir, "photos");
const appDir = join(root, "apps", "recipe-box-rs");
// 写真 1 枚ごとに pnpm を起こさないよう、pnpm install が入れた wrangler を直接動かす。
const wranglerBin = join(root, "node_modules", ".bin", "wrangler");
// ログで . と ~ に置き換える、リポジトリとホームのディレクトリのパス（端末の利用者名が入る。seed-lib.mjs の pathPattern）。
const PATHS = [
  [pathPattern(root), "."],
  [pathPattern(homedir()), "~"],
].filter(([pattern]) => pattern !== null);

const BASE_URLS = {
  "--local": "http://localhost:8787",
  "--remote": "https://recipe-box.shoji9x9.workers.dev",
};
const BUCKET = "recipe-box-photos";
// seed が使う R2 の操作と、ログでの呼び方。
const R2_ACTIONS = { put: "R2 への書き込み", delete: "R2 からの削除" };
// アプリ全体の写真の合計の上限（docs/security.md）。seed は写真の上限を判定する API を通らずに入れるので、seed で確かめる。
const MAX_TOTAL_PHOTO_BYTES = 5_000_000_000;
// 写真の行を入れる SQL を、1 回の wrangler にこの数ずつ渡す（コマンドの引数と D1 の SQL の長さの上限に収める）。
const INSERT_BATCH = 50;
// 書き込む API の回数の制限（60 秒に 30 回）で 429 になったら、枠が空くまで待ってやり直す。
const RATE_LIMIT_WAIT_MS = 61_000;
const MAX_RETRIES = 5;
// レシピを全部消すときの、一覧を読み直す回りの数の上限。
const MAX_DELETE_ROUNDS = 10;
// 写真の R2 の操作で wrangler を同時に動かす数。ローカルでは、wrangler dev が使う保存先に同時に書くと失敗することがあるので、1 つずつ書く。
const UPLOAD_CONCURRENCY = { "--local": 1, "--remote": 4 };
// wrangler の R2 の書き込み・削除と、D1 の写真の行の読み取り・削除が失敗したときに試す回数と、やり直す前に待つ時間（回ごとに延ばす）。
const RETRY_ATTEMPTS = 4;
const RETRY_WAIT_MS = 5_000;

const ID_PATTERN = /^[0-9a-f]{32}$/;

// 失敗の文は、ここで必ず伏せる（redact）。呼ぶ側で伏せ忘れても、ID とパスを出さないためである。
// 進み具合の表示（console.log）は ID もパスも含まないので、そのまま出す。手で消すキーの表示（shownKey）は、
// レシピと写真の ID を見せる必要があるので redact を通さない（デモの ID だけを shownKey が伏せる）。
function fail(message) {
  console.error(`seed: ${redact(message)}`);
  process.exit(1);
}

function warn(message) {
  console.error(redact(message));
}

function usage() {
  console.error("使い方: node scripts/seed.mjs (--local | --remote) < <デモの ID を書いたファイル>");
  process.exit(2);
}

const args = process.argv.slice(2);
if (args.length !== 1 || !(args[0] in BASE_URLS)) {
  usage();
}
const target = args[0];
const baseUrl = BASE_URLS[target];

// 1 行目だけを読み、空白を除く。
const workspaceId = readFileSync(0, "utf8").split("\n")[0].replace(/\s/g, "");
// ID は R2 のキーと SQL に埋め込むので、形（16 進数の小文字 32 文字）を必ず確かめる。
if (!ID_PATTERN.test(workspaceId)) {
  fail("ワークスペースの ID の形が正しくない（16 進数の小文字 32 文字）");
}

// 表示やエラーの文から、ID の形の値を伏せ、リポジトリとホームのパスを . と ~ に置き換える（seed-lib.mjs の redact）。
function redact(text) {
  return redactWith(text, PATHS);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// seed/recipes.json を読み、API と同じ決まりで確かめる（seed-lib.mjs の validateRecipes）。
// データの誤り（SeedDataError）は文だけを、それ以外（プログラムの誤りなど）は原因を探せるよう stack も出す。
function loadRecipes() {
  try {
    regularFileWithoutLinks(join(seedDir, "recipes.json"), root, "seed/recipes.json");
    let recipes;
    try {
      recipes = JSON.parse(readFileSync(join(seedDir, "recipes.json"), "utf8"));
    } catch (error) {
      throw new SeedDataError(`seed/recipes.json を読めない: ${error.message}`);
    }
    return validateRecipes(recipes, photoDir, root);
  } catch (error) {
    fail(error instanceof SeedDataError ? error.message : (error.stack ?? String(error)));
  }
}

// 公開 API を呼ぶ。書き込みが回数の制限で 429 になったら、待ってやり直す。
// 接続の失敗と 5xx も、一時的な失敗として、待ち時間を延ばしながらやり直す。ただし GET と DELETE だけにする。
// POST は、登録できた後で応答だけが失われることがあり、やり直すと同じレシピが 2 件できるためである。
async function api(method, path, body) {
  const retryable = method === "GET" || method === "DELETE";
  const label = `${method} ${path.split("?")[0]}`;
  for (let attempt = 0; ; attempt += 1) {
    let response;
    try {
      response = await fetch(`${baseUrl}${path}`, {
        method,
        headers: {
          "X-Workspace-Id": workspaceId,
          ...(body === undefined ? {} : { "Content-Type": "application/json" }),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(30_000),
      });
    } catch (error) {
      // fetch の error.message は「fetch failed」だけなので、原因（ECONNRESET など）を添える。
      // 接続を何度か試して失敗すると、cause が AggregateError になり、原因はその errors に入る。
      const cause =
        error.cause?.code ?? error.cause?.errors?.[0]?.code ?? error.cause?.errors?.[0]?.message ?? error.cause?.message;
      const reason = `${error.message}${cause ? `（${cause}）` : ""}`;
      if (!retryable || attempt >= MAX_RETRIES) {
        throw new Error(`${label} に失敗した: ${reason}`);
      }
      warn(`${label} に失敗したので、やり直す（${attempt + 1} 回目）: ${reason}`);
      await sleep(RETRY_WAIT_MS * (attempt + 1));
      continue;
    }
    if (response.status >= 500 && retryable && attempt < MAX_RETRIES) {
      await response.body?.cancel();
      warn(`${label} が ${response.status} を返したので、やり直す（${attempt + 1} 回目）`);
      await sleep(RETRY_WAIT_MS * (attempt + 1));
      continue;
    }
    if (response.status === 429 && attempt < MAX_RETRIES) {
      await response.body?.cancel();
      console.error(`書き込みの回数の制限に当たったので、${RATE_LIMIT_WAIT_MS / 1000} 秒待つ`);
      await sleep(RATE_LIMIT_WAIT_MS);
      continue;
    }
    return response;
  }
}

async function expectStatus(response, status, what) {
  if (response.status !== status) {
    const text = await response.text().catch(() => "");
    fail(`${what} に失敗した（${response.status}）: ${redact(text).slice(0, 300)}`);
  }
}

// wrangler を動かす。出力には ID が入るので、失敗したときだけ、ID を伏せて表示する。
async function wrangler(...wranglerArgs) {
  try {
    const { stdout } = await run(wranglerBin, [...wranglerArgs, target], {
      cwd: appDir,
      // 色の制御文字を出させない（--json の出力を JSON として読むためと、制御文字で値が分かれると伏せられなくなるため）。
      env: { ...process.env, FORCE_COLOR: "0", NO_COLOR: "1" },
      maxBuffer: 16 * 1024 * 1024,
    });
    return stdout;
  } catch (error) {
    // 理由は stderr か stdout（--json のとき）にある。Node のエラーの文は、コマンドの全体（ホームのディレクトリのパスと SQL）を
    // 含むので使わず、終了コードかシグナル（起動できなければ ENOENT などのコード）だけを添える。
    // 制御文字（色など）は、伏せる前に取り除く（制御文字に挟まれた値も伏せるため）。
    // ログのファイルの場所の行は、理由にならないので除く。
    const output = redact(
      stripVTControlCharacters(
        [error.stderr, error.stdout]
          .filter((text) => text?.trim())
          .join("\n")
          .split("\n")
          .filter((line) => !line.includes("Logs were written"))
          .join("\n"),
      ),
    );
    // 終了コードもシグナルも無ければ（execFile ではふつう起きない）、エラーの名前を添える。
    const detail = [error.code, error.signal].filter((value) => value != null).join("・") || error.name;
    // 伏せた後で切り詰める。長いときは、頭（stderr の [ERROR]）と終わり（stdout の JSON のエラー）の両方を残す。
    const text = output.length > 4000 ? `${output.slice(0, 2000)}\n…\n${output.slice(-2000)}` : output;
    throw new Error(`wrangler が失敗した（${detail}）\n${text}`);
  }
}

// wrangler を動かし、失敗したら待ち時間を延ばしながら RETRY_ATTEMPTS 回まで試す。最後の失敗は投げる。
// what は「写真の行の読み取り」のような、何をするかを表す名詞の句にする。
async function wranglerWithRetry(what, ...wranglerArgs) {
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await wrangler(...wranglerArgs);
    } catch (error) {
      if (attempt >= RETRY_ATTEMPTS) {
        throw error;
      }
      // 理由は短くして出す（全文は、やり直しても失敗したときに呼ぶ側が出す）。error.message は wrangler() が伏せている。
      warn(`${what}に失敗したので、やり直す（${attempt} 回目）: ${shortReason(error.message)}`);
      await sleep(RETRY_WAIT_MS * attempt);
    }
  }
}

// R2 の操作を、wrangler が失敗したら待ってやり直す。やり直しても失敗したら、投げずに false を返す。
async function r2(action, photo, ...extra) {
  const key = `${BUCKET}/photos/${workspaceId}/${photo.recipeId}/${photo.id}`;
  if (!Object.hasOwn(R2_ACTIONS, action)) {
    throw new Error(`R2 の操作 ${action} は使わない`);
  }
  const what = `写真 ${photo.file} の${R2_ACTIONS[action]}`;
  try {
    await wranglerWithRetry(what, "r2", "object", action, key, ...extra);
    return true;
  } catch (error) {
    warn(`${what}に失敗した: ${error.message}`);
    return false;
  }
}

// 写真の R2 のキー。表示するときは、デモの ID を伏せる（手で消すときは、伏せた所にデモの ID を入れる）。
function shownKey(photo) {
  return `${BUCKET}/photos/***/${photo.recipeId}/${photo.id}`;
}

// photos を UPLOAD_CONCURRENCY 個まで同時に処理し、task が false を返した写真を返す。
async function eachPhoto(photos, task) {
  const failed = [];
  let next = 0;
  async function worker() {
    while (next < photos.length) {
      const photo = photos[next];
      next += 1;
      if (!(await task(photo))) {
        failed.push(photo);
      }
    }
  }
  await Promise.all(Array.from({ length: UPLOAD_CONCURRENCY[target] }, worker));
  return failed;
}

// SQL の IN に並べる、写真の ID の一覧。ID は seed が作った 16 進数なので、そのまま埋め込める。
function photoIdList(photos) {
  return photos.map((photo) => `'${photo.id}'`).join(", ");
}

// 写真のうち、D1 に行が在るものの ID を返す。
async function existingPhotoIds(photos) {
  if (photos.length === 0) {
    return new Set();
  }
  const ids = photoIdList(photos);
  const sql = `SELECT id FROM photos WHERE workspace_id = '${workspaceId}' AND id IN (${ids})`;
  // やり直すのは wrangler の失敗だけで、出力を読めない失敗はやり直さない。
  const output = await wranglerWithRetry("写真の行の読み取り", "d1", "execute", "DB", "--json", "--command", sql);
  // 形が思ったものと違うときは、行が無いとみなさずに失敗にする（行が無いとみなすと、写真を入れずに行だけが残る）。
  const results = JSON.parse(output).at(-1)?.results;
  if (!Array.isArray(results)) {
    throw new Error("wrangler の d1 execute --json の出力の形が、思ったものと違う");
  }
  return new Set(results.map((row) => row.id));
}

async function deletePhotoRows(photos) {
  if (photos.length === 0) {
    return;
  }
  const ids = photoIdList(photos);
  await wranglerWithRetry("写真の行の削除", "d1", "execute", "DB", "--command", `DELETE FROM photos WHERE workspace_id = '${workspaceId}' AND id IN (${ids})`);
}

// 写真の行を消す。失敗しても投げず、消せたかを返す。
async function tryDeletePhotoRows(photos) {
  try {
    await deletePhotoRows(photos);
    return true;
  } catch (error) {
    warn(`写真の行を消せなかった（R2 のデータが無い行が残る。もう一度 seed を実行すれば消える）: ${error.message}`);
    return false;
  }
}

// seed が入れた写真を、アプリの削除と同じく R2・D1 の順に消す。失敗しても投げず、すべて片付いたかを返す。
// R2 から消せなかった写真は行を残す（データが残っていても、行からたどれる）。行も無ければ、手で消せるようキーを表示する。
async function removePhotos(photos) {
  if (photos.length === 0) {
    return true;
  }
  const notDeleted = new Set(await eachPhoto(photos, (photo) => r2("delete", photo)));
  let clean = notDeleted.size === 0;
  if (!(await tryDeletePhotoRows(photos.filter((photo) => !notDeleted.has(photo))))) {
    clean = false;
  }
  if (notDeleted.size > 0) {
    let rows = new Set();
    try {
      rows = await existingPhotoIds([...notDeleted]);
    } catch (error) {
      warn(`写真の行を読めなかったので、R2 から消せなかったデータのキーをすべて表示する: ${error.message}`);
    }
    for (const photo of notDeleted) {
      if (!rows.has(photo.id)) {
        // キーはレシピと写真の ID を見せる必要があるので、redact を通さない（デモの ID は shownKey が伏せている）。
        console.error(`どの行からもたどれないデータが R2 に残った。手で消す: ${shownKey(photo)}`);
      }
    }
  }
  return clean;
}

async function main() {
  const { recipes, photoBytes } = loadRecipes();
  const photoCount = recipes.reduce((sum, recipe) => sum + recipe.files.length, 0);
  console.log(`${target === "--local" ? "ローカル" : "本番"}のデモに、レシピ ${recipes.length} 件と写真 ${photoCount} 枚を入れる`);

  // 消し始める前に、wrangler で次を確かめる。間違えてほかの人のワークスペースの ID を渡しても消さないためと、
  // 写真を入れるのに要る D1 と R2 の権限を先に確かめるためである（写真を入れるときに分かると、写真の無いデモが残る）。
  // - ID が、印（pinned）の付いたワークスペースのものか
  // - 入れ直した後の写真の合計が、アプリ全体の上限を超えないか（今のデモの写真は、入れ直すときに消える）
  // - R2 に書けて、消せるか
  const status = JSON.parse(
    await wrangler(
      "d1",
      "execute",
      "DB",
      "--json",
      "--command",
      `SELECT w.pinned AS pinned, ` +
        `(SELECT COALESCE(SUM(size), 0) FROM photos) AS total, ` +
        `(SELECT COALESCE(SUM(size), 0) FROM photos WHERE workspace_id = '${workspaceId}') AS demo ` +
        `FROM workspaces w WHERE w.id = '${workspaceId}'`,
    ),
  );
  const row = status.at(-1)?.results?.[0];
  if (row === undefined) {
    fail("その ID のワークスペースが無い");
  }
  // D1（SQLite）は BOOLEAN を 0 と 1 で返す。
  if (row.pinned !== 1) {
    fail("そのワークスペースには、自動削除と削除から外す印が無い。デモなら、先に mise run pin で印を付ける");
  }
  if (row.total - row.demo + photoBytes > MAX_TOTAL_PHOTO_BYTES) {
    fail(`入れ直すと、アプリ全体の写真の合計が上限（${MAX_TOTAL_PHOTO_BYTES} バイト）を超える`);
  }
  // 確かめるためだけのデータなので、すぐに消す。キーはデモのワークスペースの下に置き、seed の写真と重ならない名前にする。
  const probe = { file: "（R2 の権限の確認）", recipeId: "seed-check", id: randomBytes(16).toString("hex") };
  const probeFile = join(tmpdir(), `recipe-box-seed-${probe.id}`);
  writeFileSync(probeFile, "seed");
  // 書けないと報告されても書けていることがあるので、書いた結果に関わらず消す（r2 は失敗を投げずに false を返す）。
  const written = await r2("put", probe, "--file", probeFile);
  const removed = await r2("delete", probe);
  rmSync(probeFile, { force: true });
  if (!removed) {
    // キーの ID を見せる必要があるので、redact を通さない（デモの ID は shownKey が伏せている）。
    console.error(`確かめるために書いたデータを R2 から消せなかった。手で消す: ${shownKey(probe)}`);
  }
  if (!written || !removed) {
    fail("R2 に書けないか、消せない。wrangler の認証と API トークンの権限を確かめる");
  }

  // 2. 今のレシピを全部消す。消すと一覧が詰まるので、1 ページ目を読み直しながら消す。
  // 消したはずのレシピが一覧に出続ける（食い違いがある）ときに終わらなくならないよう、前の回りまでに消そうとした ID が
  // また一覧に出たら止める。誰かがレシピを足し続けても終わるよう、回りの数にも上限を付ける（80 件は 20 件ずつで 4 回り）。
  let deleted = 0;
  const attempted = new Set();
  for (let round = 0; ; round += 1) {
    const response = await api("GET", "/api/recipes?page=1");
    await expectStatus(response, 200, "レシピの一覧の取得");
    const { recipes: page } = await response.json();
    if (page.length === 0) {
      break;
    }
    if (round >= MAX_DELETE_ROUNDS) {
      fail("レシピを消し終わらない（入れ直している間に、誰かがレシピを足し続けているかもしれない）");
    }
    if (page.some((recipe) => attempted.has(recipe.id))) {
      fail("レシピを消しても一覧から無くならない");
    }
    for (const recipe of page) {
      attempted.add(recipe.id);
      if (!ID_PATTERN.test(recipe.id)) {
        fail("一覧の ID の形が正しくない");
      }
      const response = await api("DELETE", `/api/recipes/${recipe.id}`);
      // 同時に誰かが消していれば 404 になるが、消えていればよい。
      if (response.status === 404) {
        await response.body?.cancel();
      } else {
        await expectStatus(response, 204, "レシピの削除");
        deleted += 1;
      }
    }
  }
  console.log(`今のレシピ ${deleted} 件を消した`);

  // 3. レシピを入れる。一覧は作った日時の新しい順なので、recipes.json の先頭が一覧の先頭に来るよう、後ろから入れる。
  const photos = [];
  for (const recipe of [...recipes].reverse()) {
    const response = await api("POST", "/api/recipes", recipe.body);
    await expectStatus(response, 201, `レシピ「${recipe.body.name}」の登録`);
    const { id } = await response.json();
    if (!ID_PATTERN.test(id)) {
      fail("登録したレシピの ID の形が正しくない");
    }
    for (const file of recipe.files) {
      photos.push({ ...file, recipeId: id, id: randomBytes(16).toString("hex") });
    }
  }
  console.log(`レシピ ${recipes.length} 件を入れた`);

  // 4. 写真の D1 の行を入れてから、R2 にデータを書く（アプリの写真の追加と同じ順）。
  //    デモは誰でも書き込めるので、入れている間にレシピが消されることがある。そのレシピの写真は入れない。
  //    それ以外の失敗では、入れた写真を消してから止まる（もう一度実行すれば入れ直せる）。
  let inserted = [];
  let lost = 0;
  if (photos.length > 0) {
    try {
      const now = Date.now();
      // レシピが在るときだけ行を入れる（消されていても、ほかの写真の行は入れる）。
      // 値は、形を確かめた ID・数・決まった形式の文字列だけなので、SQL に埋め込める。
      const statements = photos.map(
        (photo, index) =>
          "INSERT INTO photos (workspace_id, recipe_id, id, content_type, size, created_at) " +
          `SELECT '${workspaceId}', '${photo.recipeId}', '${photo.id}', '${photo.contentType}', ${photo.size}, ${now + index} ` +
          `FROM recipes WHERE workspace_id = '${workspaceId}' AND id = '${photo.recipeId}'`,
      );
      for (let i = 0; i < statements.length; i += INSERT_BATCH) {
        await wrangler("d1", "execute", "DB", "--command", statements.slice(i, i + INSERT_BATCH).join("; "));
      }
      const rows = await existingPhotoIds(photos);
      inserted = photos.filter((photo) => rows.has(photo.id));
      lost += photos.length - inserted.length;
    } catch (error) {
      // R2 にはまだ書いていないので、行だけを消す。
      warn(error.message);
      fail(
        (await tryDeletePhotoRows(photos))
          ? "写真の行を入れられなかったので、入れた行を消した。もう一度実行する"
          : "写真の行を入れられず、入れた行も消せなかった。もう一度実行する",
      );
    }
    // r2 は失敗を投げずに false を返す。
    const failed = await eachPhoto(inserted, (photo) => r2("put", photo, "--file", photo.path, "--content-type", photo.contentType));
    if (failed.length > 0) {
      fail(
        (await removePhotos(inserted))
          ? `写真 ${failed.length} 枚を R2 に書けなかったので、入れた写真を消した。もう一度実行する`
          : `写真 ${failed.length} 枚を R2 に書けず、入れた写真も一部を消せなかった（上の表示を見る）。もう一度実行する`,
      );
    }
    // R2 に書いている間にレシピが消されると、行は消えたのに R2 のデータが残る。
    // アプリの写真の追加と同じく、書いた後に行が在るかを確かめ、無ければ R2 のデータを消す。
    // やり直しても行を読めなければ、どれが消されたかを決められないので、ほかの失敗と同じく、書いた写真をすべて消して止まる。
    let after;
    try {
      after = await existingPhotoIds(inserted);
    } catch (error) {
      warn(error.message);
      fail(
        (await removePhotos(inserted))
          ? "写真を書いた後に行を読めなかったので、入れた写真を消した。もう一度実行する"
          : "写真を書いた後に行を読めず、入れた写真も一部を消せなかった（上の表示を見る）。もう一度実行する",
      );
    }
    const gone = inserted.filter((photo) => !after.has(photo.id));
    if (!(await removePhotos(gone))) {
      fail("消されたレシピの写真を片付けられなかった（上の表示を見る）");
    }
    inserted = inserted.filter((photo) => after.has(photo.id));
    lost += gone.length;
    console.log(`写真 ${inserted.length} 枚を入れた`);
  }
  if (lost > 0) {
    fail(`入れている間にレシピが消され、写真 ${lost} 枚を入れられなかった。もう一度実行する`);
  }

  // 5. 公開 API で確かめる。
  const list = await api("GET", "/api/recipes?page=1");
  await expectStatus(list, 200, "レシピの一覧の取得");
  const { total } = await list.json();
  if (total !== recipes.length) {
    fail(`レシピの数が ${total} 件で、入れた ${recipes.length} 件と違う（入れている間に誰かが書き込んだかもしれない）`);
  }
  for (const photo of inserted) {
    const response = await api("GET", `/api/recipes/${photo.recipeId}/photos/${photo.id}`);
    await expectStatus(response, 200, `写真 ${photo.file} の取得`);
    const bytes = Buffer.from(await response.arrayBuffer());
    if (!bytes.equals(readFileSync(photo.path))) {
      fail(`写真 ${photo.file} の中身が違う`);
    }
  }
  console.log("公開 API で、レシピの数と写真の中身を確かめた");
}

main().catch((error) => fail(error.message));
