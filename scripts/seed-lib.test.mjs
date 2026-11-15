// scripts/seed-lib.mjs の単体テスト（mise run test で実行する）。
// 伏せ字は、公開の Actions のログにデモの ID などを出さないための要なので、形ごとに確かめる。
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, truncateSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, test } from "node:test";
import {
  MAX_INGREDIENTS,
  MAX_NAME,
  MAX_PHOTO_BYTES,
  MAX_PHOTOS_PER_RECIPE,
  MAX_RECIPES,
  MAX_SERVINGS,
  MAX_STEPS,
  MAX_WORKSPACE_PHOTO_BYTES,
  pathPattern,
  redact,
  shortReason,
  validateRecipes,
} from "./seed-lib.mjs";

const HEX32 = "0123456789abcdef0123456789abcdef";
const UUID = "01234567-89ab-cdef-0123-456789abcdef";

describe("redact", () => {
  const paths = [
    [pathPattern("/home/user/projects/recipe-box"), "."],
    [pathPattern("/home/user"), "~"],
  ];

  test("16 進数 32 文字・UUID・大文字を伏せる", () => {
    assert.equal(redact(`id ${HEX32} db ${UUID} up ${HEX32.toUpperCase()}`, paths), "id *** db *** up ***");
  });

  test("ID が続けて並んでも、一部を残さない", () => {
    assert.equal(redact(`${HEX32}-${UUID.slice(9)}`, paths), "***");
    assert.equal(redact(`${UUID}${HEX32}`, paths), "***");
  });

  test("31 文字までの 16 進数は伏せない", () => {
    assert.equal(redact(HEX32.slice(1), paths), HEX32.slice(1));
  });

  test("リポジトリとホームのパスを . と ~ に置き換える", () => {
    assert.equal(
      redact("/home/user/projects/recipe-box/seed/x.jpg と /home/user/.config", paths),
      "./seed/x.jpg と ~/.config",
    );
  });

  test("名前が続く別のディレクトリは置き換えない", () => {
    assert.equal(redact("/home/user2/x", paths), "/home/user2/x");
  });

  test("相対パスと / だけのパスからは正規表現を作らない", () => {
    assert.equal(pathPattern("relative/path"), null);
    assert.equal(pathPattern("/"), null);
  });

  test("文字列でない値も文字列にして伏せる", () => {
    assert.equal(redact(new Error(HEX32), []), "Error: ***");
  });
});

describe("shortReason", () => {
  test('JSON の "text" の行を選び、[code: の行を添える', () => {
    const message = 'wrangler が失敗した（終了コード 1）\n{\n  "text": "A request failed"\n}\nnotes [code: 10000]';
    assert.equal(shortReason(message), '"text": "A request failed" notes [code: 10000]');
  });

  test("[ERROR] の行を選ぶ", () => {
    assert.equal(shortReason("wrangler が失敗した\n✘ [ERROR] Not found\n"), "✘ [ERROR] Not found");
  });

  test("どちらも無ければ、最初の 2 行を並べる", () => {
    assert.equal(shortReason("1 行目\n\n2 行目\n3 行目"), "1 行目 2 行目");
  });

  test("300 文字に切り詰める", () => {
    assert.equal(shortReason("x".repeat(1000)).length, 300);
  });
});

describe("validateRecipes", () => {
  // base はリポジトリのルートにあたる一時ディレクトリ、dir はその下の seed/photos/。
  let base;
  let dir;
  const jpeg = Buffer.from([0xff, 0xd8, 0xff, 0xe0]);
  // 先頭だけを書いて、大きさは疎なファイルで決める（ディスクを使わない）。
  function photo(name, size, head = jpeg) {
    const path = join(dir, name);
    writeFileSync(path, head);
    truncateSync(path, size);
    return name;
  }
  const recipe = (overrides = {}) => ({ name: "カレー", servings: 2, ...overrides });
  const validate = (recipes, photoDir = dir) => validateRecipes(recipes, photoDir, base);
  const rejects = (recipes, pattern, photoDir = dir) => assert.throws(() => validate(recipes, photoDir), pattern);

  before(() => {
    // 本物と同じく、<一時ディレクトリ>/seed/photos/ の形にする。
    base = mkdtempSync(join(tmpdir(), "seed-lib-test-"));
    dir = join(base, "seed", "photos");
    mkdirSync(dir, { recursive: true });
  });
  after(() => {
    rmSync(base, { recursive: true, force: true });
  });

  test("決まりどおりのレシピを受け付け、写真の形式と大きさを返す", () => {
    const file = photo("ok.jpg", 10);
    const { recipes, photoBytes } = validate([recipe({ photos: [file] })]);
    assert.deepEqual(recipes[0].body, { name: "カレー", servings: 2, ingredients: "", steps: "" });
    assert.equal(recipes[0].files[0].contentType, "image/jpeg");
    assert.equal(photoBytes, 10);
  });

  test("レシピの件数は 1 件から上限まで", () => {
    rejects([], /1 件から/);
    rejects("x", /1 件から/);
    assert.equal(validate(Array.from({ length: MAX_RECIPES }, () => recipe())).recipes.length, MAX_RECIPES);
    rejects(
      Array.from({ length: MAX_RECIPES + 1 }, () => recipe()),
      /1 件から/,
    );
  });

  test("name は上限の文字数まで（文字の数で数える）", () => {
    validate([recipe({ name: "🍛".repeat(MAX_NAME) })]);
    rejects([recipe({ name: "あ".repeat(MAX_NAME + 1) })], /name/);
    rejects([recipe({ name: "  " })], /name/);
    rejects([recipe({ name: 1 })], /name/);
  });

  test("servings は 1 から上限までの整数", () => {
    validate([recipe({ servings: 1 }), recipe({ servings: MAX_SERVINGS })]);
    for (const servings of [0, MAX_SERVINGS + 1, 1.5, "2"]) {
      rejects([recipe({ servings })], /servings/);
    }
  });

  test("ingredients と steps の長さ", () => {
    validate([recipe({ ingredients: "あ".repeat(MAX_INGREDIENTS), steps: "あ".repeat(MAX_STEPS) })]);
    rejects([recipe({ ingredients: "あ".repeat(MAX_INGREDIENTS + 1) })], /ingredients/);
    rejects([recipe({ steps: "あ".repeat(MAX_STEPS + 1) })], /steps/);
  });

  test("写真は 1 件に上限の枚数まで", () => {
    const files = Array.from({ length: MAX_PHOTOS_PER_RECIPE + 1 }, (_, i) => photo(`n${i}.jpg`, 10));
    validate([recipe({ photos: files.slice(0, MAX_PHOTOS_PER_RECIPE) })]);
    rejects([recipe({ photos: files })], /photos/);
  });

  test("写真・seed/photos/・seed/ のどれかがシンボリックリンクなら断る", () => {
    const file = photo("real.jpg", 10);
    // 写真そのもの
    symlinkSync(join(dir, file), join(dir, "link.jpg"));
    rejects([recipe({ photos: ["link.jpg"] })], /シンボリックリンク/);
    // seed/photos/
    const linkedPhotos = join(base, "seed", "linked-photos");
    symlinkSync(dir, linkedPhotos);
    rejects([recipe({ photos: [file] })], /シンボリックリンク/, linkedPhotos);
    // seed/
    const linkedSeed = join(base, "linked-seed");
    symlinkSync(join(base, "seed"), linkedSeed);
    rejects([recipe({ photos: [file] })], /シンボリックリンク/, join(linkedSeed, "photos"));
    // 写真を使わないレシピだけなら、ディレクトリを読まない
    validate([recipe()], linkedPhotos);
  });

  test("写真のファイル名で、seed/photos/ の外を読ませない", () => {
    for (const file of ["../recipes.json", "/etc/passwd", "a/b.jpg", "A.jpg", "-a.jpg", "a.gif", 1]) {
      rejects([recipe({ photos: [file] })], /ファイル名/);
    }
  });

  test("同じ写真を 2 回使わない・無い写真やディレクトリを使わない", () => {
    const file = photo("twice.jpg", 10);
    rejects([recipe({ photos: [file] }), recipe({ photos: [file] })], /2 回/);
    rejects([recipe({ photos: ["missing.jpg"] })], /読めない（ENOENT）/);
    mkdirSync(join(dir, "dir.jpg"));
    rejects([recipe({ photos: ["dir.jpg"] })], /ふつうのファイルでない/);
  });

  test("写真は 1 バイトから上限のバイト数まで", () => {
    validate([recipe({ photos: [photo("max.jpg", MAX_PHOTO_BYTES)] })]);
    rejects([recipe({ photos: [photo("over.jpg", MAX_PHOTO_BYTES + 1)] })], /バイトまで/);
    rejects([recipe({ photos: [photo("empty.jpg", 0, Buffer.alloc(0))] })], /バイトまで/);
  });

  test("写真の形式は、中身の先頭で決める", () => {
    const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    const webp = Buffer.from("RIFF\0\0\0\0WEBP", "latin1");
    const { recipes } = validate([recipe({ photos: [photo("p.png", 20, png), photo("w.webp", 20, webp)] })]);
    assert.deepEqual(
      recipes[0].files.map((f) => f.contentType),
      ["image/png", "image/webp"],
    );
    rejects([recipe({ photos: [photo("text.jpg", 20, Buffer.from("not an image"))] })], /JPEG・PNG・WebP/);
  });

  test("写真の合計は、ワークスペースの上限まで", () => {
    const perRecipe = MAX_PHOTOS_PER_RECIPE;
    // 上限ちょうどになるよう、最大の大きさの写真を並べ、最後の 1 枚で端数を合わせる。そこへ写真を 1 枚足すと超える。
    const count = Math.ceil(MAX_WORKSPACE_PHOTO_BYTES / MAX_PHOTO_BYTES);
    const last = MAX_WORKSPACE_PHOTO_BYTES - (count - 1) * MAX_PHOTO_BYTES;
    // テストの前提: 最後の 1 枚が画像の先頭を持てる大きさで、レシピの件数の上限に収まる（上限の値を変えたら、作り方を見直す）。
    assert.ok(last >= jpeg.length);
    assert.ok(Math.ceil((count + 1) / MAX_PHOTOS_PER_RECIPE) <= MAX_RECIPES);
    const sizes = [...Array(count - 1).fill(MAX_PHOTO_BYTES), last, jpeg.length];
    const files = sizes.map((size, i) => photo(`t${i}.jpg`, size));
    const chunk = (list) =>
      Array.from({ length: Math.ceil(list.length / perRecipe) }, (_, i) =>
        recipe({ photos: list.slice(i * perRecipe, (i + 1) * perRecipe) }),
      );
    assert.equal(validate(chunk(files.slice(0, count))).photoBytes, MAX_WORKSPACE_PHOTO_BYTES);
    rejects(chunk(files), /ワークスペースの上限/);
  });
});
