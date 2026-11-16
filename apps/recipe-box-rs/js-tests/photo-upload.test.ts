// public/assets/photo-upload.js の単体テスト（mise run test）。
// 判定の関数は直接呼び、送る流れは、happy-dom の DOM にフォームを置いて submit を起こし、fetch を差し替えて確かめる。
import { afterEach, beforeEach, describe as group, expect, test, vi } from "vitest";

import { classify, describe, detailPathOf, failureReason, localReason } from "../public/assets/photo-upload.js";

// 読み込んだときに、document に submit と htmx:load の受け手を付ける（モジュールの最後）。

const DETAIL = "/recipes/r1";
const TOO_LARGE = "写真は 4 MB までです";

/** 失敗の画面（知らせの文を role="alert" に置く）の応答。 */
function page(status: number, alert: string | null, redirected = false): Response {
  const body = alert === null ? "<main></main>" : `<main><p role="alert"> ${alert} </p></main>`;
  const response = new Response(status === 204 ? null : body, { status });
  Object.defineProperty(response, "redirected", { value: redirected });
  return response;
}

function file(name: string, size: number, type = "image/jpeg"): File {
  return new File([new Uint8Array(size)], name, { type });
}

group("判定の関数", () => {
  test("describe は、ファイル名と理由を 1 つの文にする", () => {
    expect(
      describe([
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

  test("detailPathOf は、送り先から /photos を除いたパスにする", () => {
    expect(detailPathOf("http://localhost:3000/recipes/r1/photos")).toBe(DETAIL);
    expect(detailPathOf("http://localhost:3000/recipes/r1/photos/")).toBe(DETAIL);
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

group("送る流れ", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    window.history.replaceState(null, "", DETAIL);
    document.body.innerHTML = `
      <main>
        <div class="page-head"><h1>カレー</h1></div>
        <p class="message success" role="status">前のメッセージ</p>
        <form action="${DETAIL}/photos" method="post" enctype="multipart/form-data"
          data-photo-upload data-remaining="3" data-max-bytes="100" data-too-large="${TOO_LARGE}">
          <input type="file" name="photo">
          <button type="submit">追加</button>
        </form>
      </main>`;
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  const form = () => document.querySelector("form") as HTMLFormElement;
  const input = () => document.querySelector('input[type="file"]') as HTMLInputElement;
  const status = () => document.querySelector("[data-photo-status]")?.textContent ?? "";
  const messages = () =>
    Array.from(document.querySelectorAll("main > .message"), (element) => ({
      kind: element.className,
      role: element.getAttribute("role"),
      text: element.querySelector("p")?.textContent ?? element.textContent,
      items: Array.from(element.querySelectorAll("li"), (li) => li.textContent),
    }));

  /** ファイルを選んだことにして、フォームを送る。既定の送り方を止めたかを返す。 */
  function submit(files: File[]): boolean {
    Object.defineProperty(input(), "files", { value: files, configurable: true });
    const event = new Event("submit", { bubbles: true, cancelable: true });
    form().dispatchEvent(event);
    return event.defaultPrevented;
  }

  const finished = () => vi.waitFor(() => expect(status()).toMatch(/^送り終えました/));

  test("htmx がページを入れ替えたら、ファイルの欄で複数を選べるようにする", () => {
    expect(input().multiple).toBe(false);
    document.body.dispatchEvent(new Event("htmx:load", { bubbles: true }));
    expect(input().multiple).toBe(true);
  });

  test("写真を選んでいなければ、フォームとして送らせる", () => {
    expect(submit([])).toBe(false);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  test("1 枚ずつ、バイト列をそのまま送り、送り終えたら件数を出す", async () => {
    fetchMock.mockResolvedValue(page(204, null));
    const photos = [file("a.jpg", 5), file("b.png", 6, "image/png")];
    expect(submit(photos)).toBe(true);
    await finished();

    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(fetchMock.mock.calls.map(([url, init]) => [url, init.method, init.body, init.headers])).toEqual([
      [form().action, "POST", photos[0], { "Content-Type": "image/jpeg" }],
      [form().action, "POST", photos[1], { "Content-Type": "image/png" }],
    ]);
    expect(status()).toBe("送り終えました。ページを読み直すと、追加した写真が出ます");
    expect(messages()).toEqual([
      { kind: "message success", role: "status", text: "2 枚の写真を追加しました", items: [] },
    ]);
    // htmx が無く読み直せないので、フォームを使える状態に戻し、残りの枚数を減らす。
    expect(form().dataset.remaining).toBe("1");
    expect(input().disabled).toBe(false);
  });

  test("形式の分からないファイルは、application/octet-stream で送る", async () => {
    fetchMock.mockResolvedValue(page(204, null));
    submit([file("a", 5, "")]);
    await finished();
    expect(fetchMock.mock.calls[0]?.[1].headers).toEqual({ "Content-Type": "application/octet-stream" });
  });

  test("その写真だけの誤りなら、残りを続けて送り、理由を出す", async () => {
    fetchMock.mockResolvedValueOnce(page(415, "JPEG・PNG・WebP の写真を選んでください")).mockResolvedValueOnce(page(204, null));
    submit([file("a.gif", 5, "image/gif"), file("b.jpg", 5)]);
    await finished();

    expect(fetchMock).toHaveBeenCalledTimes(2);
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
    fetchMock.mockResolvedValue(page(409, "写真は 5 枚までです"));
    submit([file("a.jpg", 5), file("b.jpg", 5), file("c.jpg", 5)]);
    await finished();

    expect(fetchMock).toHaveBeenCalledTimes(1);
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
    fetchMock.mockRejectedValue(new TypeError("Failed to fetch"));
    submit([file("a.jpg", 5), file("b.jpg", 5)]);
    await finished();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(messages()[0]?.items).toEqual([
      "a.jpg: 通信できませんでした",
      "b.jpg: 前の写真を追加できなかったため、送りませんでした",
    ]);
  });

  test("空のファイルと大きすぎるファイルは、送らずに断る", async () => {
    fetchMock.mockResolvedValue(page(204, null));
    submit([file("empty.jpg", 0), file("big.jpg", 101), file("ok.jpg", 100)]);
    await finished();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(messages()[1]?.items).toEqual(["empty.jpg: 空のファイルは追加できません", `big.jpg: ${TOO_LARGE}`]);
  });

  test("残りの枚数より多く選んだら、送らずに減らすよう伝え、送れない写真も挙げる", () => {
    submit([file("a.jpg", 5), file("b.jpg", 5), file("c.jpg", 5), file("d.jpg", 5), file("big.jpg", 101)]);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(status()).toBe(
      `写真はあと 3 枚まで追加できます。選んだ写真を減らしてください。ほかに、追加できない写真があります（big.jpg: ${TOO_LARGE}）`,
    );
    expect(input().disabled).toBe(false);
  });

  test("htmx があれば、詳細の画面の本文を読み直してから結果を出す", async () => {
    fetchMock.mockResolvedValue(page(204, null));
    const ajax = vi.fn(async () => {
      document.body.innerHTML = `<main><div class="page-head"><h1>カレー</h1></div></main>`;
    });
    // happy-dom では window と globalThis が別なので、vi.stubGlobal ではなく window に置く。
    Object.assign(window, { htmx: { ajax } });
    try {
      submit([file("a.jpg", 5)]);
      await vi.waitFor(() => expect(messages()[0]?.text).toBe("1 枚の写真を追加しました"));

      expect(ajax).toHaveBeenCalledWith("GET", DETAIL, { target: "body", swap: "innerHTML" });
      // 読み直した画面には、前のメッセージもフォームも無い。
      expect(messages()).toHaveLength(1);
      expect(document.querySelector("form")).toBeNull();
    } finally {
      Reflect.deleteProperty(window, "htmx");
    }
  });

  test("サーバーがほかの画面に移らせたら、残りを送らず、ページごと読み直す", async () => {
    fetchMock.mockResolvedValue(page(200, null, true));
    const assign = vi.spyOn(window.location, "assign").mockImplementation(() => {});
    submit([file("a.jpg", 5), file("b.jpg", 5)]);
    await vi.waitFor(() => expect(assign).toHaveBeenCalledWith(DETAIL));

    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  test("送っている間にほかの画面に移っていたら、その画面を書き換えない", async () => {
    let respond: (response: Response) => void = () => {};
    fetchMock.mockReturnValue(new Promise<Response>((resolve) => (respond = resolve)));
    submit([file("a.jpg", 5)]);
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

    window.history.replaceState(null, "", "/recipes");
    respond(page(204, null));
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(status()).toBe("写真を送っています（1 / 1 枚目）");
    expect(messages()).toEqual([{ kind: "message success", role: "status", text: "前のメッセージ", items: [] }]);
  });
});
