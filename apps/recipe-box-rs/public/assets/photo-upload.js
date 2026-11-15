// 写真の追加のフォーム（data-photo-upload）で、複数の写真を選べるようにし、1 枚ずつ送る。
//
// JavaScript が動かなければ、フォームは今までどおり multipart で 1 枚ずつ送る（docs/spec.md の「画面」）。
// 1 枚ずつ、写真のバイト列そのものを同じパスに送る。本文が 1 枚の写真の大きさに収まり、サーバーは本文を
// 読みためずに R2 へ流せる。送り終えたら詳細の画面を読み直し、何枚入ったかと、入らなかった写真の理由を出す。
"use strict";

(() => {
  const FORM = "form[data-photo-upload]";

  // ファイルの欄で複数を選べるようにする。htmx がページを入れ替えたときにも付け直す。
  function enableMultiple(root) {
    for (const input of root.querySelectorAll(`${FORM} input[type="file"]`)) {
      input.multiple = true;
    }
  }

  // 失敗した応答の画面から、知らせ（赤のメッセージ）の文を取り出す。
  async function failureReason(response) {
    try {
      const html = await response.text();
      const page = new DOMParser().parseFromString(html, "text/html");
      const alert = page.querySelector('[role="alert"]');
      if (alert && alert.textContent.trim()) {
        return alert.textContent.trim();
      }
    } catch {
      // 文を取り出せなければ、決まった文にする。
    }
    return "追加できませんでした";
  }

  // 1 枚を送り、追加できたら null、できなければ理由の文を返す。
  async function sendOne(form, file, detailPath) {
    let response;
    try {
      response = await fetch(form.action, {
        method: "POST",
        body: file,
        headers: { "Content-Type": file.type || "application/octet-stream" },
      });
    } catch {
      return "通信できませんでした";
    }
    // 追加できると、303 で詳細の画面に移る。fetch はその移動を追うので、移った先が詳細の画面で 200 なら成功。
    // ワークスペースが無いとき（303 で最初の画面へ）や、エラーの画面（4xx・5xx）は失敗にする。
    if (response.ok && response.redirected && new URL(response.url).pathname === detailPath) {
      return null;
    }
    if (response.ok) {
      return "追加できませんでした";
    }
    return failureReason(response);
  }

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

  // 見出しの下のメッセージを、送った結果に置き換える。
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
      messages.push(
        message(
          "error",
          `${failures.length} 枚の写真を追加できませんでした`,
          failures.map(([name, reason]) => `${name}: ${reason}`),
        ),
      );
    }
    head.after(...messages);
  }

  // 詳細の画面を読み直す。htmx があれば、ページの移動と同じく本文だけを入れ替える。
  async function reload(detailPath) {
    if (window.htmx) {
      await window.htmx.ajax("GET", detailPath, { target: "body", swap: "innerHTML" });
      return true;
    }
    window.location.assign(detailPath);
    return false;
  }

  function setStatus(form, text) {
    let status = form.querySelector("[data-photo-status]");
    if (!status) {
      status = document.createElement("p");
      status.className = "note";
      status.setAttribute("role", "status");
      status.dataset.photoStatus = "";
      form.append(status);
    }
    status.textContent = text;
  }

  async function upload(form, files) {
    const remaining = Number(form.dataset.remaining);
    const maxBytes = Number(form.dataset.maxBytes);
    if (files.length > remaining) {
      setStatus(form, `写真はあと ${remaining} 枚まで追加できます。選んだ ${files.length} 枚から減らしてください`);
      return;
    }
    const detailPath = new URL(form.action).pathname.replace(/\/photos$/, "");
    const controls = form.querySelectorAll("input, button");
    for (const control of controls) {
      control.disabled = true;
    }
    let added = 0;
    const failures = [];
    for (const [index, file] of files.entries()) {
      setStatus(form, `写真を送っています（${index + 1} / ${files.length} 枚目）`);
      // 大きすぎる写真は、送る前に断る（サーバーも同じ上限で断る）。
      const reason = file.size > maxBytes ? form.dataset.tooLarge : await sendOne(form, file, detailPath);
      if (reason === null) {
        added += 1;
      } else {
        failures.push([file.name, reason]);
      }
    }
    try {
      if (await reload(detailPath)) {
        showResults(added, failures);
      }
    } catch {
      window.location.assign(detailPath);
    }
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
      const files = input ? Array.from(input.files) : [];
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

  document.addEventListener("htmx:load", (event) => enableMultiple(event.target));
  enableMultiple(document);
})();
