// どの画面にもかかわる、2 つの補い。
//
// 1. hx-boost で画面が移った後（戻る・進むで htmx が画面を読み直したときも）のフォーカスと知らせ。
//    htmx は、ページの本文を入れ替えるだけなので、押したリンクやボタンが消えると、フォーカスが body に戻る。
//    キーボードやスクリーンリーダーで使う人は、どこにいるか分からなくなり、ページの先頭から Tab でたどり直すことになる。
//    そこで、移った後にフォーカスが行き場を失っていれば、見出し（h1）に移す。autofocus のある画面（入力の誤りの一覧・
//    ワークスペースの ID の誤り）では、htmx がそこにフォーカスを移した後なので、何もしない。
//    本文ごと入れ替わると、知らせの入れ物（templates/base.html の #messages-status・#messages-alert）も、知らせごと新しく入る。
//    新しく入った入れ物の知らせ（「保存しました」など）は、読み上げないことがあるので、フォーカスを移した後に、
//    同じ中身を入れ直す（入れ物は前からあり、中身が変わった形にする）。
// 2. Invoker Commands（commandfor・command）に対応していないブラウザでの、削除の確認のダイアログの開閉
//    （templates/delete_dialog.html）。対応しているブラウザでは何もしない（ブラウザが開閉する）。
//    外部の polyfill は使わず、使っている 2 つの命令（show-modal・close）だけを <dialog> のメソッドで行う。
//
// ES モジュールとして読み込む（base.html の <script type="module">）。判定の関数は、単体テスト
// （apps/recipe-box-rs/js-tests/）のために export する。型は JSDoc で書き、`tsc --noEmit` で確かめる。
// @ts-check

// 知らせの入れ物（templates/base.html）。
const MESSAGE_REGIONS = ["messages-status", "messages-alert"];

// 知らせを入れ直すまでの間。フォーカスを移したときの読み上げ（見出し）と重ならないよう、少し待つ。
const ANNOUNCE_DELAY_MS = 100;

/**
 * フォーカスが行き場を失っていれば（body にあれば）、見出しに移す。移したら true を返す。
 * 写真の欄を入れ替えたとき（photo-upload.js）にも使う。
 * @param {Document} doc
 * @param {string} selector フォーカスを移す見出し
 * @param {FocusOptions} [options]
 * @returns {boolean}
 */
export function focusHeading(doc, selector, options) {
  if (doc.activeElement && doc.activeElement !== doc.body) {
    return false;
  }
  const heading = doc.querySelector(selector);
  if (!(heading instanceof HTMLElement)) {
    return false;
  }
  // 見出しは Tab でたどる対象にしない（-1）。スクリプトからだけフォーカスを移せるようにする。
  heading.tabIndex = -1;
  heading.focus(options);
  return true;
}

/**
 * 知らせの入れ物の中身を、同じ中身で入れ直す。入れ物ごと新しく入った知らせを、読み上げさせるため。
 * @param {Document} doc
 */
export function reannounceMessages(doc) {
  for (const id of MESSAGE_REGIONS) {
    const region = doc.getElementById(id);
    if (region && region.childNodes.length > 0) {
      region.replaceChildren(...Array.from(region.childNodes, (node) => node.cloneNode(true)));
    }
  }
}

/**
 * 画面の本文を入れ替えた後に、フォーカスを見出しに移し、知らせを読み上げさせる。
 * @param {Document} doc
 */
function afterPageSwap(doc) {
  // 画面の先頭へのスクロールは htmx が行うので、フォーカスでスクロールさせない。
  focusHeading(doc, "main h1", { preventScroll: true });
  setTimeout(() => reannounceMessages(doc), ANNOUNCE_DELAY_MS);
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

// htmx:afterSettle は、入れ替えた要素ごとに起きるので、本文（body）を入れ替えたときだけ扱う。
document.addEventListener("htmx:afterSettle", (event) => {
  if (event instanceof CustomEvent && event.detail?.boosted && event.target === document.body) {
    afterPageSwap(document);
  }
});
// 戻る・進むで、htmx がサーバーから画面を読み直して本文を入れ替えたとき（historyCacheSize が 0 なので、いつもこちら）。
document.addEventListener("htmx:historyRestore", () => afterPageSwap(document));

if (!supportsInvokers(HTMLButtonElement.prototype)) {
  document.addEventListener("click", invokeOnClick);
}
