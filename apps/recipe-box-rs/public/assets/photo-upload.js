// 編集の画面の写真の欄（data-photo-section）で、写真の追加と削除を送り、写真の欄だけを入れ替える。
//
// JavaScript が動かなければ、追加も削除も普通のフォームとして送り、編集の画面に戻る（docs/spec.md の「画面」）。
// そのときは、書きかけの料理名などが消えるので、写真の欄に先に保存するよう書いている。
// JavaScript が動けば、ページを読み直さず、写真の欄（#photos）と知らせ（#messages）だけを入れ替える。
// 料理名などのフォームはそのまま残るので、書きかけの入力は消えない。
//
// 追加: ファイルの欄で複数を選べるようにし、1 枚ずつ、写真のバイト列そのものを同じパスに送る。
// 本文が 1 枚の写真の大きさに収まり、サーバーは本文を読みためずに R2 へ流せる。サーバーは、追加できたら 204 を返す
// （フォームのときの 303 で画面を移らせない）。送り終えたら写真の欄を読み直し、何枚入ったかと、入らなかった写真の理由を出す。
// 削除: 確認のダイアログのフォームを fetch で送る。サーバーは編集の画面へ移らせるので、移った先の画面から入れ替える。
//
// ES モジュールとして読み込む（base.html の <script type="module">）。判定の関数は、単体テスト
// （apps/recipe-box-rs/js-tests/）のために export する。型は JSDoc で書き、`tsc --noEmit` で確かめる。
// @ts-check

const SECTION = "[data-photo-section]";
const FORM = "form[data-photo-upload]";
const SECTION_ID = "photos";
const MESSAGES_ID = "messages";

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
 * ファイルの欄で複数を選べるようにする。htmx がページを入れ替えたときと、写真の欄を入れ替えたときにも付け直す。
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
 * HTML の文を、画面として読む。
 * @param {string} html
 * @returns {Document}
 */
function parsePage(html) {
  return new DOMParser().parseFromString(html, "text/html");
}

/**
 * 失敗した応答の画面から、知らせ（赤のメッセージ）の文を取り出す。
 * @param {Response} response
 * @returns {Promise<string>}
 */
export async function failureReason(response) {
  try {
    const text = parsePage(await response.text())
      .querySelector('[role="alert"]')
      ?.textContent?.trim();
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
 * 見出しの下の知らせ（#messages）を、送った結果に置き換える。
 * @param {number} added
 * @param {Failure[]} failures
 */
function showResults(added, failures) {
  const messages = document.getElementById(MESSAGES_ID);
  if (!messages) {
    return;
  }
  const items = [];
  if (added > 0) {
    items.push(message("success", `${added} 枚の写真を追加しました`, []));
  }
  if (failures.length > 0) {
    items.push(message("error", `${failures.length} 枚の写真を追加できませんでした`, describeFailures(failures)));
  }
  messages.replaceChildren(...items);
}

/**
 * 読んだ画面から、知らせ（#messages）と写真の欄（#photos）を今の画面に写す。料理名などのフォームには触れない。
 * 写真の欄を入れ替えたかを返す（エラーの画面には写真の欄が無い）。
 * @param {Document} page
 * @param {{ messages: boolean }} options messages が false なら、知らせは写さない（呼ぶ側が結果を出す）。
 * @returns {boolean}
 */
export function replaceFrom(page, options) {
  if (options.messages) {
    const next = page.getElementById(MESSAGES_ID);
    // 知らせは、今ある #messages の中身だけを入れ替える。要素ごと入れ替えると、中の role="status" が
    // 新しく入った要素になり、スクリーンリーダーが読み上げないことがある。
    if (next) {
      document.getElementById(MESSAGES_ID)?.replaceChildren(...Array.from(next.childNodes, (node) => document.importNode(node, true)));
    }
  }
  const next = page.getElementById(SECTION_ID);
  const current = document.getElementById(SECTION_ID);
  if (!next || !current) {
    return false;
  }
  const imported = document.importNode(next, true);
  current.replaceWith(imported);
  enableMultiple(imported);
  return true;
}

/**
 * 入れ替えた後、フォーカスが行き場を失っていれば（押したボタンやダイアログが消えた）、写真の欄の見出しに移す。
 */
function restoreFocus() {
  if (document.activeElement && document.activeElement !== document.body) {
    return;
  }
  const heading = document.querySelector(`#${SECTION_ID} h2`);
  if (heading instanceof HTMLElement) {
    heading.tabIndex = -1;
    heading.focus();
  }
}

/**
 * 編集の画面のパス。写真のフォームの送り先（/recipes/<id>/photos と /recipes/<id>/photos/<photo_id>/delete）から求める。
 * @param {string} action フォームの送り先の URL。
 */
export function editPathOf(action) {
  return new URL(action).pathname.replace(/\/photos(?:\/.*)?$/, "/edit");
}

/**
 * 写真の欄を読み直して、結果を出す。
 * @param {HTMLFormElement} form
 * @param {string} editPath
 * @param {number} added
 * @param {Failure[]} failures
 * @param {boolean} redirected
 */
async function finish(form, editPath, added, failures, redirected) {
  // 送っている間に、ほかの画面に移っていれば、その画面を書き換えない。
  if (window.location.pathname !== editPath) {
    return;
  }
  // サーバーがほかの画面に移らせたときは、ページごと読み直し、移った先の画面とアドレスをそろえる。
  if (redirected) {
    window.location.assign(editPath);
    return;
  }
  let refreshed = false;
  try {
    const response = await fetch(editPath);
    if (response.ok) {
      const page = parsePage(await response.text());
      // 読み直している間に、ほかの画面に移っていれば、その画面を書き換えない。
      if (window.location.pathname !== editPath) {
        return;
      }
      refreshed = replaceFrom(page, { messages: false });
    }
  } catch {
    // 今の写真の欄のまま結果を出す。
  }
  if (window.location.pathname !== editPath) {
    return;
  }
  // 読み直せなかったときは、フォームが残っているので、使える状態に戻す。
  if (!refreshed && form.isConnected) {
    const remaining = count(form.dataset.remaining);
    if (remaining !== null) {
      form.dataset.remaining = String(Math.max(0, remaining - added));
    }
    clearFileInput(form);
    setControlsDisabled(form, false);
    setStatus(form, added > 0 ? "送り終えました。ページを読み直すと、追加した写真が出ます" : "送り終えました");
  }
  showResults(added, failures);
  restoreFocus();
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
 * サーバーがフォームに書いた数（data-max-bytes・data-remaining）を読む。0 以上の整数の形でなければ null を返す。
 * 読めない値は、画面で確かめずにサーバーに任せる（上限の判定はすべてサーバーが行う。docs/security.md）。
 * @param {string | undefined} value
 * @returns {number | null}
 */
export function count(value) {
  return value !== undefined && /^\d+$/.test(value) ? Number(value) : null;
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
  // 上限（data-max-bytes）か、大きすぎるときの文（data-too-large）を読めなければ、大きさは確かめずにサーバーに任せる
  // （理由をサーバーの知らせとそろえるため）。
  const maxBytes = count(form.dataset.maxBytes);
  const tooLarge = form.dataset.tooLarge;
  const limits =
    tooLarge && maxBytes !== null && maxBytes > 0
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
  // 残りの枚数（data-remaining）を読めなければ、枚数は確かめずにサーバーに任せる。
  const remaining = count(form.dataset.remaining);
  if (remaining !== null && sending.length > remaining) {
    // 送らずに断る写真があれば、それも一緒に知らせる（選び直すときに除けるように）。
    const others = describeFailures(failures).join("。");
    setStatus(
      form,
      `写真はあと ${remaining} 枚まで追加できます。選んだ写真を減らしてください${others ? `。ほかに、追加できない写真があります（${others}）` : ""}`,
    );
    return;
  }
  // 1 枚も送らなければ、写真の欄は変わらないので、読み直さずに理由だけを出し、選び直せるよう欄を空にする。
  if (sending.length === 0) {
    clearFileInput(form);
    setStatus(form, "追加できる写真がなかったので、送りませんでした");
    showResults(0, failures);
    return;
  }
  const editPath = editPathOf(form.action);
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
  await finish(form, editPath, added, failures, redirected);
}

/**
 * 写真を消す（確認のダイアログのフォーム）。サーバーは編集の画面へ移らせるので、その画面から写真の欄と知らせを写す。
 * テストが終わりを待てるよう、export する。
 * @param {HTMLFormElement} form
 */
export async function remove(form) {
  const editPath = editPathOf(form.action);
  // フォームの欄（ダイアログの caller が足す hidden の欄）も、JavaScript が動かないときと同じく送る。
  const body = new URLSearchParams(Array.from(new FormData(form), ([name, value]) => [name, String(value)]));
  form.closest("dialog")?.close();
  setControlsDisabled(form, true);
  /** @type {Document | null} */
  let page = null;
  let response;
  try {
    response = await fetch(form.action, {
      method: "POST",
      body,
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
    });
    page = parsePage(await response.text());
  } catch {
    // 下で、通信できなかったと知らせる。
  }
  // 送っている間に、ほかの画面に移っていれば、その画面を書き換えない。
  if (window.location.pathname !== editPath) {
    return;
  }
  const messages = document.getElementById(MESSAGES_ID);
  if (!response || !page) {
    setControlsDisabled(form, false);
    messages?.replaceChildren(message("error", "写真を削除できませんでした（通信できませんでした）", []));
    return;
  }
  const landed = response.redirected ? new URL(response.url).pathname : null;
  // 編集の画面でない所へ移らされた（ワークスペースが無いなど）ときは、ページごと読み直す。
  if (landed !== null && landed !== editPath) {
    window.location.assign(editPath);
    return;
  }
  if (replaceFrom(page, { messages: true })) {
    restoreFocus();
    return;
  }
  // 写真の欄の無い画面が返った。削除が済んで編集の画面に移らされた（303）のに、その画面を読めなかったときは、
  // 消した写真を欄から除き、済んだことを知らせる。削除の前に断られた（エラーの画面）ときは、知らせだけを写した。
  if (landed === editPath) {
    form.closest("li")?.remove();
    messages?.replaceChildren(message("success", "写真を削除しました。ページを読み直すと、写真の欄が新しくなります", []));
  } else {
    setControlsDisabled(form, false);
  }
  restoreFocus();
}

// htmx（hx-boost）より先に受け取るため、捕捉の段階で聞く。htmx に送らせると、ページの本文ごと入れ替わり、
// 書きかけの料理名などが消える。
document.addEventListener(
  "submit",
  (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement) || !form.closest(SECTION)) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (!form.matches(FORM)) {
      remove(form);
      return;
    }
    const input = form.querySelector('input[type="file"]');
    const files = input instanceof HTMLInputElement && input.files ? Array.from(input.files) : [];
    if (files.length === 0) {
      setStatus(form, "追加する写真を選んでください");
      return;
    }
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
