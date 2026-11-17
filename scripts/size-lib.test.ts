// scripts/size-lib.ts の単体テスト（mise run test の vitest で実行する）。
import { describe, expect, test } from "vitest";

import {
  BLOCK_END,
  BLOCK_START,
  classify,
  countLines,
  enabledPlugins,
  isBinary,
  parseBatch,
  parseTree,
  renderBlock,
  replaceBlock,
  rustTestLines,
  SizeError,
  skillSources,
  tally,
} from "./size-lib.ts";

const encoder = new TextEncoder();
const bytes = (text: string) => encoder.encode(text);

const AGENTS = `# AGENTS.md

## スキル

\`.claude/skills/\` に、[shoji9x9/skills](https://github.com/shoji9x9/skills) から次のスキルを入れている。

- \`pr-finalize-loop\`: PR を収束させる。
- \`browser-test\`: 画面を確かめる。

\`pr-finalize-loop\` の再レビューの依頼先は \`claude-code\` にしている。

[cloudflare/security-audit-skill](https://github.com/cloudflare/security-audit-skill) から、次のスキルを入れている。

- \`security-audit\`: セキュリティのレビュー。
  - \`nested\`: 字下げした項目はスキルにしない。

## 次の節

- \`outside\`: 節の外はスキルにしない。
`;

const SETTINGS = JSON.stringify({ enabledPlugins: { "b@market": true, "a@market": true, "off@market": false } });

describe("classify", () => {
  test.each([
    ["apps/recipe-box-rs/src/screen.rs", "バックエンド（Rust）の本番"],
    ["apps/recipe-box-rs/src/nested/mod.rs", "バックエンド（Rust）の本番"],
    ["apps/recipe-box-rs/src/tests/mod.rs", "バックエンド（Rust）のテスト"],
    ["apps/recipe-box-rs/src/photo-stream.js", "バックエンド（JavaScript）の本番"],
    ["apps/recipe-box-rs/src/workers.d.ts", "バックエンド（JavaScript）の本番"],
    ["apps/recipe-box-rs/js-tests/worker/photo-stream.test.ts", "バックエンド（JavaScript）のテスト"],
    ["apps/recipe-box-rs/templates/base.html", "画面のテンプレート（HTML）"],
    ["apps/recipe-box-rs/public/assets/page.js", "画面の JavaScript の本番"],
    ["apps/recipe-box-rs/js-tests/page.test.ts", "画面の JavaScript のテスト"],
    ["apps/recipe-box-rs/public/assets/style.css", "画面の CSS とアイコン"],
    ["apps/recipe-box-rs/public/assets/favicon.svg", "画面の CSS とアイコン"],
    ["apps/recipe-box-rs/migrations/0001_create_workspaces.sql", "DB のマイグレーション（SQL）"],
    ["scripts/size.ts", "スクリプトの本番"],
    ["scripts/mwg.sh", "スクリプトの本番"],
    ["scripts/size-lib.test.ts", "スクリプトのテスト"],
    ["scripts/tsconfig.json", "CI と設定"],
    [".github/workflows/ci.yml", "CI と設定"],
    ["mise.toml", "CI と設定"],
    ["apps/recipe-box-rs/Cargo.toml", "CI と設定"],
    ["apps/recipe-box-rs/wrangler.toml", "CI と設定"],
    ["apps/recipe-box-rs/public/_headers", "CI と設定"],
    [".claude/settings.json", "CI と設定"],
    [".config/skills/shoji9x9/common.jsonc", "CI と設定"],
    ["README.md", "開発の文書"],
    ["CLAUDE.md", "開発の文書"],
    ["docs/spec.md", "開発の文書"],
    ["guide/about.md", "解説の元"],
    ["guide/_style.d2", "解説の元"],
    ["apps/recipe-box-rs/public/assets/guide.css", "解説の元"],
    ["seed/recipes.json", "seed（共有のデモの元のデータ）"],
    ["seed/photos/gyoza-1.jpg", "seed（共有のデモの元のデータ）"],
    ["LICENSE", "その他"],
    ["THIRD_PARTY_NOTICES.md", "その他"],
    ["Cargo.lock", "生成物"],
    ["pnpm-lock.yaml", "生成物"],
    ["mise.lock", "生成物"],
    [".github/workflows/actions.lock", "生成物"],
    ["apps/recipe-box-rs/public/about/index.html", "生成物"],
    ["apps/recipe-box-rs/public/assets/guide/architecture.svg", "生成物"],
  ])("%s は %s", (path, category) => {
    expect(classify(path)).toEqual({ category });
  });

  test("スキルは、ディレクトリの名前で分ける", () => {
    expect(classify(".claude/skills/browser-test/SKILL.md")).toEqual({ skill: "browser-test" });
    expect(classify(".claude/skills/security-audit/references/a.md")).toEqual({ skill: "security-audit" });
  });

  test.each([
    "new-file.txt",
    "apps/recipe-box-rs/public/assets/htmx.min.css",
    "apps/recipe-box-rs/js-tests/helper.ts",
    ".claude/agents/x.md",
    "apps/recipe-box-rs/public/robots.txt",
  ])("決まりに無いファイル（%s）は分類しない", (path) => {
    expect(classify(path)).toBeUndefined();
  });
});

describe("countLines", () => {
  test.each([
    ["", 0],
    ["a", 1],
    ["a\n", 1],
    ["a\nb", 2],
    ["a\nb\n", 2],
    ["\n", 1],
    ["\n\n", 2],
    ["a\r\nb\r\n", 2],
    ["日本語\n", 1],
  ])("%j は %i 行", (text, lines) => {
    expect(countLines(bytes(text))).toBe(lines);
  });
});

describe("isBinary", () => {
  test("写真などの拡張子はバイナリ", () => {
    expect(isBinary("seed/photos/a.jpg", bytes("text"))).toBe(true);
    expect(isBinary("a.PNG", bytes("text"))).toBe(true);
    expect(isBinary("a.webp", bytes(""))).toBe(true);
  });

  test("NUL のバイトを含めばバイナリ", () => {
    expect(isBinary("data.bin", new Uint8Array([0x61, 0x00, 0x62]))).toBe(true);
  });

  test("テキストはバイナリでない", () => {
    expect(isBinary("a.rs", bytes("fn main() {}\n"))).toBe(false);
    expect(isBinary("LICENSE", bytes("MIT\n"))).toBe(false);
    expect(isBinary("a.svg", bytes("<svg/>"))).toBe(false);
  });
});

describe("rustTestLines", () => {
  test("テストのモジュールが無ければ 0", () => {
    expect(rustTestLines("a.rs", "fn a() {}\n")).toBe(0);
  });

  test("末尾のモジュールは、#[cfg(test)] からファイルの終わりまで", () => {
    const text = "fn a() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        let _ = \"{\";\n    }\n}\n";
    expect(rustTestLines("a.rs", text)).toBe(7);
  });

  test("pub(crate) mod tests も数え、最後の改行が無くても、後ろの空の行も数える", () => {
    expect(rustTestLines("a.rs", "fn a() {}\n#[cfg(test)]\npub(crate) mod tests {\n    fn t() {}\n}")).toBe(4);
    expect(rustTestLines("a.rs", "fn a() {}\n#[cfg(test)]\nmod tests {\n}\n\n")).toBe(4);
  });

  test("ファイルを分けたモジュールの宣言（mod tests;）は 2 行", () => {
    expect(rustTestLines("lib.rs", "mod a;\n#[cfg(test)]\nmod tests;\n\nfn b() {}\n")).toBe(2);
  });

  test("ほかの #[cfg(test)] は、本番に数える", () => {
    expect(rustTestLines("a.rs", "#[cfg(test)]\nfn helper() {}\n")).toBe(0);
    expect(rustTestLines("a.rs", "mod a {\n    #[cfg(test)]\n    mod tests {}\n}\n")).toBe(0);
  });

  test("モジュールの後に項目があれば失敗する", () => {
    expect(() => rustTestLines("a.rs", "#[cfg(test)]\nmod tests {\n}\n\nfn after() {}\n")).toThrow(SizeError);
    expect(() => rustTestLines("a.rs", "#[cfg(test)]\nmod tests {\n}\nconst A: i32 = 1;\n")).toThrow(/最後の項目ではありません/);
  });

  test("閉じていなければ失敗する", () => {
    expect(() => rustTestLines("a.rs", "#[cfg(test)]\nmod tests {\n    fn t() {}\n")).toThrow(SizeError);
  });

  test("テストのモジュールが 2 つあるか、書き方が想定と違えば失敗する", () => {
    expect(() => rustTestLines("a.rs", "#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod tests {\n}\n")).toThrow(/2 つ以上/);
    expect(() => rustTestLines("a.rs", "#[cfg(test)]\nmod tests { fn t() {} }\n")).toThrow(/想定と違います/);
  });
});

describe("skillSources", () => {
  test("「スキル」の節の、入手元ごとのスキルの一覧を読む", () => {
    expect([...skillSources(AGENTS)]).toEqual([
      ["pr-finalize-loop", "shoji9x9/skills"],
      ["browser-test", "shoji9x9/skills"],
      ["security-audit", "cloudflare/security-audit-skill"],
    ]);
  });
});

describe("enabledPlugins", () => {
  test("有効なものだけを、名前の順に返す", () => {
    expect(enabledPlugins(SETTINGS)).toEqual(["a@market", "b@market"]);
    expect(enabledPlugins("{}")).toEqual([]);
  });
});

describe("tally", () => {
  const files = [
    { path: "apps/recipe-box-rs/src/a.rs", content: bytes("fn a() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n") },
    { path: "apps/recipe-box-rs/src/lib.rs", content: bytes("mod a;\n#[cfg(test)]\nmod tests;\n") },
    { path: "apps/recipe-box-rs/src/tests/mod.rs", content: bytes("fn t() {}\n") },
    { path: "seed/recipes.json", content: bytes("[]\n") },
    { path: "seed/photos/a.jpg", content: new Uint8Array([0xff, 0xd8, 0x00]) },
    { path: "Cargo.lock", content: bytes("a\nb\n") },
    { path: ".claude/skills/browser-test/SKILL.md", content: bytes("a\nb\nc\n") },
    { path: ".claude/skills/pr-finalize-loop/SKILL.md", content: bytes("a\n") },
    { path: ".claude/skills/pr-finalize-loop/x.png", content: new Uint8Array([1]) },
  ];

  const row = (report: ReturnType<typeof tally>, label: string) =>
    report.groups.flatMap((g) => [...g.rows, g.subtotal]).find((r) => r.label === label);

  test("Rust のテストの行を分け、ファイルは本番に数える", () => {
    const report = tally(files, AGENTS, SETTINGS);
    // a.rs の 2 行と lib.rs の 1 行が本番。a.rs の 4 行・lib.rs の 2 行・tests/mod.rs の 1 行がテスト。
    expect(row(report, "バックエンド（Rust）の本番")).toEqual({ label: "バックエンド（Rust）の本番", files: 2, lines: 3 });
    expect(row(report, "バックエンド（Rust）のテスト")).toEqual({ label: "バックエンド（Rust）のテスト", files: 1, lines: 7 });
  });

  test("バイナリはファイルの数にだけ数え、テキストが無ければ行数は無し", () => {
    const report = tally(files, AGENTS, SETTINGS);
    expect(row(report, "seed（共有のデモの元のデータ）")).toMatchObject({ files: 2, lines: 1 });
    expect(row(report, "その他")).toEqual({ label: "その他", files: 0, lines: undefined });
  });

  test("合計に別枠を入れず、スキルを AGENTS.md の順に並べる", () => {
    const report = tally(files, AGENTS, SETTINGS);
    expect(report.total).toEqual({ label: "合計", files: 5, lines: 11 });
    expect(report.generated).toEqual({ label: "生成物", files: 1, lines: 2 });
    expect(report.skills).toEqual([
      { label: "pr-finalize-loop（shoji9x9/skills）", files: 2, lines: 1 },
      { label: "browser-test（shoji9x9/skills）", files: 1, lines: 3 },
    ]);
    expect(report.skillsSubtotal).toEqual({ label: "スキルの小計", files: 3, lines: 4 });
    expect(report.plugins).toEqual(["a@market", "b@market"]);
  });

  test("分類できないファイルと、入手元の無いスキルがあれば、一覧を添えて失敗する", () => {
    const extra = [
      ...files,
      { path: "unknown.txt", content: bytes("x\n") },
      { path: "other/unknown.md", content: bytes("x\n") },
      { path: ".claude/skills/mystery/SKILL.md", content: bytes("x\n") },
    ];
    expect(() => tally(extra, AGENTS, SETTINGS)).toThrow(SizeError);
    expect(() => tally(extra, AGENTS, SETTINGS)).toThrow(
      /分類できない: unknown\.txt\n分類できない: other\/unknown\.md\nAGENTS\.md の「スキル」に入手元が書かれていないスキル: \.claude\/skills\/mystery\//,
    );
  });
});

describe("renderBlock と replaceBlock", () => {
  const report = tally(
    [
      { path: "LICENSE", content: bytes("x\n".repeat(1234)) },
      { path: "seed/photos/a.jpg", content: new Uint8Array([1]) },
    ],
    AGENTS,
    "{}",
  );
  const block = renderBlock(report, { sha: "0123456789abcdef0123456789abcdef01234567", date: "2026-11-17" });

  test("commit へのリンクと表を書く", () => {
    expect(block.startsWith(`${BLOCK_START}\n`)).toBe(true);
    expect(block.endsWith(`\n${BLOCK_END}`)).toBe(true);
    expect(block).toContain(
      "commit [0123456](https://github.com/shoji9x9/recipe-box/commit/0123456789abcdef0123456789abcdef01234567)（2026-11-17）で数えました。",
    );
    expect(block).toContain("| その他 | 1 | 1,234 |");
    expect(block).toContain("| seed（共有のデモの元のデータ） | 1 | — |");
    expect(block).toContain("| **合計** | **2** | **1,234** |");
    expect(block).toContain("| スキルの小計 | 0 | — |");
    expect(block).toContain("Claude Code のプラグイン（無し）");
  });

  test("印の間だけを書き換える", () => {
    const before = `# 題\n\n前\n\n${BLOCK_START}\n古い表\n${BLOCK_END}\n\n後\n`;
    expect(replaceBlock(before, "新しい")).toBe("# 題\n\n前\n\n新しい\n\n後\n");
    expect(replaceBlock(replaceBlock(before, block), block)).toBe(replaceBlock(before, block));
  });

  test("印が 1 組ずつ無ければ失敗する", () => {
    expect(() => replaceBlock("本文\n", block)).toThrow(SizeError);
    expect(() => replaceBlock(`${BLOCK_START}\n${BLOCK_END}\n${BLOCK_START}\n${BLOCK_END}\n`, block)).toThrow(SizeError);
    expect(() => replaceBlock(`${BLOCK_END}\n${BLOCK_START}\n`, block)).toThrow(SizeError);
  });
});

describe("parseTree と parseBatch", () => {
  test("git ls-tree -r -z の出力を読む", () => {
    const output = "100644 blob aaaa\tREADME.md\u0000100755 blob bbbb\tscripts/a b.sh\0";
    expect(parseTree(output)).toEqual([
      { mode: "100644", type: "blob", object: "aaaa", path: "README.md" },
      { mode: "100755", type: "blob", object: "bbbb", path: "scripts/a b.sh" },
    ]);
  });

  test("submodule などのファイルでないものがあれば失敗する", () => {
    expect(() => parseTree("160000 commit cccc\tvendor/x\0")).toThrow(/ファイルでないもの/);
  });

  test("git cat-file --batch の出力を、object ごとの中身にする", () => {
    const output = bytes("aaaa blob 3\na\nb\nbbbb blob 0\n\n");
    const contents = parseBatch(output);
    expect(new TextDecoder().decode(contents.get("aaaa"))).toBe("a\nb");
    expect(contents.get("bbbb")?.length).toBe(0);
  });

  test("途中で切れた出力や、見つからない object は失敗する", () => {
    expect(() => parseBatch(bytes("aaaa blob 5\nab\n"))).toThrow(/途中で切れています/);
    expect(() => parseBatch(bytes("aaaa missing\n"))).toThrow(/読めません/);
  });
});
