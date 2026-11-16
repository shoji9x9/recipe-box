// 写真の追加のフォーム（data-photo-upload）で、複数の写真を選べるようにし、1 枚ずつ送る。
//
// JavaScript が動かなければ、フォームは今までどおり multipart で 1 枚ずつ送る（docs/spec.md の「画面」）。
// 1 枚ずつ、写真のバイト列そのものを同じパスに送る。本文が 1 枚の写真の大きさに収まり、サーバーは本文を
// 読みためずに R2 へ流せる。サーバーは、追加できたら 204 を返す（フォームのときの 303 で詳細の画面に移らせない）。
// 送り終えたら詳細の画面を読み直し、何枚入ったかと、入らなかった写真の理由を出す。
//
// ES モジュールとして読み込む（base.html の <script type="module">）。判定の関数は、単体テスト
// （apps/recipe-box-rs/js-tests/）のために export する。型は JSDoc で書き、`tsc --noEmit` で確かめる。
// @ts-check

const FORM = "form[data-photo-upload]";

// その写真だけの誤り（読めない・大きすぎる・形式が違う）。ほかの写真は追加できるので、続けて送る。
// それ以外の失敗（上限・ワークスペースが無い・回数の制限・サーバーの障害など）は、続けても追加できないので、残りを送らない。
const PER_PHOTO_ERRORS = new Set([400, 413, 415]);

/** @typedef {[name: string, reason: string]} Failure 追加できなかった写真の、ファイル名と理由。 */

/**
 * 1 枚を送った結果。
 * - "added": 追加できた。
 * - "skip": その写真だけ追加できなかった。残りは続けて送る。
 * - "stop": 続けても追加できない。残りを送らない。
 * - "redirect": サーバーがほかの画面に移らせた（ワークスペースが無いなど）。残りを送らず、ページごと読み直す。
 * @typedef {{ kind: "added", reason: null } | { kind: "skip" | "stop" | "redirect", reason: string }} SendResult
 */

/**
 * ファイルの欄で複数を選べるようにする。htmx がページを入れ替えたときにも付け直す。
 * @param {ParentNode} root
 */
function enableMultiple(root) {
  for (const input of root.querySelectorAll(`${FORM} input[type="file"]`)) {
    if (input instanceof HTMLInputElement) {
      input.multiple = true;
    }
  }
}

/**
 * @param {HTMLFormElement} form
 * @param {boolean} disabled
 */
function setControlsDisabled(form, disabled) {
  for (const control of form.querySelectorAll("input, button")) {
    if (control instanceof HTMLInputElement || control instanceof HTMLButtonElement) {
      control.disabled = disabled;
    }
  }
}

/**
 * 追加できなかった写真の一覧を、「ファイル名: 理由」の文にする。
 * @param {Failure[]} failures
 * @returns {string[]}
 */
export function describeFailures(failures) {
  return failures.map(([name, reason]) => `${name}: ${reason}`);
}

/**
 * 失敗した応答の画面から、知らせ（赤のメッセージ）の文を取り出す。
 * @param {Response} response
 * @returns {Promise<string>}
 */
export async function failureReason(response) {
  try {
    const html = await response.text();
    const page = new DOMParser().parseFromString(html, "text/html");
    const text = page.querySelector('[role="alert"]')?.textContent?.trim();
    if (text) {
      return text;
    }
  } catch {
    // 文を取り出せなければ、決まった文にする。
  }
  return "追加できませんでした";
}

/**
 * 応答を、1 枚を送った結果に分ける。
 * @param {Response} response
 * @returns {Promise<SendResult>}
 */
export async function classify(response) {
  if (response.status === 204) {
    return { kind: "added", reason: null };
  }
  if (response.redirected) {
    return { kind: "redirect", reason: "追加できませんでした" };
  }
  const reason = await failureReason(response);
  return { kind: PER_PHOTO_ERRORS.has(response.status) ? "skip" : "stop", reason };
}

/**
 * 1 枚を送る。
 * @param {HTMLFormElement} form
 * @param {File} file
 * @returns {Promise<SendResult>}
 */
async function sendOne(form, file) {
  let response;
  try {
    response = await fetch(form.action, {
      method: "POST",
      body: file,
      headers: { "Content-Type": file.type || "application/octet-stream" },
    });
  } catch {
    return { kind: "stop", reason: "通信できませんでした" };
  }
  return classify(response);
}

/**
 * @param {"success" | "error"} kind
 * @param {string} text
 * @param {string[]} items
 */
function message(kind, text, items) {
  const element = document.createElement(items.length > 0 ? "div" : "p");
  element.className = `message ${kind}`;
  element.setAttribute("role", kind === "error" ? "alert" : "status");
  if (items.length === 0) {
    element.textContent = text;
    return element;
  }
  const lead = document.createElement("p");
  lead.textContent = text;
  const list = document.createElement("ul");
  for (const item of items) {
    const li = document.createElement("li");
    li.textContent = item;
    list.append(li);
  }
  element.append(lead, list);
  return element;
}

/**
 * 見出しの下のメッセージを、送った結果に置き換える。
 * @param {number} added
 * @param {Failure[]} failures
 */
function showResults(added, failures) {
  const head = document.querySelector("main .page-head");
  if (!head) {
    return;
  }
  for (const old of document.querySelectorAll("main > .message")) {
    old.remove();
  }
  const messages = [];
  if (added > 0) {
    messages.push(message("success", `${added} 枚の写真を追加しました`, []));
  }
  if (failures.length > 0) {
    messages.push(message("error", `${failures.length} 枚の写真を追加できませんでした`, describeFailures(failures)));
  }
  head.after(...messages);
}

/**
 * 詳細の画面を読み直して、結果を出す。htmx があれば、ページの移動と同じく本文だけを入れ替える。
 * @param {HTMLFormElement} form
 * @param {string} detailPath
 * @param {number} added
 * @param {Failure[]} failures
 * @param {boolean} redirected
 */
async function finish(form, detailPath, added, failures, redirected) {
  // 送っている間に、ほかの画面に移っていれば、その画面を入れ替えない。
  if (window.location.pathname !== detailPath) {
    return;
  }
  // サーバーがほかの画面に移らせたときは、ページごと読み直し、移った先の画面とアドレスをそろえる。
  if (redirected) {
    window.location.assign(detailPath);
    return;
  }
  // window.htmx の型は apps/recipe-box-rs/js-types/window.d.ts で宣言している。
  const htmx = window.htmx;
  try {
    if (htmx) {
      await htmx.ajax("GET", detailPath, { target: "body", swap: "innerHTML" });
    }
  } catch {
    // 今の画面のまま結果を出す。
  }
  // 読み直せなかったとき（htmx が無いなど）は、フォームが残っているので、使える状態に戻す。
  if (form.isConnected) {
    form.dataset.remaining = String(Math.max(0, Number(form.dataset.remaining) - added));
    clearFileInput(form);
    setControlsDisabled(form, false);
    setStatus(form, added > 0 ? "送り終えました。ページを読み直すと、追加した写真が出ます" : "送り終えました");
  }
  showResults(added, failures);
}

/**
 * 選び直せるよう、ファイルの欄を空にする。
 * @param {HTMLFormElement} form
 */
function clearFileInput(form) {
  const input = form.querySelector('input[type="file"]');
  if (input instanceof HTMLInputElement) {
    input.value = "";
  }
}

/**
 * @param {HTMLFormElement} form
 * @param {string} text
 */
function setStatus(form, text) {
  let status = form.querySelector("[data-photo-status]");
  if (!status) {
    status = document.createElement("p");
    status.className = "note";
    status.setAttribute("role", "status");
    status.setAttribute("data-photo-status", "");
    form.append(status);
  }
  status.textContent = text;
}

/**
 * 送らずに断る写真の理由。サーバーも同じ上限で断るので、送る手間を省くだけである。
 * @param {{ maxBytes: number, tooLarge: string }} limits フォームの data-max-bytes と data-too-large。
 * @param {{ size: number }} file
 * @returns {string | null}
 */
export function localReason(limits, file) {
  if (file.size === 0) {
    return "空のファイルは追加できません";
  }
  if (file.size > limits.maxBytes) {
    return limits.tooLarge;
  }
  return null;
}

/**
 * 詳細の画面のパス。フォームの送り先（/recipes/<id>/photos）の親である。
 * @param {string} action フォームの送り先の URL。
 */
export function detailPathOf(action) {
  return new URL(action).pathname.replace(/\/photos\/?$/, "");
}

/**
 * 選んだ写真を 1 枚ずつ送り、結果を出す。テストが終わりを待てるよう、export する。
 * @param {HTMLFormElement} form
 * @param {File[]} files
 */
export async function upload(form, files) {
  /** @type {Failure[]} */
  const failures = [];
  /** @type {File[]} */
  const sending = [];
  // 上限（data-max-bytes）か、大きすぎるときの文（data-too-large）が無ければ、大きさは確かめずにサーバーに任せる
  // （理由をサーバーの知らせとそろえるため）。
  const maxBytes = Number(form.dataset.maxBytes || Number.NaN);
  const tooLarge = form.dataset.tooLarge;
  const limits =
    tooLarge && Number.isFinite(maxBytes)
      ? { maxBytes, tooLarge }
      : { maxBytes: Number.POSITIVE_INFINITY, tooLarge: "" };
  for (const file of files) {
    const reason = localReason(limits, file);
    if (reason === null) {
      sending.push(file);
    } else {
      failures.push([file.name, reason]);
    }
  }
  const remaining = Number(form.dataset.remaining);
  if (sending.length > remaining) {
    // 送らずに断る写真があれば、それも一緒に知らせる（選び直すときに除けるように）。
    const others = describeFailures(failures).join("。");
    setStatus(
      form,
      `写真はあと ${remaining} 枚まで追加できます。選んだ写真を減らしてください${others ? `。ほかに、追加できない写真があります（${others}）` : ""}`,
    );
    return;
  }
  // 1 枚も送らなければ、詳細の画面は変わらないので、読み直さずに理由だけを出し、選び直せるよう欄を空にする。
  if (sending.length === 0) {
    clearFileInput(form);
    setStatus(form, "追加できる写真がなかったので、送りませんでした");
    showResults(0, failures);
    return;
  }
  const detailPath = detailPathOf(form.action);
  setControlsDisabled(form, true);
  let added = 0;
  let redirected = false;
  for (const [index, file] of sending.entries()) {
    setStatus(form, `写真を送っています（${index + 1} / ${sending.length} 枚目）`);
    const result = await sendOne(form, file);
    if (result.kind === "added") {
      added += 1;
      continue;
    }
    failures.push([file.name, result.reason]);
    if (result.kind !== "skip") {
      for (const rest of sending.slice(index + 1)) {
        failures.push([rest.name, "前の写真を追加できなかったため、送りませんでした"]);
      }
      redirected = result.kind === "redirect";
      break;
    }
  }
  await finish(form, detailPath, added, failures, redirected);
}

// htmx（hx-boost）より先に受け取るため、捕捉の段階で聞く。
document.addEventListener(
  "submit",
  (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement) || !form.matches(FORM)) {
      return;
    }
    const input = form.querySelector('input[type="file"]');
    const files = input instanceof HTMLInputElement && input.files ? Array.from(input.files) : [];
    // 写真を選んでいなければ、今までどおりフォームとして送る（サーバーが「選んでください」と答える）。
    if (files.length === 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    upload(form, files);
  },
  true,
);

document.addEventListener("htmx:load", (event) => {
  if (event.target instanceof Element || event.target instanceof Document) {
    enableMultiple(event.target);
  }
});
enableMultiple(document);
