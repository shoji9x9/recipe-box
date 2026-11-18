// public/assets/page.js の単体テスト（mise run test）。
// 判定の関数は直接呼び、happy-dom の DOM に画面を置いて確かめる。
import { afterEach, describe, expect, test, vi } from "vitest";

import {
  cancelPendingImages,
  focusHeading,
  invokeOnClick,
  reannounceMessages,
  restoreImages,
  supportsInvokers,
} from "../public/assets/page.js";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("hx-boost の後のフォーカス", () => {
  test("フォーカスが body にあれば、見出しに移し、Tab ではたどらない", () => {
    document.body.innerHTML = "<header><a href='/'>recipe-box</a></header><main><h1>レシピの一覧</h1></main>";
    expect(focusHeading(document, "main h1")).toBe(true);
    const heading = document.querySelector("h1") as HTMLElement;
    expect(document.activeElement).toBe(heading);
    expect(heading.tabIndex).toBe(-1);
  });

  test("autofocus などでフォーカスがほかにあれば、動かさない", () => {
    document.body.innerHTML = "<main><h1>ワークスペース</h1><input id='workspace-id' autofocus></main>";
    const input = document.getElementById("workspace-id") as HTMLInputElement;
    input.focus();
    expect(focusHeading(document, "main h1")).toBe(false);
    expect(document.activeElement).toBe(input);
  });

  test("hx-boost で本文を入れ替えたら、見出しにフォーカスを移し、少し後に知らせを同じ中身で入れ直す", async () => {
    vi.useFakeTimers();
    try {
      document.body.innerHTML = `<main><h1>カレー</h1><div id="messages">
        <div id="messages-status" role="status"><p class="message success">レシピを保存しました</p></div>
        <div id="messages-alert" role="alert"></div></div></main>`;
      const status = document.getElementById("messages-status") as HTMLElement;
      const before = status.firstChild;
      document.body.dispatchEvent(new CustomEvent("htmx:afterSettle", { bubbles: true, detail: { boosted: true } }));
      expect(document.activeElement?.textContent).toBe("カレー");
      // フォーカスの読み上げと重ならないよう、知らせはすぐには入れ直さない。
      expect(status.firstChild).toBe(before);
      vi.runAllTimers();
      expect(status.firstChild).not.toBe(before);
      expect(status.textContent).toBe("レシピを保存しました");
      expect(document.getElementById("messages-status")).toBe(status);
    } finally {
      vi.useRealTimers();
    }
  });

  test("一部だけを入れ替えたとき（検索・欄の確かめ直し）は、何もしない", () => {
    document.body.innerHTML = "<main><h1>レシピの一覧</h1><section id='results'></section></main>";
    const results = document.getElementById("results") as HTMLElement;
    results.dispatchEvent(new CustomEvent("htmx:afterSettle", { bubbles: true, detail: { boosted: false } }));
    expect(document.activeElement).toBe(document.body);
  });

  test("戻る・進むで htmx が画面を読み直したときも、見出しにフォーカスを移す", () => {
    document.body.innerHTML = "<main><h1>レシピの一覧</h1></main>";
    document.body.dispatchEvent(new CustomEvent("htmx:historyRestore", { bubbles: true, detail: { path: "/" } }));
    expect(document.activeElement?.textContent).toBe("レシピの一覧");
  });

  test("reannounceMessages は、空の入れ物には触れない", () => {
    document.body.innerHTML = '<div id="messages-status" role="status"></div><div id="messages-alert" role="alert">誤り</div>';
    const alert = document.getElementById("messages-alert") as HTMLElement;
    const before = alert.firstChild;
    reannounceMessages(document);
    expect(document.getElementById("messages-status")?.childNodes).toHaveLength(0);
    expect(alert.firstChild).not.toBe(before);
    expect(alert.textContent).toBe("誤り");
  });

  test("見出しが無ければ、何もしない", () => {
    document.body.innerHTML = "<main><p>本文</p></main>";
    expect(focusHeading(document, "main h1")).toBe(false);
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

describe("htmx で入れ替えるときの、読み込み中の写真の取りやめ", () => {
  /**
   * 写真を 3 枚置く。1 枚目は読み終えたもの、2 枚目は読み込み中のもの（srcset のある小さな版）、
   * 3 枚目は入れ替える先（#results）の外の読み込み中のもの。happy-dom は写真を読まないので、complete を決めて置く。
   * src を外せば complete は true になる（ブラウザと同じ）。
   */
  function placeImages() {
    document.body.innerHTML = `<main><h1>レシピの一覧</h1><section id="results">
      <img id="done" src="/recipes/r1/photos/p1" alt="">
      <img id="loading" src="/recipes/r2/photos/p2/thumbnail" srcset="/recipes/r2/photos/p2/thumbnail 1280w, /recipes/r2/photos/p2 4000w" alt=""></section>
      <img id="outside" src="/recipes/r3/photos/p3" alt=""></main>`;
    const done = document.getElementById("done") as HTMLImageElement;
    const loading = document.getElementById("loading") as HTMLImageElement;
    const outside = document.getElementById("outside") as HTMLImageElement;
    for (const img of [done, loading, outside]) {
      Object.defineProperty(img, "complete", { get: () => img === done || !img.hasAttribute("src") });
    }
    return { done, loading, outside, results: document.getElementById("results") as HTMLElement };
  }

  function send(target: Element, xhr: XMLHttpRequest) {
    target.dispatchEvent(new CustomEvent("htmx:beforeSend", { bubbles: true, detail: { target, xhr } }));
  }

  test("入れ替える先の、読み終えていない写真だけ src と srcset を外して隠し、戻せる", () => {
    const { done, loading, outside, results } = placeImages();
    const cancelled = cancelPendingImages(results);
    expect(cancelled).toHaveLength(1);
    expect(done.getAttribute("src")).toBe("/recipes/r1/photos/p1");
    expect(outside.getAttribute("src")).toBe("/recipes/r3/photos/p3");
    expect(loading.hasAttribute("src")).toBe(false);
    expect(loading.hasAttribute("srcset")).toBe(false);
    expect(loading.hasAttribute("data-load-cancelled")).toBe(true);

    restoreImages(cancelled);
    expect(loading.getAttribute("src")).toBe("/recipes/r2/photos/p2/thumbnail");
    expect(loading.getAttribute("srcset")).toBe("/recipes/r2/photos/p2/thumbnail 1280w, /recipes/r2/photos/p2 4000w");
    expect(loading.hasAttribute("data-load-cancelled")).toBe(false);
  });

  test("画面から外れた写真（入れ替えで消えたもの）には、src を戻さない", () => {
    const { loading, results } = placeImages();
    const cancelled = cancelPendingImages(results);
    loading.remove();
    restoreImages(cancelled);
    expect(loading.hasAttribute("src")).toBe(false);
  });

  test("要求を送るときに取りやめ、入れ替わらずに要求が終われば（回線が切れたなど）戻す", () => {
    const { loading } = placeImages();
    const xhr = new XMLHttpRequest();
    send(document.body, xhr);
    expect(loading.hasAttribute("src")).toBe(false);
    xhr.dispatchEvent(new Event("loadend"));
    expect(loading.getAttribute("src")).toBe("/recipes/r2/photos/p2/thumbnail");
  });

  test("要求が並んでも、それぞれが取りやめた写真を、それぞれの終わりで戻す", () => {
    const { loading, outside, results } = placeImages();
    const first = new XMLHttpRequest();
    const second = new XMLHttpRequest();
    send(results, first);
    send(document.body, second);
    expect(loading.hasAttribute("src")).toBe(false);
    expect(outside.hasAttribute("src")).toBe(false);
    // 2 つ目の終わりでは、2 つ目が取りやめた写真（#results の外）だけを戻す。
    second.dispatchEvent(new Event("loadend"));
    expect(outside.getAttribute("src")).toBe("/recipes/r3/photos/p3");
    expect(loading.hasAttribute("src")).toBe(false);
    first.dispatchEvent(new Event("loadend"));
    expect(loading.getAttribute("src")).toBe("/recipes/r2/photos/p2/thumbnail");
  });

  test("戻る・進むで htmx が画面を読み直すときも取りやめ、読めなければ戻す", () => {
    const { loading } = placeImages();
    const xhr = new XMLHttpRequest();
    document.body.dispatchEvent(
      new CustomEvent("htmx:historyCacheMiss", { bubbles: true, detail: { historyElt: document.body, xhr } }),
    );
    expect(loading.hasAttribute("src")).toBe(false);
    xhr.dispatchEvent(new Event("loadend"));
    expect(loading.getAttribute("src")).toBe("/recipes/r2/photos/p2/thumbnail");
  });
});
