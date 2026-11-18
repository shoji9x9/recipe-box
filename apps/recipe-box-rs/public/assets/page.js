// どの画面にもかかわる、3 つの補い。
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
// 3. htmx で画面（hx-boost）や一覧（検索）を入れ替えるときに、入れ替える先で読み込み中の写真を取りやめる。
//    htmx は中身を入れ替えるだけなので、ブラウザは、入れ替える前の写真を読み続ける（入れ替えた後も止まらない）。
//    キャッシュの無い遅い回線では、その写真と次の画面の HTML が回線を取り合い、画面がすぐに切り替わらない。
//    次の画面の写真も、前の画面の写真を読み終えるまで待たされる。
//    そこで、要求を送るときに、読み終えていない写真の src と srcset を外す（ブラウザは、その読み込みを取りやめる）。
//    回線が切れたなどで入れ替わらなかったときは、外した src と srcset を戻す。
//
// ES モジュールとして読み込む（base.html の <script type="module">）。判定の関数は、単体テスト
// （apps/recipe-box-rs/js-tests/）のために export する。型は JSDoc で書き、`tsc --noEmit` で確かめる。
// @ts-check

/** @typedef {{ img: HTMLImageElement, src: string | null, srcset: string | null }} CancelledImage */

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

/**
 * 入れ替える先（root）の中の、読み終えていない写真の src と srcset を外し、読み込みを取りやめる。
 * 外したものを返す（戻すときに使う）。外した写真には data-load-cancelled を付け、CSS で見えなくする
 * （壊れた写真の印や、alt の文字を出さないため）。
 * @param {ParentNode} root
 * @returns {CancelledImage[]}
 */
export function cancelPendingImages(root) {
  /** @type {CancelledImage[]} */
  const cancelled = [];
  for (const img of root.querySelectorAll("img")) {
    // src を外した写真は complete が true になるので、続けて呼んでも二重には外さない。
    if (img.complete) {
      continue;
    }
    cancelled.push({ img, src: img.getAttribute("src"), srcset: img.getAttribute("srcset") });
    img.setAttribute("data-load-cancelled", "");
    // srcset を先に外す。src を先に外すと、残った srcset から別の版を読みに行くことがあるため。
    img.removeAttribute("srcset");
    img.removeAttribute("src");
  }
  return cancelled;
}

/**
 * cancelPendingImages で外した src と srcset を戻す。画面から外れた写真（入れ替えで消えたもの）には戻さない。
 * @param {CancelledImage[]} cancelled
 */
export function restoreImages(cancelled) {
  for (const { img, src, srcset } of cancelled) {
    if (!img.isConnected) {
      continue;
    }
    if (srcset !== null) {
      img.setAttribute("srcset", srcset);
    }
    if (src !== null) {
      img.setAttribute("src", src);
    }
    img.removeAttribute("data-load-cancelled");
  }
}

/**
 * 要求を送るときに、入れ替える先の読み込み中の写真を取りやめ、要求が終わったら（成功・失敗・中止・時間切れの
 * どれでも起きる loadend）、画面に残っている写真だけを戻す。
 * 入れ替えは htmx が load の中で済ませるので、入れ替わっていれば、外した写真は画面から外れていて、何も戻さない。
 * 入れ替わらなかったとき（回線が切れた・応答を入れ替えられなかったなど）は、写真が読み直される。
 * htmx のエラーのイベントは要求の種類によって起きないものがある（戻る・進むの読み直しは、回線の誤りで何も起こさない）ので、
 * XHR そのものの終わりで戻す。要求ごとに覚えるので、要求が並んでも、ほかの要求の終わりで戻さない。
 * @param {ParentNode} root
 * @param {XMLHttpRequest} xhr
 */
export function cancelImagesDuring(root, xhr) {
  const cancelled = cancelPendingImages(root);
  if (cancelled.length > 0) {
    xhr.addEventListener("loadend", () => restoreImages(cancelled), { once: true });
  }
}

// htmx の要求（hx-boost の画面の移動と、検索の一覧の入れ替え）を送るとき、入れ替える先の写真を取りやめる。
// 入れ替える先の写真は、入れ替えれば要らなくなり、読み続けると、応答と次の写真が回線を取り合って待たされる。
// htmx:beforeRequest は、ほかの処理が送るのを取りやめられ、写真だけが消えたままになりうるので使わない。
document.addEventListener("htmx:beforeSend", (event) => {
  const { target, xhr } = event instanceof CustomEvent ? (event.detail ?? {}) : {};
  if (target instanceof Element && xhr instanceof XMLHttpRequest) {
    cancelImagesDuring(target, xhr);
  }
});
// 戻る・進むで、htmx がサーバーから画面を読み直すとき（historyCacheSize が 0 なので、いつもこちら）。
document.addEventListener("htmx:historyCacheMiss", (event) => {
  const { historyElt, xhr } = event instanceof CustomEvent ? (event.detail ?? {}) : {};
  if (historyElt instanceof Element && xhr instanceof XMLHttpRequest) {
    cancelImagesDuring(historyElt, xhr);
  }
});

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
