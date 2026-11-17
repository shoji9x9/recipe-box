#!/usr/bin/env node
// アプリの規模（ファイル数と行数）を、1 つの commit で数え、解説の元（guide/about.md）の「アプリの規模」の表を書き換える（mise run size）。
// Node が型の注釈を取り除いて、そのまま動かす（ビルドしない。docs/development.md の「言語」）。
//
// 使い方:
//   node scripts/size.ts                 origin/main で数える（解説の表。先に git fetch する。docs/development.md の「解説（/about）」）
//   node scripts/size.ts --ref <commit>  指定した commit で数える
//
// ファイルの一覧と中身は git から読む（git ls-tree と git cat-file）。作業中のファイルは見ないので、結果は commit だけで決まる。
// 分類と数え方は scripts/size-lib.ts。書き換えた後に mise run guide で /about を作り直す。
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { parseBatch, parseTree, renderBlock, replaceBlock, SizeError, tally } from "./size-lib.ts";

const TARGET = "guide/about.md";

function git(args: string[], input?: string): Buffer {
  return execFileSync("git", args, { input, maxBuffer: 1024 * 1024 * 1024, stdio: ["pipe", "pipe", "inherit"] });
}

function main(args: string[]): void {
  // 数え直す PR そのものも数を変えるので、既定は作業中のブランチ（HEAD）ではなく main にする。
  let ref = "origin/main";
  if (args.length === 2 && args[0] === "--ref" && args[1] !== undefined && !args[1].startsWith("-")) {
    ref = args[1];
  } else if (args.length !== 0) {
    console.error("使い方: node scripts/size.ts [--ref <commit>]");
    process.exit(2);
  }
  // --end-of-options で、ref を git の設定として読ませない。
  // SHA と日付は別々に読む（log.showSignature などの設定で、署名の行が出力に混ざらないように）。
  const sha = git(["rev-parse", "--verify", "--end-of-options", `${ref}^{commit}`]).toString().trim();
  const date = git(["log", "-1", "--no-show-signature", "--format=%cs", sha]).toString().trim();
  const entries = parseTree(git(["ls-tree", "-r", "-z", "--full-tree", "--end-of-options", sha]).toString());
  const contents = parseBatch(git(["cat-file", "--batch"], entries.map((e) => `${e.object}\n`).join("")));
  const files = entries.map(({ path, object }) => {
    const content = contents.get(object);
    if (content === undefined) {
      throw new SizeError(`git cat-file が中身を返しませんでした: ${path}`);
    }
    return { path, content };
  });
  const text = (path: string) => {
    const file = files.find((f) => f.path === path);
    if (file === undefined) {
      throw new SizeError(`${sha} に ${path} がありません`);
    }
    return new TextDecoder().decode(file.content);
  };
  const report = tally(files, text(".claude/settings.json"));
  writeFileSync(TARGET, replaceBlock(readFileSync(TARGET, "utf8"), renderBlock(report, { sha, date })));
  console.log(`${sha.slice(0, 7)}（${date}）で数え、${TARGET} を書き換えました。mise run guide で /about を作り直してください`);
}

try {
  main(process.argv.slice(2));
} catch (error) {
  if (error instanceof SizeError) {
    console.error(error.message);
    process.exit(1);
  }
  throw error;
}
