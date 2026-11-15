// scripts/seed.mjs の、外部（API・wrangler・標準入力）に触れない部分。単体テスト（seed-lib.test.mjs）で確かめる。
import { closeSync, lstatSync, openSync, readSync } from "node:fs";
import { isAbsolute, join, relative, sep } from "node:path";

// API の決まり（docs/spec.md）。seed のデータが API に断られないよう、入れる前に同じ決まりで確かめる。
// 値はアプリ（apps/recipe-box-rs/src/recipe.rs・photo.rs）と同じにする。アプリの値を変えたら、ここも直す。
export const MAX_RECIPES = 80;
export const MAX_PHOTOS_PER_RECIPE = 5;
export const MAX_PHOTO_BYTES = 4_000_000;
// seed は写真の上限を判定する API を通らずに入れるので、ワークスペースの合計の上限も seed で確かめる。
export const MAX_WORKSPACE_PHOTO_BYTES = 100_000_000;
export const MAX_NAME = 100;
export const MAX_SERVINGS = 100;
export const MAX_INGREDIENTS = 2_000;
export const MAX_STEPS = 5_000;
// やり直すときのログに出す理由の長さ。
const REASON_LENGTH = 300;

const PHOTO_FILE_PATTERN = /^[a-z0-9][a-z0-9-]*\.(jpg|png|webp)$/;

// ログで . と ~ に置き換える、リポジトリとホームのディレクトリのパス（端末の利用者名が入る）を、正規表現にする。
// 後ろがファイル名の文字（英数字・_・-）でないときだけ置き換え、/home/u で /home/u2 を崩さない。
// 前は問わない（/mnt/home/u のように長いパスの終わりに出ても伏せる。表示が少し崩れても、利用者名を出さないことを優先する）。
// 絶対パスでないか / だけなら置き換えない（ほかの文字まで置き換えてしまうため）。
export function pathPattern(path) {
  const trimmed = path.replace(/\/+$/, "");
  if (!trimmed.startsWith("/")) {
    return null;
  }
  const escaped = trimmed.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(String.raw`${escaped}(?![\w-])`, "g");
}

// 表示やエラーの文から、デモの ID を伏せる。paths は [正規表現, 置き換える文字] の配列（pathPattern で作る）。
// アカウントの ID（16 進数 32 文字）や D1 のデータベースの ID（UUID の形）も、公開の Actions のログに出さないよう伏せる。
// 16 進数と - が 32 文字以上並んだ所はすべて伏せるので、レシピや写真の ID も伏せる（表示したいキーは seed.mjs の shownKey で作る）。
export function redact(text, paths) {
  // 16 進数と - が 32 文字以上並んだ所を、まとめて 1 つの置き換えで伏せる。デモの ID・アカウントの ID（16 進数 32 文字）・
  // D1 のデータベースの ID（UUID の形、36 文字）が、続けて並んでも伏せられる（置き換えを分けると、順によって一部が残る）。
  return paths
    .reduce((acc, [pattern, replacement]) => acc.replace(pattern, replacement), String(text))
    .replace(/[0-9a-f-]{32,}/gi, "***");
}

// wrangler の失敗の文から、理由を短く選ぶ。wrangler は理由を JSON の "text" か「[ERROR]」の行に書き、
// API のエラーでは、原因を [code: ...] の付いた行（JSON では notes の "text"）に書く。
// どれも無ければ、1 行目（wrangler が失敗した、と終了コードなど）と wrangler の出力の最初の行を並べる。
export function shortReason(message) {
  const lines = message
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
  const found = lines.find((l) => l.includes('"text"')) ?? lines.find((l) => l.includes("[ERROR]"));
  const used = found === undefined ? lines.slice(0, 2) : [found];
  const main = used.join(" ");
  const code = lines.find((l) => !used.includes(l) && l.includes("[code:"));
  return (code === undefined ? main : `${main} ${code}`).slice(0, REASON_LENGTH);
}

// 中身の先頭のバイトで形式を決める（アプリと同じ）。
export function photoContentType(bytes) {
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

// 形式を決めるのに要る、ファイルの先頭の 12 バイトだけを読む。
function readHead(path) {
  const fd = openSync(path, "r");
  try {
    const head = Buffer.alloc(12);
    return head.subarray(0, readSync(fd, head, 0, head.length, 0));
  } finally {
    closeSync(fd);
  }
}

function length(text) {
  return [...text].length;
}

// path を、base（リポジトリのルート）から 1 階層ずつ lstat でたどり、どこにもシンボリックリンクが無ければ、
// path の lstat の結果を返す（リポジトリの外のファイルを読まないため）。lstat はリンクを辿らないので、外には触れない。
// base の外なら "outside"、リンクがあれば "link" を reason に持つ Error を投げる。lstat の失敗（ENOENT など）はそのまま投げる。
export function lstatWithoutLinks(path, base) {
  const rel = relative(base, path);
  if (rel === "" || rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel)) {
    throw Object.assign(new Error(`${path} が ${base} の下に無い`), { reason: "outside" });
  }
  let current = base;
  let stat;
  for (const part of rel.split(sep)) {
    current = join(current, part);
    stat = lstatSync(current);
    if (stat.isSymbolicLink()) {
      throw Object.assign(new Error(`${current} がシンボリックリンク`), { reason: "link" });
    }
  }
  return stat;
}

// lstatWithoutLinks の失敗を、shown（表示する名前）を使った文にする。
export function linkErrorMessage(error, shown) {
  if (error.reason === "link") {
    return `${shown} か、その上のディレクトリがシンボリックリンクになっている`;
  }
  if (error.reason === "outside") {
    return `${shown} がリポジトリの下に無い`;
  }
  if (error.code === undefined) {
    // fs の失敗でない（プログラムの誤りなど）なら、データの問題に見せず、そのまま投げる。
    throw error;
  }
  return `${shown} を読めない（${error.code}）`;
}

// seed/recipes.json の中身（JSON を読んだ値）を、API と同じ決まりで確かめる。写真は photoDir の下から探す。
// 写真は、base（リポジトリのルート）の下にリンクを通らずにあるものだけを受け付ける。
// 決まりに合わなければ、理由を書いた Error を投げる。
export function validateRecipes(recipes, photoDir, base) {
  if (!Array.isArray(recipes) || recipes.length === 0 || recipes.length > MAX_RECIPES) {
    throw new Error(`seed/recipes.json は 1 件から ${MAX_RECIPES} 件までの配列にする`);
  }
  const used = new Set();
  const parsed = recipes.map((recipe, index) => {
    const where = `seed/recipes.json の ${index + 1} 件目`;
    const { name, servings, ingredients = "", steps = "", photos = [] } = recipe ?? {};
    if (typeof name !== "string" || name.trim() === "" || length(name) > MAX_NAME) {
      throw new Error(`${where}: name は空でない ${MAX_NAME} 文字までの文字列にする`);
    }
    if (!Number.isInteger(servings) || servings < 1 || servings > MAX_SERVINGS) {
      throw new Error(`${where}: servings は 1 から ${MAX_SERVINGS} までの整数にする`);
    }
    if (typeof ingredients !== "string" || length(ingredients) > MAX_INGREDIENTS) {
      throw new Error(`${where}: ingredients は ${MAX_INGREDIENTS} 文字までの文字列にする`);
    }
    if (typeof steps !== "string" || length(steps) > MAX_STEPS) {
      throw new Error(`${where}: steps は ${MAX_STEPS} 文字までの文字列にする`);
    }
    if (!Array.isArray(photos) || photos.length > MAX_PHOTOS_PER_RECIPE) {
      throw new Error(`${where}: photos は ${MAX_PHOTOS_PER_RECIPE} 枚までの配列にする`);
    }
    const files = photos.map((file) => {
      // ファイル名の形を決め、seed/photos/ の外を読ませない。
      if (typeof file !== "string" || !PHOTO_FILE_PATTERN.test(file)) {
        throw new Error(
          `${where}: 写真のファイル名 ${JSON.stringify(file)} の形が正しくない（英小文字・数字・ハイフンと拡張子）`,
        );
      }
      if (used.has(file)) {
        throw new Error(`${where}: 写真 ${file} を 2 回使っている`);
      }
      used.add(file);
      const path = join(photoDir, file);
      let stat;
      try {
        stat = lstatWithoutLinks(path, base);
      } catch (error) {
        throw new Error(`${where}: ${linkErrorMessage(error, `写真 seed/photos/${file}`)}`);
      }
      // ディレクトリなどは、先頭を読むときに理由の分からない失敗になるので、先に断る。
      if (!stat.isFile()) {
        throw new Error(`${where}: 写真 seed/photos/${file} がふつうのファイルでない`);
      }
      // ハードリンクは、リポジトリの外のファイルと中身を共有しうるので断る（git はハードリンクを記録しない）。
      if (stat.nlink > 1) {
        throw new Error(`${where}: 写真 seed/photos/${file} がハードリンクになっている`);
      }
      const { size } = stat;
      if (size === 0 || size > MAX_PHOTO_BYTES) {
        throw new Error(`${where}: 写真 ${file} は 1 バイトから ${MAX_PHOTO_BYTES} バイトまでにする`);
      }
      const contentType = photoContentType(readHead(path));
      if (contentType === null) {
        throw new Error(`${where}: 写真 ${file} が JPEG・PNG・WebP のどれでもない`);
      }
      return { file, path, size, contentType };
    });
    return { body: { name, servings, ingredients, steps }, files };
  });
  const total = parsed.flatMap((recipe) => recipe.files).reduce((sum, file) => sum + file.size, 0);
  if (total > MAX_WORKSPACE_PHOTO_BYTES) {
    throw new Error(
      `写真の合計が ${total} バイトで、ワークスペースの上限（${MAX_WORKSPACE_PHOTO_BYTES} バイト）を超える`,
    );
  }
  return { recipes: parsed, photoBytes: total };
}
