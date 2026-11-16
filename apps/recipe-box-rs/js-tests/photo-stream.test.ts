// src/photo-stream.js（Worker の中で写真の本文を流す JavaScript）の単体テスト（mise run test）。
// Workers の FixedLengthStream の代わりに、長さを確かめる TransformStream を置く。
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { discardPhoto, joinPhoto, photoReader, readAtLeast } from "../src/photo-stream.js";

/** 塊を順に流す本文。取り消されたかを `cancelled` で見る。 */
function body(chunks: number[][]): { stream: ReadableStream<Uint8Array>; cancelled: () => boolean } {
  let cancelled = false;
  let index = 0;
  const stream = new ReadableStream<Uint8Array>({
    pull(controller) {
      const chunk = chunks[index++];
      if (chunk === undefined) {
        controller.close();
      } else {
        controller.enqueue(new Uint8Array(chunk));
      }
    },
    cancel() {
      cancelled = true;
    },
  });
  return { stream, cancelled: () => cancelled };
}

async function readAll(stream: ReadableStream<Uint8Array>): Promise<number[]> {
  const out: number[] = [];
  const reader = stream.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) return out;
    out.push(...value);
  }
}

/** FixedLengthStream の代わり。流れた長さが `length` と違えば、読む側を失敗させる。 */
class FakeFixedLengthStream extends TransformStream<Uint8Array, Uint8Array> {
  constructor(length: number) {
    let seen = 0;
    super({
      transform(chunk, controller) {
        seen += chunk.byteLength;
        controller.enqueue(chunk);
      },
      flush(controller) {
        if (seen !== length) controller.error(new Error(`長さが違う: ${seen}/${length}`));
      },
    });
  }
}

describe("readAtLeast", () => {
  test("要る分まで読み、多く読んだ分は残りに置く", async () => {
    const state = photoReader(body([[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12]]).stream);
    expect([...(await readAtLeast(state, 6, 6))]).toEqual([1, 2, 3, 4, 5, 6]);
    expect([...(state.rest ?? [])]).toEqual([7, 8]);
    // 残りから先に使う。本文が終われば、要る分より短い。
    expect([...(await readAtLeast(state, 10, 10))]).toEqual([7, 8, 9, 10, 11, 12]);
    expect(state.rest).toBeNull();
    expect([...(await readAtLeast(state, 1, 1))]).toEqual([]);
  });

  test("要る分が塊より小さければ、残りの上限まで 1 つの塊を分ける", async () => {
    const state = photoReader(body([[1, 2, 3, 4, 5, 6, 7, 8]]).stream);
    expect([...(await readAtLeast(state, 2, 5))]).toEqual([1, 2, 3, 4, 5]);
    expect([...(state.rest ?? [])]).toEqual([6, 7, 8]);
  });
});

describe("joinPhoto", () => {
  beforeEach(() => {
    vi.stubGlobal("FixedLengthStream", FakeFixedLengthStream);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  test("書き換えた頭・読みすぎた分・本文の残りを、この順に流す", async () => {
    const { stream } = body([[1, 2, 3, 4], [5, 6, 7, 8], [9, 10]]);
    const state = photoReader(stream);
    const head = await readAtLeast(state, 3, 3);
    expect([...head]).toEqual([1, 2, 3]);
    // 頭を書き換えた（0 で埋めた）とする。
    const joined = joinPhoto(stream, state, new Uint8Array([0, 0, 0]), 10);
    expect(await readAll(joined)).toEqual([0, 0, 0, 4, 5, 6, 7, 8, 9, 10]);
  });

  test("読みすぎた分が無くても流す", async () => {
    const { stream } = body([[1, 2], [3, 4]]);
    const state = photoReader(stream);
    await readAtLeast(state, 2, 2);
    expect(state.rest).toBeNull();
    expect(await readAll(joinPhoto(stream, state, new Uint8Array([9, 9]), 4))).toEqual([9, 9, 3, 4]);
  });

  test("本文が宣言した長さより短ければ、読む側が失敗する", async () => {
    const { stream } = body([[1, 2], [3]]);
    const state = photoReader(stream);
    await readAtLeast(state, 2, 2);
    await expect(readAll(joinPhoto(stream, state, new Uint8Array([1, 2]), 10))).rejects.toThrow("長さが違う");
  });
});

test("discardPhoto は本文を取り消す", async () => {
  const source = body([[1], [2], [3]]);
  const state = photoReader(source.stream);
  await readAtLeast(state, 1, 1);
  discardPhoto(state);
  await vi.waitFor(() => expect(source.cancelled()).toBe(true));
});
