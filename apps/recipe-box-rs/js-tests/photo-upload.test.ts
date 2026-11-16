// public/assets/photo-upload.js の単体テスト（mise run test）。
// 判定の関数は直接呼び、送る流れは、happy-dom の DOM に編集の画面を置いて submit を起こし、fetch を差し替えて確かめる。
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import {
  classify,
  count,
  describeFailures,
  editPathOf,
  failureReason,
  localReason,
  remove,
  replaceFrom,
  upload,
} from "../public/assets/photo-upload.js";

// 読み込んだときに、document に submit と htmx:load の受け手を付ける（モジュールの最後）。

const EDIT = "/recipes/r1/edit";
const TOO_LARGE = "写真は 4 MB までです";

/** 失敗の画面（知らせの文を role="alert" に置く）の応答。 */
function page(status: number, alert: string | null, redirected = false): Response {
  const body = alert === null ? "<main></main>" : `<main><p role="alert"> ${alert} </p></main>`;
  const response = new Response(status === 204 ? null : body, { status });
  Object.defineProperty(response, "redirected", { value: redirected });
  return response;
}

/** 写真の欄（photo_section.html の形）。photos は写真の ID、remaining は残りの枚数。 */
function section(photos: string[], remaining: number): string {
  const items = photos
    .map(
      (id) => `
      <li>
        <img src="/recipes/r1/photos/${id}" alt="カレーの写真">
        <dialog id="delete-photo-${id}" open>
          <form method="post" action="/recipes/r1/photos/${id}/delete"><button type="submit">削除する</button></form>
        </dialog>
      </li>`,
    )
    .join("");
  return `
    <section class="section" id="photos" data-photo-section>
      <h2 id="photos-title">写真</h2>
      <ul class="photos">${items}</ul>
      <form action="/recipes/r1/photos" method="post" enctype="multipart/form-data"
        data-photo-upload data-remaining="${remaining}" data-max-bytes="100" data-too-large="${TOO_LARGE}">
        <input type="file" name="photo">
        <button type="submit">追加</button>
      </form>
    </section>`;
}

/** 編集の画面。料理名などのフォームと、知らせ（#messages）と、写真の欄を持つ。 */
function editPage(message: string, photos: string[], remaining: number): string {
  return `
    <main>
      <div class="page-head"><h1>レシピを編集する</h1></div>
      <div id="messages" class="messages">${message}</div>
      <form class="recipe-form" method="post" action="/recipes/r1"><input name="name" value="カレー"></form>
      ${section(photos, remaining)}
    </main>`;
}

function html(status: number, body: string, url = `http://localhost:3000${EDIT}`, redirected = false): Response {
  const response = new Response(body, { status, headers: { "Content-Type": "text/html" } });
  Object.defineProperty(response, "redirected", { value: redirected });
  Object.defineProperty(response, "url", { value: url });
  return response;
}

function file(name: string, size: number, type = "image/jpeg"): File {
  return new File([new Uint8Array(size)], name, { type });
}

describe("判定の関数", () => {
  test("describeFailures は、ファイル名と理由を 1 つの文にする", () => {
    expect(
      describeFailures([
        ["a.jpg", "大きすぎます"],
        ["b.png", "形式が違います"],
      ]),
    ).toEqual(["a.jpg: 大きすぎます", "b.png: 形式が違います"]);
  });

  test("localReason は、空のファイルと上限を超えるファイルを断り、上限ちょうどは通す", () => {
    const limits = { maxBytes: 10, tooLarge: TOO_LARGE };
    expect(localReason(limits, { size: 0 })).toBe("空のファイルは追加できません");
    expect(localReason(limits, { size: 11 })).toBe(TOO_LARGE);
    expect(localReason(limits, { size: 10 })).toBeNull();
    expect(localReason(limits, { size: 1 })).toBeNull();
  });

  test("count は、0 以上の整数の形だけを読み、ほかは null にする", () => {
    expect(count("0")).toBe(0);
    expect(count("4000000")).toBe(4000000);
    for (const value of [undefined, "", " ", "-1", "1.5", "abc", "1e3"]) {
      expect(count(value)).toBeNull();
    }
  });

  test("editPathOf は、追加と削除の送り先から、編集の画面のパスにする", () => {
    expect(editPathOf("http://localhost:3000/recipes/r1/photos")).toBe(EDIT);
    expect(editPathOf("http://localhost:3000/recipes/r1/photos/")).toBe(EDIT);
    expect(editPathOf("http://localhost:3000/recipes/r1/photos/p1/delete")).toBe(EDIT);
  });

  test("classify は、204 を追加できたとする", async () => {
    expect(await classify(page(204, null))).toEqual({ kind: "added", reason: null });
  });

  test("classify は、ほかの画面に移らされたら、ページごと読み直す", async () => {
    expect(await classify(page(200, null, true))).toEqual({ kind: "redirect", reason: "追加できませんでした" });
  });

  test.each([400, 413, 415])("classify は、%i をその写真だけの誤りとし、画面の知らせを理由にする", async (status) => {
    expect(await classify(page(status, "形式が違います"))).toEqual({ kind: "skip", reason: "形式が違います" });
  });

  test.each([404, 409, 429, 500, 503])("classify は、%i で残りを送らない", async (status) => {
    expect(await classify(page(status, "上限です"))).toEqual({ kind: "stop", reason: "上限です" });
  });

  test("failureReason は、知らせが無いか読めなければ、決まった文にする", async () => {
    expect(await failureReason(page(500, null))).toBe("追加できませんでした");
    expect(await failureReason(page(500, "   "))).toBe("追加できませんでした");
    const broken = { text: () => Promise.reject(new Error("broken")) } as unknown as Response;
    expect(await failureReason(broken)).toBe("追加できませんでした");
  });
});

describe("画面の入れ替え", () => {
  beforeEach(() => {
    document.body.innerHTML = editPage('<p class="message success">前のメッセージ</p>', ["p1"], 4);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  test("replaceFrom は、知らせと写真の欄だけを写し、料理名などのフォームに触れない", () => {
    const name = document.querySelector('input[name="name"]') as HTMLInputElement;
    name.value = "書きかけ";
    const next = new DOMParser().parseFromString(
      editPage('<p class="message success">新しいメッセージ</p>', ["p1", "p2"], 3),
      "text/html",
    );
    expect(replaceFrom(next, { messages: true })).toBe(true);
    expect(document.getElementById("messages")?.textContent?.trim()).toBe("新しいメッセージ");
    expect(document.querySelectorAll("#photos img")).toHaveLength(2);
    expect(document.querySelector("input[name=name]")).toBe(name);
    expect(name.value).toBe("書きかけ");
    // 入れ替えた写真の欄でも、複数を選べる。
    expect((document.querySelector('#photos input[type="file"]') as HTMLInputElement).multiple).toBe(true);
  });

  test("replaceFrom は、messages が false なら知らせを写さない", () => {
    const next = new DOMParser().parseFromString(editPage("<p>新しい</p>", [], 5), "text/html");
    replaceFrom(next, { messages: false });
    expect(document.getElementById("messages")?.textContent?.trim()).toBe("前のメッセージ");
  });

  test("replaceFrom は、写真の欄が無い画面（エラーの画面）なら false を返し、写真の欄を残す", () => {
    const next = new DOMParser().parseFromString(
      '<main><div id="messages"><p role="alert">見つかりません</p></div></main>',
      "text/html",
    );
    expect(replaceFrom(next, { messages: true })).toBe(false);
    expect(document.getElementById("messages")?.textContent?.trim()).toBe("見つかりません");
    expect(document.querySelectorAll("#photos img")).toHaveLength(1);
  });
});

describe("追加の流れ", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    window.history.replaceState(null, "", EDIT);
    document.body.innerHTML = editPage('<p class="message success" role="status">前のメッセージ</p>', [], 3);
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  const form = () => document.querySelector("form[data-photo-upload]") as HTMLFormElement;
  const input = () => document.querySelector('input[type="file"]') as HTMLInputElement;
  const status = () => document.querySelector("[data-photo-status]")?.textContent ?? "";
  const messages = () =>
    Array.from(document.querySelectorAll("#messages > .message"), (element) => ({
      kind: element.className,
      role: element.getAttribute("role"),
      text: element.querySelector("p")?.textContent ?? element.textContent,
      items: Array.from(element.querySelectorAll("li"), (li) => li.textContent),
    }));

  /** 写真の POST には results を順に返し、編集の画面の GET には refreshed を返す。 */
  function respond(results: (Response | Error)[], refreshed: Response | Error = html(500, "")) {
    const queue = [...results];
    fetchMock.mockImplementation(async (_url: string, init?: RequestInit) => {
      const next = init?.method === "POST" ? (queue.shift() ?? page(204, null)) : refreshed;
      if (next instanceof Error) {
        throw next;
      }
      return next;
    });
  }

  const posts = () => fetchMock.mock.calls.filter(([, init]) => init?.method === "POST");

  /** ファイルを選んだことにして、フォームを送る。既定の送り方を止めたかを返す。 */
  function submit(files: File[]): boolean {
    Object.defineProperty(input(), "files", { value: files, configurable: true });
    const event = new Event("submit", { bubbles: true, cancelable: true });
    form().dispatchEvent(event);
    return event.defaultPrevented;
  }

  const finished = () => vi.waitFor(() => expect(status()).toMatch(/^送り終えました/));

  test("htmx がページを入れ替えたら、ファイルの欄で複数を選べるようにする", () => {
    input().multiple = false;
    document.body.dispatchEvent(new Event("htmx:load", { bubbles: true }));
    expect(input().multiple).toBe(true);
  });

  test("写真を選んでいなければ、送らずに選ぶよう伝える（htmx に本文ごと入れ替えさせない）", () => {
    expect(submit([])).toBe(true);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(status()).toBe("追加する写真を選んでください");
  });

  test("1 枚ずつ、バイト列をそのまま送り、写真の欄だけを読み直して件数を出す", async () => {
    respond([page(204, null), page(204, null)], html(200, editPage("", ["p1", "p2"], 1)));
    const name = document.querySelector('input[name="name"]') as HTMLInputElement;
    name.value = "書きかけ";
    const photos = [file("a.jpg", 5), file("b.png", 6, "image/png")];
    const action = form().action;
    expect(submit(photos)).toBe(true);
    await vi.waitFor(() => expect(messages()[0]?.text).toBe("2 枚の写真を追加しました"));

    expect(posts().map(([url, init]) => [url, init.body, init.headers])).toEqual([
      [action, photos[0], { "Content-Type": "image/jpeg" }],
      [action, photos[1], { "Content-Type": "image/png" }],
    ]);
    expect(fetchMock).toHaveBeenLastCalledWith(EDIT);
    expect(document.querySelectorAll("#photos img")).toHaveLength(2);
    expect(form().dataset.remaining).toBe("1");
    // 料理名などのフォームは入れ替えない。
    expect(name.isConnected).toBe(true);
    expect(name.value).toBe("書きかけ");
    expect(messages()).toHaveLength(1);
    // 押したボタンが消えたので、写真の欄の見出しにフォーカスを移す。
    expect(document.activeElement?.id).toBe("photos-title");
  });

  test("写真の欄を読み直せなければ、フォームを使える状態に戻し、残りの枚数を減らす", async () => {
    respond([page(204, null), page(204, null)], new TypeError("Failed to fetch"));
    submit([file("a.jpg", 5), file("b.png", 6, "image/png")]);
    const setValue = vi.fn();
    Object.defineProperty(input(), "value", { get: () => "C:\\fakepath\\a.jpg", set: setValue, configurable: true });
    await finished();

    expect(setValue).toHaveBeenCalledWith("");
    expect(status()).toBe("送り終えました。ページを読み直すと、追加した写真が出ます");
    expect(messages()).toEqual([
      { kind: "message success", role: "status", text: "2 枚の写真を追加しました", items: [] },
    ]);
    expect(form().dataset.remaining).toBe("1");
    expect(input().disabled).toBe(false);
  });

  test("形式の分からないファイルは、application/octet-stream で送る", async () => {
    respond([page(204, null)]);
    submit([file("a", 5, "")]);
    await finished();
    expect(posts()[0]?.[1].headers).toEqual({ "Content-Type": "application/octet-stream" });
  });

  test("その写真だけの誤りなら、残りを続けて送り、理由を出す", async () => {
    respond([page(415, "JPEG・PNG・WebP の写真を選んでください"), page(204, null)]);
    submit([file("a.gif", 5, "image/gif"), file("b.jpg", 5)]);
    await finished();

    expect(posts()).toHaveLength(2);
    expect(messages()).toEqual([
      { kind: "message success", role: "status", text: "1 枚の写真を追加しました", items: [] },
      {
        kind: "message error",
        role: "alert",
        text: "1 枚の写真を追加できませんでした",
        items: ["a.gif: JPEG・PNG・WebP の写真を選んでください"],
      },
    ]);
  });

  test("続けても追加できない失敗なら、残りを送らない", async () => {
    respond([page(409, "写真は 5 枚までです")]);
    submit([file("a.jpg", 5), file("b.jpg", 5), file("c.jpg", 5)]);
    await finished();

    expect(posts()).toHaveLength(1);
    expect(status()).toBe("送り終えました");
    expect(messages()).toEqual([
      {
        kind: "message error",
        role: "alert",
        text: "3 枚の写真を追加できませんでした",
        items: [
          "a.jpg: 写真は 5 枚までです",
          "b.jpg: 前の写真を追加できなかったため、送りませんでした",
          "c.jpg: 前の写真を追加できなかったため、送りませんでした",
        ],
      },
    ]);
  });

  test("通信できなければ、残りを送らない", async () => {
    respond([new TypeError("Failed to fetch")]);
    submit([file("a.jpg", 5), file("b.jpg", 5)]);
    await finished();

    expect(posts()).toHaveLength(1);
    expect(messages()[0]?.items).toEqual([
      "a.jpg: 通信できませんでした",
      "b.jpg: 前の写真を追加できなかったため、送りませんでした",
    ]);
  });

  test("空のファイルと大きすぎるファイルは、送らずに断る", async () => {
    respond([page(204, null)]);
    submit([file("empty.jpg", 0), file("big.jpg", 101), file("ok.jpg", 100)]);
    await finished();

    expect(posts()).toHaveLength(1);
    expect(messages()[1]?.items).toEqual(["empty.jpg: 空のファイルは追加できません", `big.jpg: ${TOO_LARGE}`]);
  });

  test("1 枚も送らなければ、写真の欄を読み直さずに理由だけを出す", async () => {
    const setValue = vi.fn();
    Object.defineProperty(input(), "value", { get: () => "C:\\fakepath\\big.jpg", set: setValue, configurable: true });
    await upload(form(), [file("big.jpg", 101)]);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(status()).toBe("追加できる写真がなかったので、送りませんでした");
    expect(messages()[0]?.items).toEqual([`big.jpg: ${TOO_LARGE}`]);
    expect(setValue).toHaveBeenCalledWith("");
    expect(input().disabled).toBe(false);
  });

  test.each([
    ["tooLarge", undefined],
    ["maxBytes", undefined],
    ["maxBytes", ""],
    ["maxBytes", "0"],
    ["maxBytes", "abc"],
  ] as const)(
    "data-%s が %s なら、大きさを確かめずに送り、サーバーの知らせを理由にする",
    async (key, value) => {
      if (value === undefined) {
        delete form().dataset[key];
      } else {
        form().dataset[key] = value;
      }
      respond([page(413, "サーバーの知らせ")]);
      await upload(form(), [file("big.jpg", 101)]);
      expect(posts()).toHaveLength(1);
      expect(messages()[0]?.items).toEqual(["big.jpg: サーバーの知らせ"]);
    },
  );

  test("残りの枚数を読めなければ、枚数を確かめずに送る", async () => {
    form().dataset.remaining = "";
    respond([]);
    await upload(form(), [file("a.jpg", 5), file("b.jpg", 5), file("c.jpg", 5), file("d.jpg", 5)]);
    expect(posts()).toHaveLength(4);
    expect(form().dataset.remaining).toBe("");
  });

  test("残りの枚数より多く選んだら、送らずに減らすよう伝え、送れない写真も挙げる", () => {
    submit([file("a.jpg", 5), file("b.jpg", 5), file("c.jpg", 5), file("d.jpg", 5), file("big.jpg", 101)]);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(status()).toBe(
      `写真はあと 3 枚まで追加できます。選んだ写真を減らしてください。ほかに、追加できない写真があります（big.jpg: ${TOO_LARGE}）`,
    );
    expect(input().disabled).toBe(false);
  });

  test("サーバーがほかの画面に移らせたら、残りを送らず、ページごと読み直す", async () => {
    respond([page(200, null, true)]);
    const assign = vi.spyOn(window.location, "assign").mockImplementation(() => {});
    submit([file("a.jpg", 5), file("b.jpg", 5)]);
    await vi.waitFor(() => expect(assign).toHaveBeenCalledWith(EDIT));

    expect(posts()).toHaveLength(1);
  });

  test("写真の欄を読み直している間にほかの画面に移っていたら、その画面の知らせを書き換えない", async () => {
    fetchMock.mockImplementation(async (_url: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        return page(204, null);
      }
      window.history.replaceState(null, "", "/recipes");
      return html(200, editPage("", ["p1"], 2));
    });
    await upload(form(), [file("a.jpg", 5)]);
    expect(messages()).toEqual([{ kind: "message success", role: "status", text: "前のメッセージ", items: [] }]);
    expect(document.querySelectorAll("#photos img")).toHaveLength(0);
  });

  test("送っている間にほかの画面に移っていたら、その画面を書き換えない", async () => {
    // 送っている間に画面を移る。待つ時間に頼らないよう、upload を直接呼んで終わりを待つ。
    fetchMock.mockImplementation(async () => {
      window.history.replaceState(null, "", "/recipes");
      return page(204, null);
    });
    await upload(form(), [file("a.jpg", 5)]);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(status()).toBe("写真を送っています（1 / 1 枚目）");
    expect(messages()).toEqual([{ kind: "message success", role: "status", text: "前のメッセージ", items: [] }]);
  });
});

describe("削除の流れ", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    window.history.replaceState(null, "", EDIT);
    document.body.innerHTML = editPage("", ["p1", "p2"], 3);
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  const deleteForm = () => document.querySelector('form[action$="/p1/delete"]') as HTMLFormElement;
  const messageText = () => document.getElementById("messages")?.textContent?.trim();

  test("確認のダイアログから送り、移った先の編集の画面から写真の欄と知らせを写す", async () => {
    fetchMock.mockResolvedValue(
      html(200, editPage('<p class="message success">写真を削除しました</p>', ["p2"], 4), undefined, true),
    );
    const name = document.querySelector('input[name="name"]') as HTMLInputElement;
    name.value = "書きかけ";
    const event = new Event("submit", { bubbles: true, cancelable: true });
    deleteForm().dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(messageText()).toBe("写真を削除しました"));

    expect(fetchMock).toHaveBeenCalledWith("http://localhost:3000/recipes/r1/photos/p1/delete", {
      method: "POST",
      body: expect.any(URLSearchParams),
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
    });
    expect(Array.from(document.querySelectorAll("#photos img"), (img) => img.getAttribute("src"))).toEqual([
      "/recipes/r1/photos/p2",
    ]);
    expect(name.value).toBe("書きかけ");
  });

  test("エラーの画面が返れば、知らせだけを写し、写真の欄をそのまま使えるようにする", async () => {
    fetchMock.mockResolvedValue(
      html(429, '<main><div id="messages"><p role="alert">少し待ってください</p></div></main>'),
    );
    await remove(deleteForm());
    expect(messageText()).toBe("少し待ってください");
    expect(document.querySelectorAll("#photos img")).toHaveLength(2);
    expect((deleteForm().querySelector("button") as HTMLButtonElement).disabled).toBe(false);
  });

  test("通信できなければ、知らせを出し、写真の欄をそのまま使えるようにする", async () => {
    fetchMock.mockRejectedValue(new TypeError("Failed to fetch"));
    await remove(deleteForm());
    expect(messageText()).toBe("写真を削除できませんでした（通信できませんでした）");
    expect((deleteForm().querySelector("button") as HTMLButtonElement).disabled).toBe(false);
  });

  test("編集の画面でない所へ移らされたら、ページごと読み直す", async () => {
    fetchMock.mockResolvedValue(html(200, "<main></main>", "http://localhost:3000/", true));
    const assign = vi.spyOn(window.location, "assign").mockImplementation(() => {});
    await remove(deleteForm());
    expect(assign).toHaveBeenCalledWith(EDIT);
  });

  test("送っている間にほかの画面に移っていたら、その画面を書き換えない", async () => {
    fetchMock.mockImplementation(async () => {
      window.history.replaceState(null, "", "/recipes");
      return html(200, editPage("<p>写真を削除しました</p>", [], 5));
    });
    await remove(deleteForm());
    expect(messageText()).toBe("");
    expect(document.querySelectorAll("#photos img")).toHaveLength(2);
  });

  test("通信できずに、その間にほかの画面に移っていたら、その画面に知らせを出さない", async () => {
    fetchMock.mockImplementation(async () => {
      window.history.replaceState(null, "", "/recipes");
      throw new TypeError("Failed to fetch");
    });
    await remove(deleteForm());
    expect(messageText()).toBe("");
  });

  test("削除が済んだのに編集の画面を読めなければ、消した写真を欄から除き、済んだことを知らせる", async () => {
    fetchMock.mockResolvedValue(html(500, "<main></main>", undefined, true));
    await remove(deleteForm());
    expect(Array.from(document.querySelectorAll("#photos img"), (img) => img.getAttribute("src"))).toEqual([
      "/recipes/r1/photos/p2",
    ]);
    expect(messageText()).toBe("写真を削除しました。ページを読み直すと、写真の欄が新しくなります");
  });

  test("ダイアログのフォームの欄も、一緒に送る", async () => {
    const hidden = document.createElement("input");
    hidden.type = "hidden";
    hidden.name = "back";
    hidden.value = "edit";
    deleteForm().prepend(hidden);
    fetchMock.mockResolvedValue(html(200, editPage("", ["p2"], 4), undefined, true));
    await remove(deleteForm());
    expect(String(fetchMock.mock.calls[0]?.[1].body)).toBe("back=edit");
  });

  test("知らせは、今ある #messages の中身だけを入れ替える（読み上げのため、要素は同じものを使う）", async () => {
    const messages = document.getElementById("messages");
    fetchMock.mockResolvedValue(
      html(200, editPage('<p class="message success" role="status">写真を削除しました</p>', ["p2"], 4), undefined, true),
    );
    await remove(deleteForm());
    expect(document.getElementById("messages")).toBe(messages);
    expect(messageText()).toBe("写真を削除しました");
  });
});
