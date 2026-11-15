// 写真の追加のフォーム（data-photo-upload）で、複数の写真を選べるようにし、1 枚ずつ送る。
//
// JavaScript が動かなければ、フォームは今までどおり multipart で 1 枚ずつ送る（docs/spec.md の「画面」）。
// 1 枚ずつ、写真のバイト列そのものを同じパスに送る。本文が 1 枚の写真の大きさに収まり、サーバーは本文を
// 読みためずに R2 へ流せる。サーバーは、追加できたら 204 を返す（フォームのときの 303 で詳細の画面に移らせない）。
// 送り終えたら詳細の画面を読み直し、何枚入ったかと、入らなかった写真の理由を出す。
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

  // ワークスペースが無いなど、続けて送っても追加できないときの印。
  const STOP = Symbol("stop");

  // 1 枚を送り、追加できたら null、できなければ理由の文を返す。続けても追加できなければ STOP を返す。
  async function sendOne(form, file) {
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
    if (response.status === 204) {
      return null;
    }
    // ワークスペースが無いとき（303 で最初の画面へ移る）など、エラーの画面でない応答は、残りも送らない。
    if (response.ok) {
      return STOP;
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

  // 詳細の画面を読み直して、結果を出す。htmx があれば、ページの移動と同じく本文だけを入れ替える。
  // 読み直せなければ、今の画面のまま結果を出す（追加した写真は、ページを読み直すと出る）。
  async function finish(form, detailPath, added, failures) {
    // 送っている間に、ほかの画面に移っていれば、その画面を入れ替えない。
    if (window.location.pathname !== detailPath) {
      return;
    }
    // 読み直せなかったときのために、先にフォームを使える状態に戻す（読み直せば、フォームごと入れ替わる）。
    form.dataset.remaining = String(Math.max(0, Number(form.dataset.remaining) - added));
    for (const control of form.querySelectorAll("input, button")) {
      control.disabled = false;
    }
    const input = form.querySelector('input[type="file"]');
    if (input) {
      input.value = "";
    }
    setStatus(form, "送り終えました。ページを読み直すと、追加した写真が出ます");
    try {
      if (window.htmx) {
        await window.htmx.ajax("GET", detailPath, { target: "body", swap: "innerHTML" });
      }
    } catch {
      // 今の画面のまま結果を出す。
    }
    showResults(added, failures);
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

  // 送らずに断る写真の理由。サーバーも同じ上限で断るので、送る手間を省くだけである。
  function localReason(form, file) {
    if (file.size === 0) {
      return "空のファイルは追加できません";
    }
    if (file.size > Number(form.dataset.maxBytes)) {
      return form.dataset.tooLarge;
    }
    return null;
  }

  async function upload(form, files) {
    const failures = [];
    const sending = [];
    for (const file of files) {
      const reason = localReason(form, file);
      if (reason === null) {
        sending.push(file);
      } else {
        failures.push([file.name, reason]);
      }
    }
    const remaining = Number(form.dataset.remaining);
    if (sending.length > remaining) {
      // 送らずに断る写真があれば、それも一緒に知らせる（選び直すときに除けるように）。
      const others = failures.map(([name, reason]) => `${name}: ${reason}`).join("。");
      setStatus(
        form,
        `写真はあと ${remaining} 枚まで追加できます。選んだ写真を減らしてください${others ? `。ほかに、追加できない写真があります（${others}）` : ""}`,
      );
      return;
    }
    // 詳細の画面のパス。フォームの送り先（/recipes/<id>/photos）の親である。
    const detailPath = new URL(form.action).pathname.replace(/\/photos\/?$/, "");
    for (const control of form.querySelectorAll("input, button")) {
      control.disabled = true;
    }
    let added = 0;
    for (const [index, file] of sending.entries()) {
      setStatus(form, `写真を送っています（${index + 1} / ${sending.length} 枚目）`);
      const reason = await sendOne(form, file);
      if (reason === STOP) {
        for (const rest of sending.slice(index)) {
          failures.push([rest.name, "追加できませんでした。ページを読み直してください"]);
        }
        break;
      }
      if (reason === null) {
        added += 1;
      } else {
        failures.push([file.name, reason]);
      }
    }
    await finish(form, detailPath, added, failures);
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
