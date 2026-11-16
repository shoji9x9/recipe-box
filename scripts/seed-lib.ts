// scripts/seed.ts の、外部（API・wrangler・標準入力）に触れない部分。単体テスト（seed-lib.test.ts）で確かめる。
import { closeSync, lstatSync, openSync, readSync, type Stats } from "node:fs";
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

/** ログで置き換えるパスの正規表現と、置き換える文字（pathPattern で作る）。 */
export type PathRule = [pattern: RegExp, replacement: string];

export type PhotoContentType = "image/jpeg" | "image/png" | "image/webp";

/** API に送るレシピの本文。 */
export type RecipeBody = { name: string; servings: number; ingredients: string; steps: string };

/** 確かめた写真のファイル。path は読むためのパス、file は seed/photos/ の下の名前。 */
export type PhotoFile = { file: string; path: string; size: number; contentType: PhotoContentType };

/** 確かめたレシピ。 */
export type SeedRecipe = { body: RecipeBody; files: PhotoFile[] };

// ログで . と ~ に置き換える、リポジトリとホームのディレクトリのパス（端末の利用者名が入る）を、正規表現にする。
// 後ろがファイル名の文字（英数字・_・-）でないときだけ置き換え、/home/u で /home/u2 を崩さない。
// 前は問わない（/mnt/home/u のように長いパスの終わりに出ても伏せる。表示が少し崩れても、利用者名を出さないことを優先する）。
// 絶対パスでないか / だけなら置き換えない（ほかの文字まで置き換えてしまうため）。
export function pathPattern(path: string): RegExp | null {
  const trimmed = path.replace(/\/+$/, "");
  if (!trimmed.startsWith("/")) {
    return null;
  }
  const escaped = trimmed.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(String.raw`${escaped}(?![\w-])`, "g");
}

// 表示やエラーの文から、デモの ID を伏せる。paths は pathPattern で作った置き換えの一覧。
// アカウントの ID（16 進数 32 文字）や D1 のデータベースの ID（UUID の形）も、公開の Actions のログに出さないよう伏せる。
// 16 進数と - が 32 文字以上並んだ所はすべて伏せるので、レシピや写真の ID も伏せる（表示したいキーは seed.ts の shownKey で作る）。
// 文字列でない値（Error など）も、文字列にしてから伏せる。
export function redact(text: unknown, paths: readonly PathRule[]): string {
  // 16 進数と - が 32 文字以上並んだ所を、まとめて 1 つの置き換えで伏せる。デモの ID・アカウントの ID（16 進数 32 文字）・
  // D1 のデータベースの ID（UUID の形、36 文字）が、続けて並んでも伏せられる（置き換えを分けると、順によって一部が残る）。
  return paths
    .reduce((acc, [pattern, replacement]) => acc.replace(pattern, replacement), String(text))
    .replace(/[0-9a-f-]{32,}/gi, "***");
}

// wrangler の失敗の文から、理由を短く選ぶ。wrangler は理由を JSON の "text" か「[ERROR]」の行に書き、
// API のエラーでは、原因を [code: ...] の付いた行（JSON では notes の "text"）に書く。
// どれも無ければ、1 行目（wrangler が失敗した、と終了コードなど）と wrangler の出力の最初の行を並べる。
export function shortReason(message: string): string {
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
export function photoContentType(bytes: Buffer): PhotoContentType | null {
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
function readHead(path: string): Buffer {
  const fd = openSync(path, "r");
  try {
    const head = Buffer.alloc(12);
    return head.subarray(0, readSync(fd, head, 0, head.length, 0));
  } finally {
    closeSync(fd);
  }
}

function length(text: string): number {
  return [...text].length;
}

// seed のデータ（recipes.json と写真）が決まりに合わないか、読めないときの例外。
// validateRecipes は、これにだけ「何件目のレシピか」を添え、ほかの例外（プログラムの誤りなど）はデータの誤りに見せずにそのまま投げる。
export class SeedDataError extends Error {
  override name = "SeedDataError";
}

// lstatWithoutLinks が、path をたどれないときに投げる例外。reason は、base の外（"outside"）か、リンクがある（"link"）か。
export class PathCheckError extends Error {
  override name = "PathCheckError";
  readonly reason: "outside" | "link";

  constructor(message: string, reason: "outside" | "link") {
    super(message);
    this.reason = reason;
  }
}

// path を、base（リポジトリのルート）から 1 階層ずつ lstat でたどり、どこにもシンボリックリンクが無ければ、
// path の lstat の結果を返す（リポジトリの外のファイルを読まないため）。lstat はリンクを辿らないので、外には触れない。
// base の外かリンクがあれば、PathCheckError を投げる。lstat の失敗（ENOENT など）はそのまま投げる。
export function lstatWithoutLinks(path: string, base: string): Stats {
  const rel = relative(base, path);
  if (rel === "" || rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel)) {
    throw new PathCheckError(`${path} が ${base} の下に無い`, "outside");
  }
  let current = base;
  let stat: Stats | undefined;
  for (const part of rel.split(sep)) {
    current = join(current, part);
    stat = lstatSync(current);
    if (stat.isSymbolicLink()) {
      throw new PathCheckError(`${current} がシンボリックリンク`, "link");
    }
  }
  // rel は空でないので、少なくとも 1 回は lstat している。
  return stat as Stats;
}

// fs のシステムの失敗（ENOENT・EACCES など。syscall を持つ）なら、shown を読めないという SeedDataError にして投げる。
// それ以外（プログラムの誤り）は、データの誤りに見せずにそのまま投げる。
function throwAsReadFailure(error: unknown, shown: string): never {
  if (!(error instanceof Error) || (error as NodeJS.ErrnoException).syscall === undefined) {
    throw error;
  }
  throw new SeedDataError(`${shown} を読めない（${(error as NodeJS.ErrnoException).code}）`);
}

// path が、base の下にリンクを通らずに置かれた、ふつうのファイルかを確かめ、lstat の結果を返す。
// ハードリンクも、リポジトリの外のファイルと中身を共有しうるので断る（git はハードリンクを記録しない）。
// FIFO などは読むと止まるので、ふつうのファイルだけを受け付ける。
// 合わなければ、shown（表示する名前）を使った文の SeedDataError を投げる。fs の失敗でない例外（プログラムの誤り）は、そのまま投げる。
export function regularFileWithoutLinks(path: string, base: string, shown: string): Stats {
  let stat: Stats;
  try {
    stat = lstatWithoutLinks(path, base);
  } catch (error) {
    if (error instanceof PathCheckError && error.reason === "link") {
      throw new SeedDataError(`${shown} か、その上のディレクトリがシンボリックリンクになっている`);
    }
    if (error instanceof PathCheckError && error.reason === "outside") {
      throw new SeedDataError(`${shown} がリポジトリの下に無い`);
    }
    throwAsReadFailure(error, shown);
  }
  if (!stat.isFile()) {
    throw new SeedDataError(`${shown} がふつうのファイルでない`);
  }
  if (stat.nlink > 1) {
    throw new SeedDataError(`${shown} がハードリンクになっている`);
  }
  return stat;
}

// seed/recipes.json の中身（JSON を読んだ値）を、API と同じ決まりで確かめる。写真は photoDir の下から探す。
// 写真は、base（リポジトリのルート）の下にリンクを通らずにあるものだけを受け付ける。
// 決まりに合わないか読めなければ、理由を書いた SeedDataError を投げる。ほかの例外は、そのまま投げる。
export function validateRecipes(
  recipes: unknown,
  photoDir: string,
  base: string,
): { recipes: SeedRecipe[]; photoBytes: number } {
  if (!Array.isArray(recipes) || recipes.length === 0 || recipes.length > MAX_RECIPES) {
    throw new SeedDataError(`seed/recipes.json は 1 件から ${MAX_RECIPES} 件までの配列にする`);
  }
  const used = new Set<string>();
  const parsed = recipes.map((recipe: unknown, index): SeedRecipe => {
    const where = `seed/recipes.json の ${index + 1} 件目`;
    // JSON の値なので、どの項目も形を確かめるまでは unknown として扱う。
    const {
      name,
      servings,
      ingredients = "",
      steps = "",
      photos = [],
    } = (recipe ?? {}) as Record<string, unknown>;
    if (typeof name !== "string" || name.trim() === "" || length(name) > MAX_NAME) {
      throw new SeedDataError(`${where}: name は空でない ${MAX_NAME} 文字までの文字列にする`);
    }
    if (typeof servings !== "number" || !Number.isInteger(servings) || servings < 1 || servings > MAX_SERVINGS) {
      throw new SeedDataError(`${where}: servings は 1 から ${MAX_SERVINGS} までの整数にする`);
    }
    if (typeof ingredients !== "string" || length(ingredients) > MAX_INGREDIENTS) {
      throw new SeedDataError(`${where}: ingredients は ${MAX_INGREDIENTS} 文字までの文字列にする`);
    }
    if (typeof steps !== "string" || length(steps) > MAX_STEPS) {
      throw new SeedDataError(`${where}: steps は ${MAX_STEPS} 文字までの文字列にする`);
    }
    if (!Array.isArray(photos) || photos.length > MAX_PHOTOS_PER_RECIPE) {
      throw new SeedDataError(`${where}: photos は ${MAX_PHOTOS_PER_RECIPE} 枚までの配列にする`);
    }
    const files = photos.map((file: unknown): PhotoFile => {
      // ファイル名の形を決め、seed/photos/ の外を読ませない。
      if (typeof file !== "string" || !PHOTO_FILE_PATTERN.test(file)) {
        throw new SeedDataError(
          `${where}: 写真のファイル名 ${JSON.stringify(file)} の形が正しくない（英小文字・数字・ハイフンと拡張子）`,
        );
      }
      if (used.has(file)) {
        throw new SeedDataError(`${where}: 写真 ${file} を 2 回使っている`);
      }
      used.add(file);
      const path = join(photoDir, file);
      let stat: Stats;
      try {
        stat = regularFileWithoutLinks(path, base, `写真 seed/photos/${file}`);
      } catch (error) {
        if (!(error instanceof SeedDataError)) {
          throw error;
        }
        throw new SeedDataError(`${where}: ${error.message}`);
      }
      const { size } = stat;
      if (size === 0 || size > MAX_PHOTO_BYTES) {
        throw new SeedDataError(`${where}: 写真 ${file} は 1 バイトから ${MAX_PHOTO_BYTES} バイトまでにする`);
      }
      let head: Buffer;
      try {
        head = readHead(path);
      } catch (error) {
        throwAsReadFailure(error, `${where}: 写真 seed/photos/${file}`);
      }
      const contentType = photoContentType(head);
      if (contentType === null) {
        throw new SeedDataError(`${where}: 写真 ${file} が JPEG・PNG・WebP のどれでもない`);
      }
      return { file, path, size, contentType };
    });
    return { body: { name, servings, ingredients, steps }, files };
  });
  const total = parsed.flatMap((recipe) => recipe.files).reduce((sum, file) => sum + file.size, 0);
  if (total > MAX_WORKSPACE_PHOTO_BYTES) {
    throw new SeedDataError(
      `写真の合計が ${total} バイトで、ワークスペースの上限（${MAX_WORKSPACE_PHOTO_BYTES} バイト）を超える`,
    );
  }
  return { recipes: parsed, photoBytes: total };
}
