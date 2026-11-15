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
//      ID が、印（mise run pin）の付いたワークスペースのものかを、wrangler で D1 を読んで確かめる。
//   2. 公開 API で、デモのレシピを全部消す（レシピを消すと、その写真も消える）。
//   3. 公開 API で、サンプルのレシピを入れる。
//   4. 写真は、デモでは API で追加できないので、wrangler で D1 の行と R2 のデータを直接入れる。
//      アプリと同じく D1 の行を先に入れ、R2 に書けなかった写真の行は消す。書いた後に行が消えていれば（レシピが消された）、
//      R2 のデータも消す。どの行からもたどれないデータを R2 に残さないためである。
//   5. 公開 API で、レシピの数と、写真をすべて読めることを確かめる。
// 何度実行しても同じ状態になる。途中で失敗したら、もう一度実行すれば入れ直せる。
import { execFile } from "node:child_process";
import { randomBytes } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const run = promisify(execFile);
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const seedDir = join(root, "seed");
const photoDir = join(seedDir, "photos");
const appDir = join(root, "apps", "recipe-box-rs");
// 写真 1 枚ごとに pnpm を起こさないよう、pnpm install が入れた wrangler を直接動かす。
const wranglerBin = join(root, "node_modules", ".bin", "wrangler");

const BASE_URLS = {
  "--local": "http://localhost:8787",
  "--remote": "https://recipe-box.shoji9x9.workers.dev",
};
const BUCKET = "recipe-box-photos";
// API の決まり（docs/spec.md）。seed のデータが API に断られないよう、入れる前に同じ決まりで確かめる。
const MAX_RECIPES = 80;
const MAX_PHOTOS_PER_RECIPE = 5;
const MAX_PHOTO_BYTES = 4_000_000;
// seed は写真の上限を判定する API を通らずに入れるので、ワークスペースの合計の上限も seed で確かめる。
const MAX_WORKSPACE_PHOTO_BYTES = 100_000_000;
const MAX_NAME = 100;
const MAX_INGREDIENTS = 2_000;
const MAX_STEPS = 5_000;
// 書き込む API の回数の制限（60 秒に 30 回）で 429 になったら、枠が空くまで待ってやり直す。
const RATE_LIMIT_WAIT_MS = 61_000;
const MAX_RETRIES = 5;
// 写真の R2 の操作で wrangler を同時に動かす数。ローカルでは、wrangler dev が使う保存先に同時に書くと失敗することがあるので、1 つずつ書く。
const UPLOAD_CONCURRENCY = { "--local": 1, "--remote": 4 };
// R2 への書き込みと削除が失敗したときに試す回数と、やり直す前に待つ時間（回ごとに延ばす）。
const R2_ATTEMPTS = 4;
const R2_RETRY_WAIT_MS = 5_000;

const ID_PATTERN = /^[0-9a-f]{32}$/;
const PHOTO_FILE_PATTERN = /^[a-z0-9][a-z0-9-]*\.(jpg|png|webp)$/;

function fail(message) {
  console.error(`seed: ${message}`);
  process.exit(1);
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

// 表示やエラーの文から、デモの ID を伏せる。
function redact(text) {
  return String(text).replaceAll(workspaceId, "***");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// 中身の先頭のバイトで形式を決める（アプリと同じ）。
function photoContentType(bytes) {
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) {
    return "image/jpeg";
  }
  const png = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
  if (bytes.length >= png.length && png.every((b, i) => bytes[i] === b)) {
    return "image/png";
  }
  if (
    bytes.length >= 12 &&
    bytes.toString("latin1", 0, 4) === "RIFF" &&
    bytes.toString("latin1", 8, 12) === "WEBP"
  ) {
    return "image/webp";
  }
  return null;
}

function length(text) {
  return [...text].length;
}

// seed/recipes.json を読み、API と同じ決まりで確かめる。写真のファイルも読んでおく。
function loadRecipes() {
  let recipes;
  try {
    recipes = JSON.parse(readFileSync(join(seedDir, "recipes.json"), "utf8"));
  } catch (error) {
    fail(`seed/recipes.json を読めない: ${error.message}`);
  }
  if (!Array.isArray(recipes) || recipes.length === 0 || recipes.length > MAX_RECIPES) {
    fail(`seed/recipes.json は 1 件から ${MAX_RECIPES} 件までの配列にする`);
  }
  const used = new Set();
  const parsed = recipes.map((recipe, index) => {
    const where = `seed/recipes.json の ${index + 1} 件目`;
    const { name, servings, ingredients = "", steps = "", photos = [] } = recipe ?? {};
    if (typeof name !== "string" || name.trim() === "" || length(name) > MAX_NAME) {
      fail(`${where}: name は空でない ${MAX_NAME} 文字までの文字列にする`);
    }
    if (!Number.isInteger(servings) || servings < 1 || servings > 100) {
      fail(`${where}: servings は 1 から 100 までの整数にする`);
    }
    if (typeof ingredients !== "string" || length(ingredients) > MAX_INGREDIENTS) {
      fail(`${where}: ingredients は ${MAX_INGREDIENTS} 文字までの文字列にする`);
    }
    if (typeof steps !== "string" || length(steps) > MAX_STEPS) {
      fail(`${where}: steps は ${MAX_STEPS} 文字までの文字列にする`);
    }
    if (!Array.isArray(photos) || photos.length > MAX_PHOTOS_PER_RECIPE) {
      fail(`${where}: photos は ${MAX_PHOTOS_PER_RECIPE} 枚までの配列にする`);
    }
    const files = photos.map((file) => {
      // ファイル名の形を決め、seed/photos/ の外を読ませない。
      if (typeof file !== "string" || !PHOTO_FILE_PATTERN.test(file)) {
        fail(`${where}: 写真のファイル名 ${JSON.stringify(file)} の形が正しくない（英小文字・数字・ハイフンと拡張子）`);
      }
      if (used.has(file)) {
        fail(`${where}: 写真 ${file} を 2 回使っている`);
      }
      used.add(file);
      const path = join(photoDir, file);
      let size;
      try {
        size = statSync(path).size;
      } catch {
        fail(`${where}: 写真 seed/photos/${file} が無い`);
      }
      if (size === 0 || size > MAX_PHOTO_BYTES) {
        fail(`${where}: 写真 ${file} は 1 バイトから ${MAX_PHOTO_BYTES} バイトまでにする`);
      }
      const bytes = readFileSync(path);
      const contentType = photoContentType(bytes);
      if (contentType === null) {
        fail(`${where}: 写真 ${file} が JPEG・PNG・WebP のどれでもない`);
      }
      return { file, path, size, contentType, bytes };
    });
    return { body: { name, servings, ingredients, steps }, files };
  });
  const total = parsed.flatMap((recipe) => recipe.files).reduce((sum, file) => sum + file.size, 0);
  if (total > MAX_WORKSPACE_PHOTO_BYTES) {
    fail(`写真の合計が ${total} バイトで、ワークスペースの上限（${MAX_WORKSPACE_PHOTO_BYTES} バイト）を超える`);
  }
  return parsed;
}

// 公開 API を呼ぶ。書き込みが回数の制限で 429 になったら、待ってやり直す。
async function api(method, path, body) {
  for (let attempt = 0; ; attempt += 1) {
    const response = await fetch(`${baseUrl}${path}`, {
      method,
      headers: {
        "X-Workspace-Id": workspaceId,
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(30_000),
    });
    if (response.status === 429 && attempt < MAX_RETRIES) {
      await response.body?.cancel();
      console.log(`書き込みの回数の制限に当たったので、${RATE_LIMIT_WAIT_MS / 1000} 秒待つ`);
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
      maxBuffer: 16 * 1024 * 1024,
    });
    return stdout;
  } catch (error) {
    throw new Error(redact(`${error.stderr ?? ""}${error.stdout ?? ""}${error.message}`).slice(-2000));
  }
}

// R2 の操作を、一時的な失敗なら待ってやり直す。やり直しても失敗したら false を返す。
async function r2(action, photo, ...extra) {
  const key = `${BUCKET}/photos/${workspaceId}/${photo.recipeId}/${photo.id}`;
  for (let attempt = 1; ; attempt += 1) {
    try {
      await wrangler("r2", "object", action, key, ...extra);
      return true;
    } catch (error) {
      if (attempt >= R2_ATTEMPTS) {
        console.error(`写真 ${photo.file} の R2 の ${action} に失敗した: ${error.message}`);
        return false;
      }
      console.log(`写真 ${photo.file} の R2 の ${action} に失敗したので、やり直す（${attempt} 回目）`);
      await sleep(R2_RETRY_WAIT_MS * attempt);
    }
  }
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

// 写真のうち、D1 に行が在るものの ID を返す。
async function existingPhotoIds(photos) {
  if (photos.length === 0) {
    return new Set();
  }
  const ids = photos.map((photo) => `'${photo.id}'`).join(", ");
  const output = await wrangler(
    "d1",
    "execute",
    "DB",
    "--json",
    "--command",
    `SELECT id FROM photos WHERE workspace_id = '${workspaceId}' AND id IN (${ids})`,
  );
  return new Set((JSON.parse(output).at(-1)?.results ?? []).map((row) => row.id));
}

async function deletePhotoRows(photos) {
  if (photos.length === 0) {
    return;
  }
  const ids = photos.map((photo) => `'${photo.id}'`).join(", ");
  await wrangler("d1", "execute", "DB", "--command", `DELETE FROM photos WHERE workspace_id = '${workspaceId}' AND id IN (${ids})`);
}

async function main() {
  const recipes = loadRecipes();
  const photoCount = recipes.reduce((sum, recipe) => sum + recipe.files.length, 0);
  console.log(`${target === "--local" ? "ローカル" : "本番"}のデモに、レシピ ${recipes.length} 件と写真 ${photoCount} 枚を入れる`);

  // 消し始める前に、ID が印（pinned）の付いたワークスペースかを D1 で確かめる。
  // 間違えてほかの人のワークスペースの ID を渡しても消さないためと、写真を入れる wrangler の認証を先に確かめるためである
  // （認証が無いと分かるのが写真を入れるときだと、写真の無いデモが残る）。
  const status = JSON.parse(
    await wrangler("d1", "execute", "DB", "--json", "--command", `SELECT pinned FROM workspaces WHERE id = '${workspaceId}'`),
  );
  const pinned = status.at(-1)?.results?.[0]?.pinned;
  if (pinned === undefined) {
    fail("その ID のワークスペースが無い");
  }
  // D1（SQLite）は BOOLEAN を 0 と 1 で返す。
  if (pinned !== 1) {
    fail("そのワークスペースには、自動削除と削除から外す印が無い。デモなら、先に mise run pin で印を付ける");
  }

  // 2. 今のレシピを全部消す。消すと一覧が詰まるので、1 ページ目を読み直しながら消す。
  let deleted = 0;
  for (;;) {
    const response = await api("GET", "/api/recipes?page=1");
    await expectStatus(response, 200, "レシピの一覧の取得");
    const { recipes: page } = await response.json();
    if (page.length === 0) {
      break;
    }
    for (const recipe of page) {
      if (!ID_PATTERN.test(recipe.id)) {
        fail("一覧の ID の形が正しくない");
      }
      const response = await api("DELETE", `/api/recipes/${recipe.id}`);
      // 同時に誰かが消していれば 404 になるが、消えていればよい。
      if (response.status !== 404) {
        await expectStatus(response, 204, "レシピの削除");
      }
      deleted += 1;
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
  let inserted = [];
  let lost = 0;
  if (photos.length > 0) {
    const now = Date.now();
    // レシピが在るときだけ行を入れる（消されていても、ほかの写真の行は入れる）。
    // 値は、形を確かめた ID・数・決まった形式の文字列だけなので、SQL に埋め込める。
    const statements = photos.map(
      (photo, index) =>
        "INSERT INTO photos (workspace_id, recipe_id, id, content_type, size, created_at) " +
        `SELECT '${workspaceId}', '${photo.recipeId}', '${photo.id}', '${photo.contentType}', ${photo.size}, ${now + index} ` +
        `FROM recipes WHERE workspace_id = '${workspaceId}' AND id = '${photo.recipeId}'`,
    );
    await wrangler("d1", "execute", "DB", "--command", statements.join("; "));
    const rows = await existingPhotoIds(photos);
    inserted = photos.filter((photo) => rows.has(photo.id));
    lost += photos.length - inserted.length;

    const failed = await eachPhoto(inserted, (photo) => r2("put", photo, "--file", photo.path, "--content-type", photo.contentType));
    if (failed.length > 0) {
      await deletePhotoRows(failed);
      fail(`写真 ${failed.length} 枚を R2 に書けなかった。もう一度実行する`);
    }
    // R2 に書いている間にレシピが消されると、行は消えたのに R2 のデータが残る。
    // アプリの写真の追加と同じく、書いた後に行が在るかを確かめ、無ければ R2 のデータを消す。
    const after = await existingPhotoIds(inserted);
    const gone = inserted.filter((photo) => !after.has(photo.id));
    const notDeleted = await eachPhoto(gone, (photo) => r2("delete", photo));
    if (notDeleted.length > 0) {
      fail(`消されたレシピの写真 ${notDeleted.length} 枚のデータを R2 から消せなかった`);
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
    if (!bytes.equals(photo.bytes)) {
      fail(`写真 ${photo.file} の中身が違う`);
    }
  }
  console.log("公開 API で、レシピの数と写真の中身を確かめた");
}

main().catch((error) => fail(redact(error.message)));
