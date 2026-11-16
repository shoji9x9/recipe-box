// 写真の本文を流す、Worker の中の JavaScript（src/stream.rs が wasm-bindgen で呼ぶ）。
// 頭の部分（画像のデータより前）だけを読んで Rust に渡し、書き換えた頭の後に、残りを組み込みの pipeTo で R2 へ流す。
// 本文の全体を JavaScript に流すと CPU 時間が大きく増えるので、頭の後ろは読まない（Issue #52）。
// 画面の JavaScript ではないので public/ に置かない。単体テストは js-tests/photo-stream.test.ts。

/**
 * 本文を読む状態。`rest` は、読んだが Rust に渡していない残り。
 * @typedef {{ reader: ReadableStreamDefaultReader<Uint8Array>, rest: Uint8Array | null }} PhotoReader
 */

/**
 * 本文を読む状態を作る。
 * @param {ReadableStream<Uint8Array>} body
 * @returns {PhotoReader}
 */
export function photoReader(body) {
  return { reader: body.getReader(), rest: null };
}

/**
 * 残り（null でないこと）から、多くとも `max` バイトを取り出す。超えた分は残りに置く。
 * @param {PhotoReader} state
 * @param {number} max
 * @returns {Uint8Array}
 */
function take(state, max) {
  const chunk = /** @type {Uint8Array} */ (state.rest);
  if (chunk.byteLength <= max) {
    state.rest = null;
    return chunk;
  }
  state.rest = chunk.subarray(max);
  return chunk.subarray(0, max);
}

/**
 * 少なくとも `min` バイト、多くとも `max` バイト（`min` 以上）を読み、1 つにして返す。本文が終われば、それより短い。
 * 読んだ塊のうち `max` を超えた分は `rest` に残し、Rust に写さない（joinPhoto が流す）。
 * @param {PhotoReader} state
 * @param {number} min
 * @param {number} max
 * @returns {Promise<Uint8Array>}
 */
export async function readAtLeast(state, min, max) {
  /** @type {Uint8Array[]} */
  const chunks = [];
  let got = 0;
  while (got < min) {
    if (state.rest === null) {
      const { done, value } = await state.reader.read();
      if (done) break;
      state.rest = value;
    }
    const chunk = take(state, max - got);
    chunks.push(chunk);
    got += chunk.byteLength;
  }
  if (chunks.length === 1 && chunks[0] !== undefined) return chunks[0];
  const out = new Uint8Array(got);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return out;
}

/**
 * 断った写真の本文を、読まずに捨てる。捨てないと、送り手がまだ送っている間に応答を返して、接続が切れることがある。
 * @param {PhotoReader} state
 */
export function discardPhoto(state) {
  state.reader.cancel().catch(() => {});
}

/**
 * 読まない本文を取り消す。読まずに応答を返すと、送り手がまだ送っている間に接続が切れることがあるため。
 * 取り消せない（reader が付いているなど）ときは何もしない。
 * @param {ReadableStream<Uint8Array>} body
 */
export function cancelBody(body) {
  body.cancel().catch(() => {});
}

/**
 * reader を外し、`head`（書き換えた頭）・`rest`・本文の残りを続けて流す、長さ `length` のストリームを返す。
 * 本文の残りは JavaScript を通さず、組み込みの pipeTo で流す。読む側（R2）が読み始めるまでは、何も流れない。
 * 長さが実際の本文と違えば、FixedLengthStream が失敗し、読む側も失敗する。
 * @param {ReadableStream<Uint8Array>} body
 * @param {PhotoReader} state
 * @param {Uint8Array} head
 * @param {number} length
 * @returns {ReadableStream<Uint8Array>}
 */
// guide:start join-photo
export function joinPhoto(body, state, head, length) {
  state.reader.releaseLock();
  /** @type {FixedLengthStream} */
  let fixed;
  try {
    fixed = new FixedLengthStream(length);
  } catch (error) {
    cancelBody(body);
    throw error;
  }
  const { readable, writable } = fixed;
  const writer = writable.getWriter();
  const rest = state.rest;
  writer
    .write(head)
    .then(() => (rest === null ? undefined : writer.write(rest)))
    .then(() => {
      writer.releaseLock();
      return body.pipeTo(writable);
    })
    // 読む側（R2）が失敗するか取り消すと、書き込みか pipeTo が失敗する。本文の残りを取り消す
    // （pipeTo の途中なら pipeTo が取り消すので、ここでは何もしない）。
    .catch(() => {
      cancelBody(body);
    });
  return readable;
}
// guide:end join-photo
