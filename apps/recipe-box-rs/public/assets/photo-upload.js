// 写真の追加のフォーム（data-photo-upload）で、複数の写真を選べるようにし、1 枚ずつ送る。
//
// JavaScript が動かなければ、フォームは今までどおり multipart で 1 枚ずつ送る（docs/spec.md の「画面」）。
// 1 枚ずつ、写真のバイト列そのものを同じパスに送る。本文が 1 枚の写真の大きさに収まり、サーバーは本文を
// 読みためずに R2 へ流せる。サーバーは、追加できたら 204 を返す（フォームのときの 303 で詳細の画面に移らせない）。
// 送り終えたら詳細の画面を読み直し、何枚入ったかと、入らなかった写真の理由を出す。
"use strict";

(() => {
  const FORM = "form[data-photo-upload]";

  // その写真だけの誤り（読めない・大きすぎる・形式が違う）。ほかの写真は追加できるので、続けて送る。
  // それ以外の失敗（上限・ワークスペースが無い・回数の制限・サーバーの障害など）は、続けても追加できないので、残りを送らない。
  const PER_PHOTO_ERRORS = new Set([400, 413, 415]);

  // ファイルの欄で複数を選べるようにする。htmx がページを入れ替えたときにも付け直す。
  function enableMultiple(root) {
    for (const input of root.querySelectorAll(`${FORM} input[type="file"]`)) {
      input.multiple = true;
    }
  }

  function setControlsDisabled(form, disabled) {
    for (const control of form.querySelectorAll("input, button")) {
      control.disabled = disabled;
    }
  }

  // 追加できなかった写真の一覧を、「ファイル名: 理由」の文にする。
  function describe(failures) {
    return failures.map(([name, reason]) => `${name}: ${reason}`);
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

  // 1 枚を送る。結果の kind は次のどれか。reason は、追加できなかった理由の文。
  // - "added": 追加できた。
  // - "skip": その写真だけ追加できなかった。残りは続けて送る。
  // - "stop": 続けても追加できない。残りを送らない。
  // - "redirect": サーバーがほかの画面に移らせた（ワークスペースが無いなど）。残りを送らず、ページごと読み直す。
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
    if (response.status === 204) {
      return { kind: "added", reason: null };
    }
    if (response.redirected) {
      return { kind: "redirect", reason: "追加できませんでした" };
    }
    const reason = await failureReason(response);
    return { kind: PER_PHOTO_ERRORS.has(response.status) ? "skip" : "stop", reason };
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
      messages.push(message("error", `${failures.length} 枚の写真を追加できませんでした`, describe(failures)));
    }
    head.after(...messages);
  }

  // 詳細の画面を読み直して、結果を出す。htmx があれば、ページの移動と同じく本文だけを入れ替える。
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
    try {
      if (window.htmx) {
        await window.htmx.ajax("GET", detailPath, { target: "body", swap: "innerHTML" });
      }
    } catch {
      // 今の画面のまま結果を出す。
    }
    // 読み直せなかったとき（htmx が無いなど）は、フォームが残っているので、使える状態に戻す。
    if (form.isConnected) {
      form.dataset.remaining = String(Math.max(0, Number(form.dataset.remaining) - added));
      const input = form.querySelector('input[type="file"]');
      if (input) {
        input.value = "";
      }
      setControlsDisabled(form, false);
      setStatus(form, added > 0 ? "送り終えました。ページを読み直すと、追加した写真が出ます" : "送り終えました");
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
      const others = describe(failures).join("。");
      setStatus(
        form,
        `写真はあと ${remaining} 枚まで追加できます。選んだ写真を減らしてください${others ? `。ほかに、追加できない写真があります（${others}）` : ""}`,
      );
      return;
    }
    // 詳細の画面のパス。フォームの送り先（/recipes/<id>/photos）の親である。
    const detailPath = new URL(form.action).pathname.replace(/\/photos\/?$/, "");
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
