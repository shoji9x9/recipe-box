#!/usr/bin/env node
// アプリの解説（/about）を、元の文章（guide/about.md）と図の元（guide/*.d2）から作る（mise run guide）。
// Node が型の注釈を取り除いて、そのまま動かす（ビルドしない。docs/development.md の「言語」）。
//
// 使い方:
//   node scripts/guide.ts          作り直して、静的アセットの置き場に書く
//   node scripts/guide.ts --check  作り直したものが置いてあるものと同じかを確かめる（mise run lint と CI）。違えば失敗する
//
// 書き出すもの（どちらも commit する）:
//   apps/recipe-box-rs/public/about/index.html  解説のページ
//   apps/recipe-box-rs/public/assets/guide/      図（SVG）
// 引用するコードは、生成のたびにリポジトリのファイルの印から取り出す（scripts/guide-lib.ts）。印が無ければ失敗する。
// 図は mise で版を固定した d2 で作る。同じ元からは同じ SVG ができる。
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expandDirectives, GuideError, renderMarkdown, renderPage } from "./guide-lib.ts";

const SOURCE = "guide/about.md";
const DIAGRAM_SOURCE_DIR = "guide";
const PAGE_OUT = "apps/recipe-box-rs/public/about/index.html";
const DIAGRAM_OUT_DIR = "apps/recipe-box-rs/public/assets/guide";

/** 作るファイルの、パスと中身。 */
type Outputs = Map<string, string>;

function build(): Outputs {
  const { markdown, diagrams } = expandDirectives(readFileSync(SOURCE, "utf8"), (path) => {
    if (!existsSync(path)) {
      throw new GuideError(`引用する元のファイルがありません: ${path}`);
    }
    return readFileSync(path, "utf8");
  });
  const page = renderMarkdown(markdown, existsSync);
  const outputs: Outputs = new Map([[PAGE_OUT, renderPage(page)]]);

  const sources = readdirSync(DIAGRAM_SOURCE_DIR).filter((file) => file.endsWith(".d2"));
  const used = new Set(diagrams.map((d) => d.name));
  const unused = sources.filter((file) => !used.has(file.slice(0, -".d2".length)));
  if (unused.length > 0) {
    throw new GuideError(`使っていない図の元があります（消すか、${SOURCE} で使ってください）: ${unused.join(", ")}`);
  }
  const work = mkdtempSync(join(tmpdir(), "recipe-box-guide-"));
  try {
    for (const { name } of diagrams) {
      const source = join(DIAGRAM_SOURCE_DIR, `${name}.d2`);
      if (!existsSync(source)) {
        throw new GuideError(`図の元がありません: ${source}`);
      }
      const out = join(work, `${name}.svg`);
      // 余白を小さくし、XML の宣言を付けない。テーマは既定（0）。d2 が失敗すれば、その出力を添えて例外になる。
      // --scale 1 で、図に本来の幅と高さを持たせる（ページでは CSS で幅に合わせて縮め、押すと元の大きさで開く）。
      execFileSync("d2", ["--pad", "16", "--scale", "1", "--no-xml-tag", source, out], {
        stdio: ["ignore", "ignore", "pipe"],
      });
      outputs.set(join(DIAGRAM_OUT_DIR, `${name}.svg`), readFileSync(out, "utf8"));
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
  return outputs;
}

/** 置き場にあるが、作るものに無いファイル（消し忘れた図）。 */
function staleFiles(outputs: Outputs): string[] {
  if (!existsSync(DIAGRAM_OUT_DIR)) {
    return [];
  }
  return readdirSync(DIAGRAM_OUT_DIR)
    .map((file) => join(DIAGRAM_OUT_DIR, file))
    .filter((path) => !outputs.has(path));
}

function main(args: string[]): void {
  if (args.length > 1 || (args.length === 1 && args[0] !== "--check")) {
    console.error("使い方: node scripts/guide.ts [--check]");
    process.exit(2);
  }
  const outputs = build();
  const stale = staleFiles(outputs);
  if (args[0] === "--check") {
    const differ = [...outputs].filter(([path, text]) => !existsSync(path) || readFileSync(path, "utf8") !== text);
    const problems = [...differ.map(([path]) => `作り直したものと違う: ${path}`), ...stale.map((p) => `余分: ${p}`)];
    if (problems.length > 0) {
      console.error(`${problems.join("\n")}\nmise run guide で作り直して commit してください（docs/development.md）`);
      process.exit(1);
    }
    console.log("解説（/about）は、元の文章と引用するファイルに合っています");
    return;
  }
  for (const path of stale) {
    rmSync(path);
  }
  for (const [path, text] of outputs) {
    mkdirSync(join(path, ".."), { recursive: true });
    writeFileSync(path, text);
  }
  console.log(`作りました: ${[...outputs.keys()].join(", ")}`);
}

try {
  main(process.argv.slice(2));
} catch (error) {
  if (error instanceof GuideError) {
    console.error(error.message);
    process.exit(1);
  }
  throw error;
}
