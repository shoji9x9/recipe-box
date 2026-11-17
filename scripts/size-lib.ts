// アプリの規模（ファイル数と行数）を数える処理のうち、git とファイルに触れない部分（scripts/size.ts から使う）。
// 単体テストは scripts/size-lib.test.ts。
//
// 数える決まり（guide/about.md の「アプリの規模」と docs/development.md の「解説（/about）」）:
// - git で管理しているファイルを、1 つの commit で数える（作業中のファイルは見ない）。
// - 行数は物理的な行（改行の数。最後の行に改行が無ければ 1 を足す。空のファイルは 0）。空の行とコメントも数える。
// - バイナリ（NUL のバイトを含むか、写真などの拡張子のもの）は、ファイルの数にだけ数え、行数に数えない。
// - どのファイルも、分類の決まり（RULES）のどれか 1 つに当たる。どれにも当たらないファイルがあれば失敗させる
//   （新しいファイルを足したら、どこに数えるかを決めてここに書く）。
// - Rust の本番のファイルの末尾のテストのモジュール（#[cfg(test)] mod tests）は、行をテストに数え、ファイルは本番に数える。

import { REPOSITORY } from "./guide-lib.ts";

/** 数え方を決められないなどの誤り。分類の決まりか、数える元のファイルを直す必要がある。 */
export class SizeError extends Error {
  override name = "SizeError";
}

/** guide/about.md の中で、このスクリプトが書き換える範囲の印。 */
export const BLOCK_START = "<!-- size:start mise run size が書く。手で直さない -->";
export const BLOCK_END = "<!-- size:end -->";

/** 合計に数える分類。表に出す順に並べる。 */
export const GROUPS = [
  {
    name: "アプリ",
    categories: [
      "バックエンド（Rust）の本番",
      "バックエンド（Rust）のテスト",
      "バックエンド（JavaScript）の本番",
      "バックエンド（JavaScript）のテスト",
      "画面のテンプレート（HTML）",
      "画面の JavaScript の本番",
      "画面の JavaScript のテスト",
      "画面の CSS とアイコン",
      "DB のマイグレーション（SQL）",
    ],
  },
  {
    name: "開発・運用",
    categories: [
      "スクリプト（TypeScript）の本番",
      "スクリプト（TypeScript）のテスト",
      "スクリプト（シェル）",
      "CI と設定",
      "開発の文書",
      "解説の元",
      "seed（共有のデモの元のデータ。JSON・写真・出典）",
      "その他",
    ],
  },
] as const;

/** 合計に数える分類の名前。 */
export type Category = (typeof GROUPS)[number]["categories"][number];

/** 合計に入れない分類。 */
export const GENERATED = "生成物";
export const SKILLS = "スキル";

/** 分類の結果。 */
export type Classification = { category: Category | typeof GENERATED | typeof SKILLS };

const RUST_PRODUCTION: Category = "バックエンド（Rust）の本番";
const RUST_TEST: Category = "バックエンド（Rust）のテスト";

/** ルートに置く設定のファイル。新しいファイルを足したら、ここか、ほかの決まりに足す。 */
const ROOT_CONFIGS = [
  "mise.toml",
  "Cargo.toml",
  "package.json",
  "pnpm-workspace.yaml",
  "tsconfig.json",
  "vitest.config.ts",
  "commitlint.config.ts",
  "lefthook.yml",
  "deny.toml",
  ".taplo.toml",
  ".rumdl.toml",
  ".editorconfig",
  ".gitignore",
  ".mcp.json",
];

function exact(paths: string[]): RegExp {
  return new RegExp(`^(${paths.map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})$`);
}

const APP = "apps/recipe-box-rs/";

/** 分類の決まり。上から順に見て、最初に当たったものにする。 */
const RULES: { pattern: RegExp; result: Classification }[] = [
  // 生成物（lockfile と、mise run guide が作る解説）。
  { pattern: exact(["Cargo.lock", "pnpm-lock.yaml", "mise.lock", ".github/workflows/actions.lock"]), result: { category: GENERATED } },
  { pattern: new RegExp(`^${APP}public/about/`), result: { category: GENERATED } },
  { pattern: new RegExp(`^${APP}public/assets/guide/[^/]+\\.svg$`), result: { category: GENERATED } },
  // 外から入れたスキル。
  { pattern: /^\.claude\/skills\/[^/]+\//, result: { category: SKILLS } },
  // アプリ。
  { pattern: new RegExp(`^${APP}src/tests/`), result: { category: RUST_TEST } },
  { pattern: new RegExp(`^${APP}src/.+\\.rs$`), result: { category: RUST_PRODUCTION } },
  { pattern: new RegExp(`^${APP}src/[^/]+\\.(js|d\\.ts)$`), result: { category: "バックエンド（JavaScript）の本番" } },
  { pattern: new RegExp(`^${APP}js-tests/worker/`), result: { category: "バックエンド（JavaScript）のテスト" } },
  { pattern: new RegExp(`^${APP}templates/`), result: { category: "画面のテンプレート（HTML）" } },
  { pattern: new RegExp(`^${APP}public/assets/[^/]+\\.js$`), result: { category: "画面の JavaScript の本番" } },
  { pattern: new RegExp(`^${APP}js-tests/[^/]+\\.test\\.ts$`), result: { category: "画面の JavaScript のテスト" } },
  { pattern: exact([`${APP}public/assets/style.css`, `${APP}public/assets/favicon.svg`]), result: { category: "画面の CSS とアイコン" } },
  { pattern: new RegExp(`^${APP}migrations/`), result: { category: "DB のマイグレーション（SQL）" } },
  // 開発・運用。
  // 設定は、スクリプトの決まりより先に見る（scripts/tsconfig.json）。
  // Workers の静的アセットのヘッダー（public/_headers）は、wrangler.toml と同じく設定に数える。
  {
    pattern: exact([...ROOT_CONFIGS, ".claude/settings.json", "scripts/tsconfig.json", `${APP}Cargo.toml`, `${APP}wrangler.toml`, `${APP}public/_headers`]),
    result: { category: "CI と設定" },
  },
  { pattern: /^\.github\//, result: { category: "CI と設定" } },
  { pattern: /^\.config\/skills\//, result: { category: "CI と設定" } },
  // scripts/ のそのほかのファイル（JavaScript など）は、分類できないとして失敗させる。
  { pattern: /^scripts\/.+\.test\.ts$/, result: { category: "スクリプト（TypeScript）のテスト" } },
  { pattern: /^scripts\/.+\.ts$/, result: { category: "スクリプト（TypeScript）の本番" } },
  { pattern: /^scripts\/.+\.sh$/, result: { category: "スクリプト（シェル）" } },
  { pattern: exact(["README.md", "AGENTS.md", "CLAUDE.md", "SECURITY.md"]), result: { category: "開発の文書" } },
  { pattern: /^docs\//, result: { category: "開発の文書" } },
  { pattern: /^guide\//, result: { category: "解説の元" } },
  { pattern: exact([`${APP}public/assets/guide.css`]), result: { category: "解説の元" } },
  { pattern: /^seed\//, result: { category: "seed（共有のデモの元のデータ。JSON・写真・出典）" } },
  { pattern: exact(["LICENSE", "THIRD_PARTY_NOTICES.md"]), result: { category: "その他" } },
];

/** ファイルの分類。どの決まりにも当たらなければ undefined。 */
export function classify(path: string): Classification | undefined {
  for (const { pattern, result } of RULES) {
    if (pattern.test(path)) {
      return result;
    }
  }
  return undefined;
}

/** 中身を見なくてもバイナリと分かる拡張子。 */
const BINARY_EXTENSIONS = new Set(["jpg", "jpeg", "png", "webp", "gif", "ico", "avif", "wasm", "pdf", "woff", "woff2"]);

/** バイナリか（拡張子か、NUL のバイトで決める）。 */
export function isBinary(path: string, content: Uint8Array): boolean {
  const extension = path.includes(".") ? (path.split(".").pop() ?? "").toLowerCase() : "";
  return BINARY_EXTENSIONS.has(extension) || content.includes(0);
}

/** 物理的な行の数。改行（\n）の数に、最後の行に改行が無ければ 1 を足す。空のファイルは 0。 */
export function countLines(content: Uint8Array): number {
  let lines = 0;
  for (const byte of content) {
    if (byte === 0x0a) {
      lines += 1;
    }
  }
  return content.length > 0 && content[content.length - 1] !== 0x0a ? lines + 1 : lines;
}

/** 行の一覧（最後の改行の後の空の行は入れない）。 */
function splitLines(text: string): string[] {
  const lines = text.split(/\r?\n/);
  if (lines.at(-1) === "") {
    lines.pop();
  }
  return lines;
}

/**
 * Rust の本番のファイルのうち、末尾のテストのモジュールの行の数。
 * 行の頭の `#[cfg(test)]` の次の行が、行の頭の `mod tests {`（`pub(crate)` などが付いてもよい）なら、その `#[cfg(test)]` からファイルの終わりまで。
 * そのモジュールがファイルの最後の項目でなければ（閉じる `}` の後に何かあれば）、失敗させる。
 * rustfmt で整えたコードでは、行の頭の `}` は一番外の項目を閉じるので、`mod tests {` の後の最初の行の頭の `}` を、モジュールの終わりとみなす。
 * 次の行が `mod tests;`（ファイルを分けたモジュールの宣言）なら、その 2 行。
 * 行の頭の `#[cfg(...test...)]` の後に（属性を挟んで）モジュールが続くのに、この書き方でなければ、黙って本番に数えずに失敗させる
 * （`#[cfg(all(test, ...))]`・間に別の属性・`mod unit_tests` など）。
 */
export function rustTestLines(path: string, text: string): number {
  const lines = splitLines(text);
  const starts = lines.flatMap((line, i) => (line === "#[cfg(test)]" && /^(pub(\([a-z]+\))? )?mod tests\b/.test(lines[i + 1] ?? "") ? [i] : []));
  for (const [i, line] of lines.entries()) {
    if (!/^#\[cfg\(.*\btest\b.*\)\]$/.test(line) || starts.includes(i)) {
      continue;
    }
    const item = lines.slice(i + 1).find((next) => !next.startsWith("#["));
    if (item !== undefined && /^(pub(\([a-z]+\))? )?mod \w+/.test(item)) {
      throw new SizeError(`${path} の ${i + 1} 行目のテストのモジュールの書き方が、想定（#[cfg(test)] の次の行に mod tests）と違います`);
    }
  }
  if (starts.length === 0) {
    return 0;
  }
  if (starts.length > 1) {
    throw new SizeError(`${path} に、テストのモジュール（#[cfg(test)] mod tests）が 2 つ以上あります`);
  }
  const [start] = starts as [number];
  const declaration = lines[start + 1] ?? "";
  if (/^(pub(\([a-z]+\))? )?mod tests;$/.test(declaration)) {
    return 2;
  }
  if (!/^(pub(\([a-z]+\))? )?mod tests \{$/.test(declaration)) {
    throw new SizeError(`${path} のテストのモジュールの書き方が、想定と違います: ${declaration}`);
  }
  const close = lines.findIndex((line, i) => i > start + 1 && line === "}");
  const last = lines.findLastIndex((line) => line.trim() !== "");
  if (close === -1 || close !== last) {
    throw new SizeError(
      `${path} のテストのモジュール（${start + 1} 行目から）が、ファイルの最後の項目ではありません（テストの行を分けられません）`,
    );
  }
  return lines.length - start;
}

/** .claude/settings.json の、有効にしたプラグインの名前。 */
export function enabledPlugins(settings: string): string[] {
  const parsed = JSON.parse(settings) as { enabledPlugins?: Record<string, boolean> };
  return Object.entries(parsed.enabledPlugins ?? {})
    .filter(([, enabled]) => enabled)
    .map(([name]) => name)
    .sort();
}

/** 数える 1 つのファイル。 */
export interface SourceFile {
  path: string;
  content: Uint8Array;
}

/** 表の 1 行。行数は、テキストのファイルが 1 つも無ければ undefined（表では —）。 */
export interface Row {
  label: string;
  files: number;
  lines: number | undefined;
}

/** 数えた結果。 */
export interface Report {
  groups: { name: string; rows: Row[]; subtotal: Row }[];
  total: Row;
  generated: Row;
  skills: Row;
  plugins: string[];
}

function emptyRow(label: string): Row {
  return { label, files: 0, lines: undefined };
}

function add(row: Row, files: number, lines: number | undefined): void {
  row.files += files;
  if (lines !== undefined) {
    row.lines = (row.lines ?? 0) + lines;
  }
}

function sum(label: string, rows: Row[]): Row {
  const out = emptyRow(label);
  for (const row of rows) {
    add(out, row.files, row.lines);
  }
  return out;
}

const decoder = new TextDecoder();

/**
 * ファイルを分類して数える。`settings` は、同じ commit の .claude/settings.json の中身。
 * 分類できないファイルがあれば、その一覧を添えて失敗させる。
 */
export function tally(files: SourceFile[], settings: string): Report {
  const categories = new Map<string, Row>();
  for (const group of GROUPS) {
    for (const category of group.categories) {
      categories.set(category, emptyRow(category));
    }
  }
  const category = (name: string): Row => {
    const row = categories.get(name);
    if (row === undefined) {
      throw new SizeError(`知らない分類です: ${name}`);
    }
    return row;
  };
  const generated = emptyRow(GENERATED);
  const skills = emptyRow(SKILLS);
  // スキルの数（.claude/skills/<名前>/ のディレクトリの数）。
  const skillNames = new Set<string>();
  const unclassified: string[] = [];

  for (const { path, content } of files) {
    const result = classify(path);
    if (result === undefined) {
      unclassified.push(path);
      continue;
    }
    const lines = isBinary(path, content) ? undefined : countLines(content);
    if (result.category === SKILLS) {
      skillNames.add(path.split("/")[2] ?? "");
      add(skills, 1, lines);
      continue;
    }
    if (result.category === GENERATED) {
      add(generated, 1, lines);
      continue;
    }
    const row = category(result.category);
    if (result.category === RUST_PRODUCTION && lines !== undefined) {
      const test = rustTestLines(path, decoder.decode(content));
      add(row, 1, lines - test);
      if (test > 0) {
        add(category(RUST_TEST), 0, test);
      }
      continue;
    }
    add(row, 1, lines);
  }

  if (unclassified.length > 0) {
    throw new SizeError(
      `${unclassified.map((path) => `分類できない: ${path}`).join("\n")}\nscripts/size-lib.ts の分類の決まり（RULES）に足してください`,
    );
  }

  const groups = GROUPS.map((group) => {
    const rows = group.categories.map((category) => categories.get(category) ?? emptyRow(category));
    return { name: group.name, rows, subtotal: sum(`${group.name}の小計`, rows) };
  });
  skills.label = `スキル（.claude/skills/ の ${skillNames.size} つ）`;
  return {
    groups,
    total: sum("合計", groups.map((g) => g.subtotal)),
    generated,
    skills,
    plugins: enabledPlugins(settings),
  };
}

function number(value: number | undefined): string {
  return value === undefined ? "—" : value.toLocaleString("en-US");
}

/** 表の 1 行。`first` は、先頭に足す列（区分）。 */
function tableRow(row: Row, options: { bold?: boolean; first?: string } = {}): string {
  const cell = (text: string) => (options.bold ? `**${text}**` : text);
  const cells = [...(options.first === undefined ? [] : [options.first]), cell(row.label), cell(number(row.files)), cell(number(row.lines))];
  return `|${cells.map((text) => (text === "" ? " " : ` ${text} `)).join("|")}|`;
}

/** 数えた commit。 */
export interface Commit {
  sha: string;
  /** commit の日付（YYYY-MM-DD）。 */
  date: string;
}

/** guide/about.md の印の間に入れる Markdown（印の行を含む）。 */
export function renderBlock(report: Report, commit: Commit): string {
  // 区分の名前は、その区分の最初の行にだけ書く。小計と合計は太字にする。
  const counted = report.groups.flatMap((group) => [
    ...group.rows.map((row, i) => tableRow(row, { first: i === 0 ? group.name : "" })),
    tableRow(group.subtotal, { first: "", bold: true }),
  ]);
  const plugins = report.plugins.length === 0 ? "無し" : report.plugins.map((p) => `\`${p}\``).join("・");
  return [
    BLOCK_START,
    `commit [${commit.sha.slice(0, 7)}](${REPOSITORY}commit/${commit.sha})（${commit.date}）で数えました。数え直すには \`git fetch\` の後に \`mise run size\` を実行します。`,
    "",
    "| 区分 | 分類 | ファイル数 | 行数 |",
    "| --- | --- | ---: | ---: |",
    ...counted,
    tableRow(report.total, { first: "", bold: true }),
    "",
    "別枠（合計に入れないもの）:",
    "",
    "| 分類 | ファイル数 | 行数 |",
    "| --- | ---: | ---: |",
    tableRow(report.generated),
    tableRow(report.skills),
    "",
    `Claude Code のプラグイン（${plugins}）は、コードがリポジトリに無いので数えません。`,
    BLOCK_END,
  ].join("\n");
}

/** `text`（guide/about.md）の印の間を、`block`（印の行を含む）に置き換える。印が 1 組ずつ無ければ失敗させる。 */
export function replaceBlock(text: string, block: string): string {
  const lines = text.split("\n");
  const starts = lines.flatMap((line, i) => (line.trim().startsWith("<!-- size:start") ? [i] : []));
  const ends = lines.flatMap((line, i) => (line.trim() === BLOCK_END ? [i] : []));
  if (starts.length !== 1 || ends.length !== 1 || (ends[0] ?? 0) < (starts[0] ?? 0)) {
    throw new SizeError(
      `guide/about.md に、印 <!-- size:start --> と ${BLOCK_END} が、この順に 1 つずつありません（start ${starts.length} 個・end ${ends.length} 個）`,
    );
  }
  const [start] = starts as [number];
  const [end] = ends as [number];
  return [...lines.slice(0, start), block, ...lines.slice(end + 1)].join("\n");
}

/** `git ls-tree -r -z <commit>` の出力の 1 件。 */
export interface TreeEntry {
  mode: string;
  type: string;
  object: string;
  path: string;
}

/** `git ls-tree -r -z` の出力を読む。ファイル（blob）でないもの（submodule など）があれば失敗させる。 */
export function parseTree(output: string): TreeEntry[] {
  return output
    .split("\0")
    .filter((entry) => entry !== "")
    .map((entry) => {
      const match = /^(\d+) (\w+) ([0-9a-f]+)\t(.+)$/s.exec(entry);
      if (!match) {
        throw new SizeError(`git ls-tree の出力を読めません: ${entry}`);
      }
      const [, mode, type, object, path] = match as unknown as [string, string, string, string, string];
      if (type !== "blob") {
        throw new SizeError(`ファイルでないもの（${type}）は数えられません: ${path}`);
      }
      return { mode, type, object, path };
    });
}

/** `git cat-file --batch` の出力（`<object> blob <大きさ>\n<中身>\n` のくり返し）を、object ごとの中身にする。 */
export function parseBatch(output: Uint8Array): Map<string, Uint8Array> {
  const contents = new Map<string, Uint8Array>();
  let offset = 0;
  while (offset < output.length) {
    const newline = output.indexOf(0x0a, offset);
    if (newline === -1) {
      throw new SizeError("git cat-file の出力が途中で切れています");
    }
    const header = decoder.decode(output.subarray(offset, newline));
    const match = /^([0-9a-f]+) blob (\d+)$/.exec(header);
    if (!match) {
      throw new SizeError(`git cat-file の出力を読めません: ${header}`);
    }
    const size = Number(match[2]);
    const start = newline + 1;
    // 中身の後に、区切りの改行が 1 つある。
    if (start + size >= output.length || output[start + size] !== 0x0a) {
      throw new SizeError("git cat-file の出力が途中で切れています");
    }
    contents.set(match[1] ?? "", output.subarray(start, start + size));
    offset = start + size + 1;
  }
  return contents;
}
