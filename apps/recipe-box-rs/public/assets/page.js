// どの画面にもかかわる、2 つの補い。
//
// 1. hx-boost で画面が移った後のフォーカス。
//    htmx は、ページの本文を入れ替えるだけなので、押したリンクやボタンが消えると、フォーカスが body に戻る。
//    キーボードやスクリーンリーダーで使う人は、どこにいるか分からなくなり、ページの先頭から Tab でたどり直すことになる。
//    そこで、移った後にフォーカスが行き場を失っていれば、見出し（h1）に移す。autofocus のある画面（入力の誤りの一覧・
//    ワークスペースの ID の誤り）では、htmx がそこにフォーカスを移した後なので、何もしない。
// 2. Invoker Commands（commandfor・command）に対応していないブラウザでの、削除の確認のダイアログの開閉
//    （templates/delete_dialog.html）。対応しているブラウザでは何もしない（ブラウザが開閉する）。
//    外部の polyfill は使わず、使っている 2 つの命令（show-modal・close）だけを <dialog> のメソッドで行う。
//
// ES モジュールとして読み込む（base.html の <script type="module">）。判定の関数は、単体テスト
// （apps/recipe-box-rs/js-tests/）のために export する。型は JSDoc で書き、`tsc --noEmit` で確かめる。
// @ts-check

/**
 * hx-boost で画面が移った後、フォーカスが行き場を失っていれば（body にあれば）、見出し（h1）に移す。
 * 移したら true を返す。
 * @param {Document} doc
 * @returns {boolean}
 */
export function focusHeading(doc) {
  if (doc.activeElement && doc.activeElement !== doc.body) {
    return false;
  }
  const heading = doc.querySelector("main h1");
  if (!(heading instanceof HTMLElement)) {
    return false;
  }
  // 見出しは Tab でたどる対象にしない（-1）。スクリプトからだけフォーカスを移せるようにする。
  heading.tabIndex = -1;
  // 画面の先頭へのスクロールは htmx が行うので、フォーカスでスクロールさせない。
  heading.focus({ preventScroll: true });
  return true;
}

/**
 * ブラウザが Invoker Commands に対応しているか。
 * @param {typeof HTMLButtonElement.prototype} prototype
 */
export function supportsInvokers(prototype) {
  return "commandForElement" in prototype;
}

/**
 * Invoker Commands のボタン（commandfor・command）が押されたら、対象の <dialog> を開閉する。
 * 開閉したら true を返す。対応していないブラウザでだけ使う。
 * @param {Event} event
 * @returns {boolean}
 */
export function invokeOnClick(event) {
  const button = event.target instanceof Element ? event.target.closest("button[commandfor][command]") : null;
  if (!(button instanceof HTMLButtonElement) || button.disabled) {
    return false;
  }
  const target = button.ownerDocument.getElementById(button.getAttribute("commandfor") ?? "");
  if (!(target instanceof HTMLDialogElement)) {
    return false;
  }
  const command = button.getAttribute("command")?.toLowerCase();
  if (command === "show-modal" && !target.open) {
    target.showModal();
    return true;
  }
  if (command === "close" && target.open) {
    target.close();
    return true;
  }
  return false;
}

document.addEventListener("htmx:afterSettle", (event) => {
  if (event instanceof CustomEvent && event.detail?.boosted) {
    focusHeading(document);
  }
});

if (!supportsInvokers(HTMLButtonElement.prototype)) {
  document.addEventListener("click", invokeOnClick);
}
