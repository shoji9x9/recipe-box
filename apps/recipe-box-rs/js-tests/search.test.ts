// public/assets/search.js の単体テスト（mise run test）。
// happy-dom の DOM に検索のフォームを置き、input・compositionend・click を起こして、search-input が起きるかを確かめる。
import { afterEach, beforeEach, describe, expect, test } from "vitest";

import { allowsRequest, searchesOnInput } from "../public/assets/search.js";

// 読み込んだときに、document に compositionstart・input・compositionend・htmx:confirm・click の受け手を付ける（モジュールの最後）。

/** templates/list.html と同じ形の検索のフォーム。 */
function render(value: string): { input: HTMLInputElement; clear: HTMLButtonElement; searches: string[] } {
  document.body.innerHTML = `
    <form method="get" action="/" role="search" class="search">
      <input id="q" type="search" name="q" placeholder="料理名で探す" data-search-input>
      <button type="submit" form="clear-search" class="clear" data-search-clear aria-label="検索の文字を消す"><svg aria-hidden="true"><path d=""/></svg></button>
      <button type="submit">探す</button>
    </form>
    <form id="clear-search" method="get" action="/" hidden></form>`;
  const input = document.querySelector<HTMLInputElement>("#q");
  const clear = document.querySelector<HTMLButtonElement>("[data-search-clear]");
  if (!input || !clear) {
    throw new Error("検索のフォームが無い");
  }
  input.value = value;
  const searches: string[] = [];
  input.addEventListener("search-input", () => searches.push(input.value));
  return { input, clear, searches };
}

/** 打ったときの input を起こす。composing なら変換中。 */
function type(input: HTMLInputElement, value: string, composing: boolean): void {
  input.value = value;
  input.dispatchEvent(new InputEvent("input", { bubbles: true, isComposing: composing }));
}

beforeEach(() => {
  document.body.innerHTML = "";
});
afterEach(() => {
  document.body.innerHTML = "";
});

describe("searchesOnInput", () => {
  test.each([
    { composing: false, value: "カレー", expected: true },
    { composing: false, value: "curry", expected: true },
    { composing: true, value: "だn", expected: false },
    { composing: true, value: "カレーcurry", expected: false },
    { composing: true, value: "curry", expected: true },
    { composing: true, value: "", expected: true },
  ])("変換中=$composing・値「$value」なら $expected", ({ composing, value, expected }) => {
    expect(searchesOnInput(new InputEvent("input", { isComposing: composing }), value)).toBe(expected);
  });

  test("InputEvent でない input は、変換中でないとみなす", () => {
    expect(searchesOnInput(new Event("input"), "だn")).toBe(true);
  });
});

describe("打ちながらの検索", () => {
  test("日本語は変換中に検索せず、確定したときに検索する", () => {
    const { input, searches } = render("");
    for (const value of ["お", "おn", "おに", "おにg", "おにぎ", "おにぎり"]) {
      type(input, value, true);
    }
    expect(searches).toEqual([]);
    input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "おにぎり" }));
    expect(searches).toEqual(["おにぎり"]);
  });

  test("変換中でない入力は、打つたびに検索する", () => {
    const { input, searches } = render("");
    type(input, "c", false);
    type(input, "cu", false);
    expect(searches).toEqual(["c", "cu"]);
  });

  test("英字だけなら、変換中（Android のキーボードなど）でも検索する", () => {
    const { input, searches } = render("");
    type(input, "cur", true);
    expect(searches).toEqual(["cur"]);
  });

  test("検索の欄でない入力では、検索しない", () => {
    const { searches } = render("");
    const other = document.createElement("input");
    document.body.append(other);
    other.dispatchEvent(new InputEvent("input", { bubbles: true }));
    other.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
    expect(searches).toEqual([]);
  });
});

describe("入力を消すボタン", () => {
  test("押すと、欄を空にして検索し、欄にフォーカスを戻す。フォームは送らない", () => {
    const { input, clear, searches } = render("カレー");
    const event = new MouseEvent("click", { bubbles: true, cancelable: true });
    clear.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(input.value).toBe("");
    expect(searches).toEqual([""]);
    expect(document.activeElement).toBe(input);
  });

  test("図形（SVG）を押しても、ボタンを押したのと同じ", () => {
    const { input, clear, searches } = render("カレー");
    const path = clear.querySelector("path");
    path?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(input.value).toBe("");
    expect(searches).toEqual([""]);
  });

  test("ほかのボタンでは、何もしない", () => {
    const { input, searches } = render("カレー");
    const submit = document.querySelector<HTMLButtonElement>("button:not([data-search-clear])");
    // 探すボタンのフォームの送信は、happy-dom でページを移らせないよう止める。
    submit?.addEventListener("click", (event) => event.preventDefault());
    submit?.click();
    expect(input.value).toBe("カレー");
    expect(searches).toEqual([]);
  });
});

describe("検索を送る直前の確かめ（htmx:confirm）", () => {
  test.each([
    { composing: false, value: "おにぎり", expected: true },
    { composing: true, value: "かr", expected: false },
    { composing: true, value: "k", expected: true },
  ])("変換中=$composing・値「$value」なら送る=$expected", ({ composing, value, expected }) => {
    expect(allowsRequest(composing, value)).toBe(expected);
  });

  /** htmx が検索を送る直前に起こす htmx:confirm を起こし、止められたかを返す。 */
  function confirm(input: HTMLInputElement): boolean {
    const event = new CustomEvent("htmx:confirm", { bubbles: true, cancelable: true });
    input.dispatchEvent(event);
    return event.defaultPrevented;
  }

  test("最初の英字で検索が始まっても、送る時に変換中の日本語なら送らず、確定したら検索する", () => {
    const { input, searches } = render("");
    input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    type(input, "k", true);
    expect(searches).toEqual(["k"]);
    // htmx が 300 ms 待つ間に、変換中の日本語になった。
    type(input, "かr", true);
    expect(confirm(input)).toBe(true);
    input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
    expect(searches).toEqual(["k", "かr"]);
    expect(confirm(input)).toBe(false);
  });
});
