// public/assets/page.js の単体テスト（mise run test）。
// 判定の関数は直接呼び、happy-dom の DOM に画面を置いて確かめる。
import { afterEach, describe, expect, test, vi } from "vitest";

import { focusHeading, invokeOnClick, supportsInvokers } from "../public/assets/page.js";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("hx-boost の後のフォーカス", () => {
  test("フォーカスが body にあれば、見出しに移し、Tab ではたどらない", () => {
    document.body.innerHTML = "<header><a href='/'>recipe-box</a></header><main><h1>レシピの一覧</h1></main>";
    expect(focusHeading(document)).toBe(true);
    const heading = document.querySelector("h1") as HTMLElement;
    expect(document.activeElement).toBe(heading);
    expect(heading.tabIndex).toBe(-1);
  });

  test("autofocus などでフォーカスがほかにあれば、動かさない", () => {
    document.body.innerHTML = "<main><h1>ワークスペース</h1><input id='workspace-id' autofocus></main>";
    const input = document.getElementById("workspace-id") as HTMLInputElement;
    input.focus();
    expect(focusHeading(document)).toBe(false);
    expect(document.activeElement).toBe(input);
  });

  test("見出しが無ければ、何もしない", () => {
    document.body.innerHTML = "<main><p>本文</p></main>";
    expect(focusHeading(document)).toBe(false);
    expect(document.activeElement).toBe(document.body);
  });
});

describe("Invoker Commands に対応していないブラウザでのダイアログ", () => {
  test("supportsInvokers は、HTMLButtonElement の commandForElement で見分ける", () => {
    expect(supportsInvokers({ commandForElement: null } as unknown as HTMLButtonElement)).toBe(true);
    expect(supportsInvokers({} as unknown as HTMLButtonElement)).toBe(false);
  });

  /** delete_dialog.html の形の画面を置き、ダイアログの開閉を差し替える。 */
  function setUp() {
    document.body.innerHTML = `
      <button type="button" class="text danger" commandfor="delete-r1" command="show-modal" aria-label="「カレー」を削除">削除</button>
      <dialog id="delete-r1" class="confirm" aria-labelledby="delete-r1-title" aria-describedby="delete-r1-text">
        <h2 id="delete-r1-title">レシピを削除しますか？</h2>
        <p id="delete-r1-text">「カレー」を削除します。元には戻せません。</p>
        <div class="actions">
          <button type="button" commandfor="delete-r1" command="close">やめる</button>
          <form method="post" action="/recipes/r1/delete"><button type="submit">削除する</button></form>
        </div>
      </dialog>`;
    const dialog = document.querySelector("dialog") as HTMLDialogElement;
    const showModal = vi.spyOn(dialog, "showModal").mockImplementation(() => {
      dialog.setAttribute("open", "");
    });
    const close = vi.spyOn(dialog, "close").mockImplementation(() => {
      dialog.removeAttribute("open");
    });
    return { dialog, showModal, close };
  }

  /** ボタン（の中の要素）を押したことにして、invokeOnClick に渡す。 */
  function click(selector: string): boolean {
    const target = document.querySelector(selector) as HTMLElement;
    const event = new MouseEvent("click", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "target", { value: target });
    return invokeOnClick(event);
  }

  test("show-modal のボタンでモーダルとして開き、close のボタンで閉じる", () => {
    const { showModal, close } = setUp();
    expect(click('[command="show-modal"]')).toBe(true);
    expect(showModal).toHaveBeenCalledTimes(1);
    // 開いている間に押しても、開き直さない。
    expect(click('[command="show-modal"]')).toBe(false);
    expect(click('[command="close"]')).toBe(true);
    expect(close).toHaveBeenCalledTimes(1);
    // 閉じている間に押しても、何もしない。
    expect(click('[command="close"]')).toBe(false);
  });

  test("ボタンの中の要素を押しても開く", () => {
    const { showModal } = setUp();
    const button = document.querySelector('[command="show-modal"]') as HTMLButtonElement;
    button.innerHTML = "<span>削除</span>";
    expect(click('[command="show-modal"] span')).toBe(true);
    expect(showModal).toHaveBeenCalledTimes(1);
  });

  test("対象が無いか <dialog> でないとき、命令が違うとき、無効のボタンでは何もしない", () => {
    const { showModal } = setUp();
    const button = document.querySelector('[command="show-modal"]') as HTMLButtonElement;
    button.setAttribute("command", "toggle-popover");
    expect(click('[command="toggle-popover"]')).toBe(false);
    button.setAttribute("command", "show-modal");
    button.disabled = true;
    expect(click('[command="show-modal"]')).toBe(false);
    button.disabled = false;
    button.setAttribute("commandfor", "missing");
    expect(click('[command="show-modal"]')).toBe(false);
    button.setAttribute("commandfor", "delete-r1-text");
    expect(click('[command="show-modal"]')).toBe(false);
    // 削除を送るボタン（commandfor の無いボタン）は扱わない。
    expect(click('button[type="submit"]')).toBe(false);
    expect(showModal).not.toHaveBeenCalled();
  });
});
