// scripts/guide-lib.ts の単体テスト（mise run test）。
import { describe, expect, test } from "vitest";

import {
  boldFallback,
  checkPath,
  diagramSources,
  githubSlug,
  expandDirectives,
  extractSnippet,
  GuideError,
  REPOSITORY_BLOB,
  renderMarkdown,
  renderPage,
  slug,
} from "./guide-lib.ts";

describe("extractSnippet", () => {
  const rust = [
    "fn other() {}",
    "    // guide:start key",
    "    pub fn key() -> u8 {",
    "        1",
    "    }",
    "    // guide:end key",
  ].join("\n");

  test("印の間を、共通の字下げを外して取り出す", () => {
    expect(extractSnippet(rust, "a.rs", "key")).toBe("pub fn key() -> u8 {\n    1\n}");
  });

  test.each([
    ["# guide:start x\nvalue = 1\n# guide:end x", "value = 1"],
    ["{#- guide:start x #}\n<p>a</p>\n{#- guide:end x #}", "<p>a</p>"],
    ["<!-- guide:start x -->\n<p>a</p>\n<!-- guide:end x -->", "<p>a</p>"],
    ["/* guide:start x */\na {}\n/* guide:end x */", "a {}"],
  ])("TOML・askama・HTML・CSS のコメントの印も読む", (text, expected) => {
    expect(extractSnippet(text, "f", "x")).toBe(expected);
  });

  test("タブの字下げも外す", () => {
    expect(extractSnippet("\t// guide:start key\n\tif a {\n\t\tb\n\t}\n\t// guide:end key", "f", "key")).toBe("if a {\n\tb\n}");
  });

  test("名前が前方で一致するだけの印は、別の印とみなす", () => {
    const text = "// guide:start key-2\na\n// guide:end key-2\n// guide:start key\nb\n// guide:end key";
    expect(extractSnippet(text, "f", "key")).toBe("b");
  });

  test.each([
    ["印が無い", "fn a() {}"],
    ["end が無い", "// guide:start key\nfn a() {}"],
    ["start が 2 つ", "// guide:start key\na\n// guide:start key\nb\n// guide:end key"],
    ["中身が空", "// guide:start key\n// guide:end key"],
    ["end が前", "// guide:end key\na\n// guide:start key"],
    ["中身が空の行だけ", "// guide:start key\n\n  \n// guide:end key"],
  ])("%s なら、生成を止める", (_, text) => {
    expect(() => extractSnippet(text, "f", "key")).toThrow(GuideError);
  });
});

describe("expandDirectives", () => {
  const files: Record<string, string> = {
    "src/a.rs": "// guide:start one\nlet a = 1;\n// guide:end one",
  };
  const read = (path: string) => {
    const text = files[path];
    if (text === undefined) {
      throw new GuideError(`無い: ${path}`);
    }
    return text;
  };

  test("コードの印を、言語と出典のリンクの付いたブロックにする", () => {
    const { markdown } = expandDirectives("前\n<!-- code: src/a.rs#one -->\n後", read);
    expect(markdown).toBe(
      `前\n[src/a.rs](${REPOSITORY_BLOB}src/a.rs) から:\n\n\`\`\`rust\nlet a = 1;\n\`\`\`\n後`,
    );
  });

  test("図の印を画像にし、使った図を返す", () => {
    const result = expandDirectives("<!-- diagram: flow | リクエストの流れ -->", read);
    expect(result.markdown).toBe("[![リクエストの流れ](/assets/guide/flow.svg)](/assets/guide/flow.svg)");
    expect(result.diagrams).toEqual([{ name: "flow", alt: "リクエストの流れ" }]);
  });

  test("ほかのコメントは、ページに出さない", () => {
    expect(expandDirectives("前\n<!-- メモ -->\n後", read).markdown).toBe("前\n後");
  });

  test("コードのブロックの中の印は、そのまま残す", () => {
    const markdown = "```md\n<!-- code: <パス>#<名前> -->\n<!-- メモ -->\n```";
    expect(expandDirectives(markdown, read).markdown).toBe(markdown);
  });

  test.each([
    ["書き方が違う印", "<!-- code: src/a.rs -->"],
    ["説明の無い図", "<!-- diagram: flow -->"],
    ["大文字の名前", "<!-- diagram: Flow | x -->"],
    ["リポジトリの外のパス", "<!-- code: ../a.rs#one -->"],
    ["無いファイル", "<!-- code: src/b.rs#one -->"],
    ["2 行にわたるコメント", "<!-- メモ\n-->"],
    ["閉じていないコードのブロック", "```\na"],
  ])("%s なら、生成を止める", (_, line) => {
    expect(() => expandDirectives(line, read)).toThrow(GuideError);
  });
});

describe("diagramSources", () => {
  test("_ で始まる共通の設定と、.d2 でないファイルを除く", () => {
    expect(diagramSources(["b.d2", "_style.d2", "about.md", "a.d2"])).toEqual(["a.d2", "b.d2"]);
  });
});

describe("boldFallback", () => {
  const svg = [
    '.d2-1 .text-bold {\n\tfont-family: "d2-1-font-bold";\n}',
    '@font-face {\n\tfont-family: d2-1-font-bold;\n\tsrc: url("data:x");\n}',
  ].join("\n");

  test("太字の class とフォントの両方に、太さを書く", () => {
    expect(boldFallback(svg)).toBe(
      [
        '.d2-1 .text-bold {\n\tfont-family: "d2-1-font-bold"; font-weight: 700;\n}',
        '@font-face {\n\tfont-family: d2-1-font-bold; font-weight: 700;\n\tsrc: url("data:x");\n}',
      ].join("\n"),
    );
  });

  test("太字を使わない SVG は変えない", () => {
    expect(boldFallback("<svg></svg>")).toBe("<svg></svg>");
  });

  test("書き方が想定と違えば失敗する", () => {
    expect(() => boldFallback('.d2-1 .text-bold { font-family: "d2-1-font-bold"; }')).toThrow(GuideError);
  });
});

describe("checkPath", () => {
  test.each(["/etc/passwd", "a/../b", "./a", "a//b"])("%s を断る", (path) => {
    expect(() => checkPath(path)).toThrow(GuideError);
  });
});

describe("githubSlug", () => {
  test.each([
    ["技術構成（リライト前）", "技術構成リライト前"],
    ["写真の API", "写真の-api"],
    ["`Send` の扱い", "send-の扱い"],
  ])("%s → %s", (text, expected) => {
    expect(githubSlug(text)).toBe(expected);
  });
});

describe("slug", () => {
  test.each([
    ["Cloudflare の各サービス", "cloudflare-の各サービス"],
    ["D1（SQLite）と SQL", "d1sqliteと-sql"],
    ["  前後の空白  ", "前後の空白"],
  ])("%s → %s", (text, expected) => {
    expect(slug(text)).toBe(expected);
  });
});

describe("renderMarkdown", () => {
  const exists = (path: string) => path === "docs/spec.md" || path === "src/a.rs";
  const read = () => "# 仕様\n\n## 画面\n\n### 写真の API\n\n```\n## コードの中\n```";

  test("題を取り出し、見出しに ID を付けて目次を返す", () => {
    const result = renderMarkdown("# 題\n\n## 一\n\n### 二\n\n#### 三\n\n## 一", exists, read);
    expect(result.title).toBe("題");
    expect(result.html).not.toContain("<h1");
    expect(result.html).toContain('<h2 id="一">');
    expect(result.html).toContain('<h4 id="三">');
    expect(result.headings).toEqual([
      { level: 2, id: "一", text: "一" },
      { level: 3, id: "二", text: "二" },
      { level: 2, id: "一-2", text: "一" },
    ]);
  });

  test("repo: のリンクを、公開リポジトリの main のファイルへのリンクにする", () => {
    const { html } = renderMarkdown("# 題\n\n[仕様](repo:docs/spec.md#画面)", exists, read);
    expect(html).toContain(`href="${REPOSITORY_BLOB}docs/spec.md#%E7%94%BB%E9%9D%A2"`);
  });

  test("GitHub の ID の形で、見出しへのリンクを確かめる", () => {
    expect(renderMarkdown("# 題\n\n[a](repo:docs/spec.md#写真の-api)", exists, read).html).toContain("docs/spec.md#");
  });

  test("目次と題には、見出しの表に出る文字だけを使う", () => {
    const result = renderMarkdown("# `A` の題\n\n## `Send` と [リンク](#send-と-リンク)\n", exists, read);
    expect(result.title).toBe("A の題");
    expect(result.headings[0]?.text).toBe("Send と リンク");
  });

  test("引用するコードの前の行に、cite のクラスを付ける", () => {
    const { html } = renderMarkdown(
      `# 題\n\n[src/a.rs](${REPOSITORY_BLOB}src/a.rs) から:\n\n\`\`\`rust\na\n\`\`\`\n\nふつうの段落\n`,
      exists,
      read,
    );
    expect(html).toContain('<p class="cite"><a href');
    expect(html).toContain("<p>ふつうの段落</p>");
  });

  test("ページの中のリンクは、ある見出しなら通す", () => {
    expect(renderMarkdown("# 題\n\n## 一\n\n[a](#一)", exists, read).html).toContain('href="#%E4%B8%80"');
  });

  test("表とコードを、キーボードで横に動かせる部分で包む", () => {
    const { html } = renderMarkdown("# 題\n\n| a |\n| --- |\n| b |\n\n```rust\nlet a = 1;\n```", exists, read);
    expect(html).toContain('<div class="scroll" tabindex="0" role="region" aria-label="表（横に動かせる）">\n<table>');
    expect(html).toContain('<pre class="scroll" tabindex="0" role="region" aria-label="コード（横に動かせる）"><code class="language-rust">');
  });

  test("生の HTML は、文字として出す", () => {
    expect(renderMarkdown("# 題\n\n<script>x</script>", exists, read).html).toContain("&lt;script&gt;");
  });

  test.each([
    ["無いファイルへのリンク", "# 題\n\n[a](repo:docs/none.md)"],
    ["相対パスのリンク", "# 題\n\n[a](docs/spec.md)"],
    ["http のリンク", "# 題\n\n[a](http://example.com)"],
    ["題が無い", "## 一"],
    ["無い見出しへのリンク", "# 題\n\n[a](repo:docs/spec.md#ない)"],
    ["コードのブロックの中の # へのリンク", "# 題\n\n[a](repo:docs/spec.md#コードの中)"],
    ["Markdown でない文書の # へのリンク", "# 題\n\n[a](repo:src/a.rs#L1)"],
    ["無い見出しへのページの中のリンク", "# 題\n\n## 一\n\n[a](#二)"],
  ])("%s なら、生成を止める", (_, markdown) => {
    expect(() => renderMarkdown(markdown, exists, read)).toThrow(GuideError);
  });
});

describe("renderPage", () => {
  test("目次を左、本文を右に置く入れ物と、レシピ帳に戻るリンクを入れる", () => {
    const html = renderPage({ title: "題", html: "<p>本文</p>\n", headings: [] });
    expect(html).toContain('<nav aria-label="サイト"><a href="/">レシピ帳に戻る</a></nav>');
    expect(html).toMatch(/<div class="guide-layout">\s*<nav class="toc" aria-labelledby="toc-title">/);
    expect(html).toContain('<p class="toc-title" id="toc-title">目次</p>');
    expect(html).toMatch(/<main>\s*<h1>題<\/h1>\s*<article>\s*<p>本文<\/p>/);
  });

  test("題と目次を入れ、インラインのスタイルとスクリプトを使わない", () => {
    const html = renderPage({ title: "A & B", html: "<p>本文</p>\n", headings: [{ level: 2, id: "x", text: "<見出し>" }] });
    expect(html).toContain("<title>A &#38; B - recipe-box</title>");
    expect(html).toContain('<a href="#x">&#60;見出し&#62;</a>');
    expect(html).not.toMatch(/<script|style=|<style/);
  });
});
