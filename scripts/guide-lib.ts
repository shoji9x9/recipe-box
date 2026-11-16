// アプリの解説（/about）を作る処理のうち、ファイルと外部のコマンドに触れない部分（scripts/guide.ts から使う）。
// 単体テストは scripts/guide-lib.test.ts。
//
// 元の Markdown（guide/about.md）には、次の 2 つの印を書ける。どちらも行の全体に書く。
//   <!-- code: <リポジトリの中のパス>#<名前> -->
//     そのファイルの `guide:start <名前>` の行から `guide:end <名前>` の行までを、コードのブロックとして、元のファイルへのリンクの後に入れる。
//     印の行そのものは入れない。コードを写して貼らず、生成のたびに main のファイルから取り出すためである。
//   <!-- diagram: <名前> | <図の説明> -->
//     guide/<名前>.d2 から作った図（/assets/guide/<名前>.svg）を入れる。説明は画像の代わりの文（alt）にする。
// 印が見つからないとき、印の書き方が違うとき、リンクの先のファイルが無いときは、生成を失敗させる。
import MarkdownIt from "markdown-it";

/** 解説のリンクと引用の元にする、公開リポジトリの main のファイルの URL の先頭。 */
export const REPOSITORY_BLOB = "https://github.com/shoji9x9/recipe-box/blob/main/";

/** expandDirectives が、引用するコードの前に置く行の形。 */
const CITE_LINE = /^\[[^\]]+\]\(https:\/\/[^)]+\) から:$/;

/** 図を置くパス（Workers の静的アセット）。 */
export const DIAGRAM_PATH = "/assets/guide/";

/** 生成を止める誤り。元の文章か、引用するファイルを直す必要がある。 */
export class GuideError extends Error {
  override name = "GuideError";
}

/** 拡張子から、コードのブロックの言語の名前を決める。 */
const LANGUAGES: Record<string, string> = {
  rs: "rust",
  toml: "toml",
  html: "html",
  js: "javascript",
  ts: "typescript",
  css: "css",
  yml: "yaml",
  yaml: "yaml",
  sql: "sql",
  sh: "bash",
  json: "json",
};

/**
 * `text`（`path` のファイルの中身）から、`guide:start <name>` と `guide:end <name>` の行の間を取り出す。
 * 共通の字下げは外す。印が無い・2 つ以上ある・中身が空なら、GuideError を投げる。
 */
export function extractSnippet(text: string, path: string, name: string): string {
  const lines = text.split(/\r?\n/);
  const isMarker = (line: string, kind: "start" | "end") =>
    new RegExp(`(^|\\s)guide:${kind}\\s+${escapeRegExp(name)}\\s*(-->|\\*/|#\\}|$)`).test(line.trim());
  const starts = lines.flatMap((line, i) => (isMarker(line, "start") ? [i] : []));
  const ends = lines.flatMap((line, i) => (isMarker(line, "end") ? [i] : []));
  if (starts.length !== 1 || ends.length !== 1) {
    throw new GuideError(
      `${path} に、印 guide:start ${name} と guide:end ${name} が 1 つずつありません（start ${starts.length} 個・end ${ends.length} 個）`,
    );
  }
  const [start] = starts as [number];
  const [end] = ends as [number];
  if (end <= start + 1) {
    throw new GuideError(`${path} の印 ${name} の中身が空か、end が start より前にあります`);
  }
  return dedent(lines.slice(start + 1, end), path, name);
}

/** 行の共通の字下げ（空白）を外して、改行でつなぐ。空の行は数えない。 */
function dedent(lines: string[], path: string, name: string): string {
  const indents = lines.filter((line) => line.trim() !== "").map((line) => /^[ \t]*/.exec(line)?.[0].length ?? 0);
  if (indents.length === 0) {
    throw new GuideError(`${path} の印 ${name} の中身が空です`);
  }
  const indent = Math.min(...indents);
  return lines.map((line) => line.slice(indent).trimEnd()).join("\n");
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * guide/ のファイルの名前から、図の元（.d2）を選ぶ。_ で始まるものは、各図が読み込む共通の設定（_style.d2）なので図にしない。
 */
export function diagramSources(files: string[]): string[] {
  return files.filter((file) => file.endsWith(".d2") && !file.startsWith("_")).sort();
}

/** d2 が SVG に埋め込むフォントの種類と、文字の class の名前と、OS のフォントで同じ形にするための指定。 */
const D2_FONTS = [
  { kind: "regular", className: "text", style: "" },
  { kind: "bold", className: "text-bold", style: " font-weight: 700;" },
  { kind: "italic", className: "text-italic", style: " font-style: italic;" },
] as const;

/**
 * 図の文字に使う OS のフォント。アプリの画面の文字（apps/recipe-box-rs/public/assets/style.css の --font）と同じにする。
 * 日本語のゴシック（Hiragino Sans・Noto Sans JP、無ければ sans-serif）まで並べ、明朝で描かれないようにする。
 */
export const SYSTEM_FONTS =
  '-apple-system, system-ui, "Segoe UI", Roboto, "Helvetica Neue", "Hiragino Sans", "Noto Sans JP", sans-serif';

/**
 * d2 が作った SVG から埋め込みのフォントを取り除き、文字を OS のフォントで描く。
 * d2 は文字を、図に使う文字だけに絞ったフォント（data: の URL の @font-face）で描く。そのフォントは配らない
 * （ライセンスの表示が要り、日本語を持たないので英字と日本語の形がそろわない）。
 * - @font-face を消す。
 * - 文字の class のフォントを、OS のフォントの並び（SYSTEM_FONTS）にする。太字は太さ（700）を書く。
 * 斜体は、日本語の斜体の字形が無いので、図の元で使わない（guide/_style.d2）。
 * d2 の出力の形が変わって書き換えられなければ、失敗させる（黙ってフォントを埋め込んだ図を作らない）。
 */
export function systemFonts(svg: string): string {
  let out = svg;
  for (const { kind, className, style } of D2_FONTS) {
    const rule = new RegExp(`(\\.d2-\\d+ \\.${className} \\{\\s*font-family: )"d2-\\d+-font-${kind}";`, "g");
    const face = new RegExp(
      `\\n?@font-face \\{\\s*font-family: d2-\\d+-font-${kind};\\s*src: url\\("data:[^"]*"\\);\\s*\\}`,
      "g",
    );
    const rules = out.match(rule)?.length ?? 0;
    const faces = out.match(face)?.length ?? 0;
    if (rules === 0 && faces === 0) {
      continue;
    }
    if (rules !== 1 || faces !== 1) {
      throw new GuideError(
        `d2 の SVG のフォント（${kind}）の書き方が、想定と違います（scripts/guide-lib.ts の systemFonts）`,
      );
    }
    out = out.replace(rule, `$1${SYSTEM_FONTS};${style}`).replace(face, "");
  }
  // 知らない種類のフォントが残っていれば、埋め込んだまま配らないよう失敗させる。図の文字（<text>）は見ず、<style> の中だけを見る。
  const styles = out.match(/<style[^>]*>[\s\S]*?<\/style>/g) ?? [];
  if (styles.some((style) => /@font-face|d2-\d+-font-/.test(style))) {
    throw new GuideError("d2 の SVG に、取り除けなかったフォントがあります（scripts/guide-lib.ts の systemFonts）");
  }
  return out;
}

/** 名前（印と図）に使える文字。 */
const NAME = /^[a-z0-9][a-z0-9-]*$/;

/** 元の文章で使った図の名前。 */
export interface Diagram {
  name: string;
  alt: string;
}

/**
 * 元の Markdown の印を、Markdown に置き換える。`readFile` は、リポジトリの中のパスのファイルの中身を返す。
 * 使った図の一覧も返す。
 */
export function expandDirectives(
  markdown: string,
  readFile: (path: string) => string,
): { markdown: string; diagrams: Diagram[] } {
  const diagrams: Diagram[] = [];
  // コードのブロック（``` か ~~~ で囲んだ所）の中は、印として読まない（印の書き方の例を書けるように）。
  let fence: string | undefined;
  const lines = markdown.split(/\r?\n/).flatMap((line) => {
    const trimmed = line.trim();
    const marker = /^(`{3,}|~{3,})/.exec(trimmed)?.[1];
    if (fence !== undefined) {
      if (marker !== undefined && marker[0] === fence[0] && marker.length >= fence.length && trimmed === marker) {
        fence = undefined;
      }
      return [line];
    }
    if (marker !== undefined) {
      fence = marker;
      return [line];
    }
    if (!trimmed.startsWith("<!--")) {
      return [line];
    }
    const code = /^<!--\s*code:\s*([^#\s]+)#(\S+)\s*-->$/.exec(trimmed);
    if (code) {
      const [, path, name] = code as unknown as [string, string, string];
      checkName(name);
      checkPath(path);
      const snippet = extractSnippet(readFile(path), path, name);
      const language = LANGUAGES[path.split(".").pop() ?? ""] ?? "";
      const codeFence = snippet.includes("```") ? "````" : "```";
      return [`[${path}](${REPOSITORY_BLOB}${path}) から:`, "", `${codeFence}${language}`, snippet, codeFence];
    }
    const diagram = /^<!--\s*diagram:\s*(\S+)\s*\|\s*(.+?)\s*-->$/.exec(trimmed);
    if (diagram) {
      const [, name, alt] = diagram as unknown as [string, string, string];
      checkName(name);
      diagrams.push({ name, alt });
      // 図はページの幅に合わせて縮むので、押すと元の大きさで開けるようにする。
      const src = `${DIAGRAM_PATH}${name}.svg`;
      return [`[![${alt.replace(/[[\]]/g, "")}](${src})](${src})`];
    }
    if (/^<!--\s*(code|diagram)\b/.test(trimmed)) {
      throw new GuideError(`印の書き方が違います: ${trimmed}`);
    }
    // ほかのコメント（メモなど）は、ページに出さない。生の HTML を書かせない（html: false）ので、残すと文字として出るためである。
    // 1 行に収まるコメントだけを書ける。
    if (!trimmed.endsWith("-->")) {
      throw new GuideError(`コメントは 1 行に収めてください: ${trimmed}`);
    }
    return [];
  });
  if (fence !== undefined) {
    throw new GuideError("閉じていないコードのブロックがあります");
  }
  return { markdown: lines.join("\n"), diagrams };
}

function checkName(name: string): void {
  if (!NAME.test(name)) {
    throw new GuideError(`名前 ${name} に使えない文字があります（英小文字・数字・- だけ）`);
  }
}

/** リポジトリの中の相対パスか（外に出るパス・絶対パスを断る）。 */
export function checkPath(path: string): void {
  if (path.startsWith("/") || path.split("/").some((part) => part === ".." || part === "." || part === "")) {
    throw new GuideError(`リポジトリの中の相対パスにしてください: ${path}`);
  }
}

/** 見出しの文字から、ページの中のリンクの ID を作る。空白を - にし、記号を外す。 */
export function slug(text: string): string {
  return text
    .normalize("NFKC")
    .toLowerCase()
    .replace(/[\s　]+/g, "-")
    .replace(/[^\p{L}\p{N}-]/gu, "")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
}

/**
 * GitHub が Markdown の見出しに付ける ID（リンクの # の後ろ）。小文字にし、文字・数字・空白・-・_ 以外を外し、空白を - にする。
 * 同じ見出しが 2 つ以上あるときの番号（-1 など）は付けない。
 */
export function githubSlug(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, "")
    .replace(/\s/g, "-");
}

/** Markdown の文書の見出しの、GitHub の ID の一覧。コードのブロックの中の # の行は数えない。 */
export function markdownAnchors(markdown: string): Set<string> {
  const anchors = new Set<string>();
  let inFence = false;
  for (const line of markdown.split(/\r?\n/)) {
    if (/^\s*(```|~~~)/.test(line)) {
      inFence = !inFence;
      continue;
    }
    const heading = /^#{1,6}\s+(.+?)\s*#*\s*$/.exec(line);
    if (!inFence && heading) {
      anchors.add(githubSlug(heading[1] ?? ""));
    }
  }
  return anchors;
}

/** 見出しなどの inline の token から、表に出る文字だけを取り出す（`code` の記号やリンクの先を外す）。 */
function plainText(children: { type: string; content: string }[] | null): string {
  return (children ?? [])
    .filter((child) => child.type === "text" || child.type === "code_inline")
    .map((child) => child.content)
    .join("");
}

/** 目次に出す見出し。 */
export interface Heading {
  level: 2 | 3;
  id: string;
  text: string;
}

/**
 * Markdown を HTML にする。生の HTML は書かせない（html: false）。
 * 見出しに ID を付け、`repo:<パス>` のリンクを公開リポジトリの main のファイルへのリンクにする（`exists` で、ファイルがあるかを確かめる）。
 * `repo:<パス>#<見出し>` のリンクは、その Markdown の文書（`read` で読む）に、GitHub の ID がその見出しがあるかも確かめる。
 */
export function renderMarkdown(
  markdown: string,
  exists: (path: string) => boolean,
  read: (path: string) => string,
): { html: string; title: string; headings: Heading[] } {
  const md = new MarkdownIt({ html: false, linkify: false, typographer: false });
  // 幅の広い表とコードは、ページの全体ではなくその部分だけを横に動かす。
  // キーボードだけでも動かせるよう tabindex を付け、何の部分かを読み上げられるよう名前を付ける。
  md.renderer.rules.table_open = () =>
    '<div class="scroll" tabindex="0" role="region" aria-label="表（横に動かせる）">\n<table>\n';
  md.renderer.rules.table_close = () => "</table>\n</div>\n";
  const fence = md.renderer.rules.fence;
  md.renderer.rules.fence = (tokens, idx, options, env, self) =>
    (fence?.(tokens, idx, options, env, self) ?? "").replace(
      /^<pre>/,
      '<pre class="scroll" tabindex="0" role="region" aria-label="コード（横に動かせる）">',
    );
  const tokens = md.parse(markdown, {});
  const headings: Heading[] = [];
  const used = new Set<string>();
  const anchors: string[] = [];
  let title = "";
  tokens.forEach((token, i) => {
    // 引用するコードの前の「<ファイル> から:」の段落に、見た目を分けるクラスを付ける（expandDirectives が作る行）。
    if (token.type === "paragraph_open" && CITE_LINE.test(tokens[i + 1]?.content ?? "")) {
      token.attrSet("class", "cite");
    }
    if (token.type === "heading_open") {
      const text = plainText(tokens[i + 1]?.children ?? null);
      const level = Number(token.tag.slice(1));
      if (level === 1) {
        title = text;
        return;
      }
      let id = slug(text) || `section-${i}`;
      while (used.has(id)) {
        id = `${id}-2`;
      }
      used.add(id);
      token.attrSet("id", id);
      if (level === 2 || level === 3) {
        headings.push({ level, id, text });
      }
    }
    for (const child of token.children ?? []) {
      if (child.type !== "link_open") {
        continue;
      }
      const href = String(child.attrGet("href") ?? "");
      if (href.startsWith("repo:")) {
        const [path, hash] = href.slice("repo:".length).split("#") as [string, string | undefined];
        checkPath(path);
        if (!exists(path)) {
          throw new GuideError(`リンクの先のファイルがありません: ${path}`);
        }
        if (hash !== undefined) {
          if (!path.endsWith(".md")) {
            throw new GuideError(`# の付いたリンクは、Markdown の文書にだけ書けます: ${path}#${hash}`);
          }
          if (!markdownAnchors(read(path)).has(decodeURIComponent(hash))) {
            throw new GuideError(`リンクの先の見出しがありません: ${path}#${hash}`);
          }
        }
        child.attrSet("href", `${REPOSITORY_BLOB}${path}${hash ? `#${hash}` : ""}`);
      } else if (href.startsWith("#")) {
        anchors.push(decodeURIComponent(href.slice(1)));
      } else if (!/^(https:\/\/|\/)/.test(href)) {
        throw new GuideError(`リンクは https:・#・/ か、repo:<パス> にしてください: ${href}`);
      }
    }
  });
  if (title === "") {
    throw new GuideError("元の文章に、# の見出し（ページの題）がありません");
  }
  const missing = anchors.filter((anchor) => !used.has(anchor));
  if (missing.length > 0) {
    throw new GuideError(`ページの中のリンクの先の見出しがありません: ${missing.join(", ")}`);
  }
  const body = md.renderer.render(
    tokens.filter(
      (token, i) =>
        token.tag !== "h1" && !(token.type === "inline" && tokens[i - 1]?.type === "heading_open" && tokens[i - 1]?.tag === "h1"),
    ),
    md.options,
    {},
  );
  return { html: body, title, headings };
}

/** HTML の文字として出すために、特別な文字を置き換える。 */
export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

/** ページの全体の HTML を作る。CSP でインラインのスタイルとスクリプトを禁じるので、どちらも使わない。 */
export function renderPage(page: { title: string; html: string; headings: Heading[] }): string {
  const toc = page.headings
    .map((h) => `          <li class="toc-${h.level}"><a href="#${h.id}">${escapeHtml(h.text)}</a></li>`)
    .join("\n");
  return `<!doctype html>
<!-- このファイルは mise run guide が guide/about.md から作る。手で直さない（docs/development.md）。 -->
<html lang="ja">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>${escapeHtml(page.title)} - recipe-box</title>
    <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
    <link rel="stylesheet" href="/assets/style.css">
    <link rel="stylesheet" href="/assets/guide.css">
  </head>
  <body class="guide">
    <header>
      <a class="brand" href="/">recipe-box</a>
      <nav aria-label="サイト"><a href="/">レシピ帳に戻る</a></nav>
    </header>
    <div class="guide-layout">
      <nav class="toc" aria-labelledby="toc-title">
        <p class="toc-title" id="toc-title">目次</p>
        <ol>
${toc}
        </ol>
      </nav>
      <main>
        <h1>${escapeHtml(page.title)}</h1>
        <article>
${page.html.trimEnd()}
        </article>
      </main>
    </div>
  </body>
</html>
`;
}
