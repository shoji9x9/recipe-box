//! 写真のバイト列をそのまま送るリクエストの本文を、読みためずにストリームのまま R2 に渡す。
//!
//! 本文を一度メモリーに読みためると、4 MB の写真で CPU 時間が数十 ms かかる（Wasm に写さず JS の側でためても同じ）。
//! ストリームのまま R2 に渡せば 1 ms ほどで済む（Issue #31 で測った）。
//!
//! 形式を決めてメタデータを取り除くのに、頭の部分（画像のデータより前）だけを読む（[`crate::metadata`]）。
//! 頭の部分は Rust で書き換え、書き換えた頭を書いてから、残りは読まずに Workers の組み込みの処理（`pipeTo`）で R2 へ流す。
//! 本文の全体を JavaScript に流すと、1 回の追加で CPU 時間が 150〜200 ms 増えるためである（Issue #52 で測った）。
//! R2 は長さの分かるストリームしか受け付けないので、`FixedLengthStream` を通す。
//! 長さは `Content-Length` で決まり、実際の本文と違えば `FixedLengthStream` が失敗して、R2 への書き込みも失敗する。
//! 書き換えは長さを変えないので、`Content-Length` のまま流せる。
//!
//! 使うのは本番（wasm）だけである。テストでは本文をバイト列として読む経路（[`crate::photo::PhotoBody::Bytes`]）を通る。

use axum::http::{Method, header};
use wasm_bindgen::prelude::wasm_bindgen;
use worker::{
    HttpRequest,
    js_sys::{Promise, Uint8Array},
    send::{SendFuture, SendWrapper},
    wasm_bindgen::{JsCast, JsValue},
    wasm_bindgen_futures::JsFuture,
    web_sys,
};

use crate::{
    error::{ApiError, log_error},
    metadata::{self, Inspection, Rejection},
    photo::{self, PhotoFormat},
};

#[wasm_bindgen(inline_js = r#"
export function photoReader(body) {
  return { reader: body.getReader(), rest: null };
}

function take(state, max) {
  const chunk = state.rest;
  if (chunk.byteLength <= max) {
    state.rest = null;
    return chunk;
  }
  state.rest = chunk.subarray(max);
  return chunk.subarray(0, max);
}

export async function readAtLeast(state, min, max) {
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
  if (chunks.length === 1) return chunks[0];
  const out = new Uint8Array(got);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return out;
}

export function discardPhoto(state) {
  state.reader.cancel().catch(() => {});
}

export function joinPhoto(body, state, head, length) {
  state.reader.releaseLock();
  const { readable, writable } = new FixedLengthStream(length);
  const writer = writable.getWriter();
  const rest = state.rest;
  writer
    .write(head)
    .then(() => (rest === null ? undefined : writer.write(rest)))
    .then(() => {
      writer.releaseLock();
      return body.pipeTo(writable);
    })
    .catch((error) => {
      body.cancel(error).catch(() => {});
    });
  return readable;
}
"#)]
extern "C" {
    /// 本文を読む reader と、読んだが渡していない残り（`rest`）を持つ状態を作る。
    #[wasm_bindgen(catch, js_name = photoReader)]
    fn photo_reader(body: &web_sys::ReadableStream) -> Result<JsValue, JsValue>;

    /// 少なくとも `min` バイト、多くとも `max` バイトを読んで、1 つの `Uint8Array` にして返す（本文が終われば、それより短い）。
    /// 読んだ塊のうち `max` を超えた分は、Wasm に写さず、状態の `rest` に残す（[`join_photo`] が R2 に流す）。
    #[wasm_bindgen(catch, js_name = readAtLeast)]
    fn read_at_least(state: &JsValue, min: u32, max: u32) -> Result<Promise, JsValue>;

    /// 断った写真の本文を、読まずに捨てる。
    #[wasm_bindgen(js_name = discardPhoto)]
    fn discard_photo(state: &JsValue);

    /// reader を外し、`head`（書き換えた頭）・状態の `rest`・`body` の残りを続けて流す、長さ `length` のストリームを返す。
    /// 残りは JavaScript を通さず、組み込みの `pipeTo` で流す。R2 が読み始めるまでは、何も流れない。
    #[wasm_bindgen(catch, js_name = joinPhoto)]
    fn join_photo(
        body: &web_sys::ReadableStream,
        state: &JsValue,
        head: &Uint8Array,
        length: u32,
    ) -> Result<web_sys::ReadableStream, JsValue>;
}

/// ストリームのまま扱う写真の本文。[`split_photo_body`] がリクエストの extension に入れる。
///
/// extension は `Clone`・`Send`・`Sync` が要る。Workers は 1 つのスレッドで動くので `SendWrapper` で包む。
/// 複製しても同じ JS のストリームを指し、読むのは 1 回だけである。
#[derive(Clone)]
pub struct RawPhotoBody {
    stream: SendWrapper<web_sys::ReadableStream>,
    /// `Content-Length` の値。
    length: usize,
}

/// 頭の部分を読み終えた写真の本文。
pub struct PhotoStream {
    /// 本文の大きさ（`Content-Length`）。
    pub length: usize,
    /// 頭の部分を調べた結果（形式か、断る理由）。
    pub format: Result<PhotoFormat, Rejection>,
    /// 読んだ本文の先頭。頭の部分は書き換えてある。頭より後ろも、読んだ分は入る。
    head: Vec<u8>,
    /// 本文。`state` の reader で先頭を読んだ後の残りを流す。
    body: SendWrapper<web_sys::ReadableStream>,
    /// 本文を読む reader と、読んだが Wasm に写していない残り（[`photo_reader`]）。
    state: SendWrapper<JsValue>,
}

impl PhotoStream {
    /// 書き換えた頭と、本文の残りを続けて流す、長さの分かるストリームにする（R2 にそのまま渡せる）。
    pub fn into_readable(self) -> Result<web_sys::ReadableStream, JsValue> {
        // 写真の上限（4,000,000）以下なので u32 に収まる。
        let length =
            u32::try_from(self.length).map_err(|_| JsValue::from_str("本文が大きすぎる"))?;
        let head = Uint8Array::from(self.head.as_slice());
        join_photo(&self.body, &self.state, &head, length)
    }
}

/// Workers のリクエストを axum に渡す形にする。写真のバイト列をそのまま送るリクエストだけは、
/// 本文を axum に渡さず、[`RawPhotoBody`] として extension に入れる。
///
/// 対象は、写真を追加するパス（API と画面）への POST で、multipart でなく、
/// `Content-Length` が 1 から写真の上限（[`photo::MAX_PHOTO_BYTES`]）までのものである。
/// 上限を超えるものや `Content-Length` の無いもの（chunked）は今までどおり axum に渡し、
/// middleware が本文を読む前に断るか、本文の上限で止める。
pub fn split_photo_body(req: worker::Request) -> worker::Result<HttpRequest> {
    let Some(length) = streamable_length(&req)? else {
        return HttpRequest::try_from(req);
    };
    let Some(stream) = req.inner().body() else {
        return HttpRequest::try_from(req);
    };
    // 本文の無い同じリクエストを作って axum に渡す。workers-rs は本文を読む準備をした時点でストリームを
    // 取り込んでしまう（ほかからは読めなくなる）ので、本文を渡さない。
    // 作れなければ、今までどおり本文ごと axum に渡す（本文を読んでから確かめる）。
    let mut http = match bare_request(&req) {
        Ok(http) => http,
        Err(error) => {
            // 写真の追加が、本文を読みためる（CPU 時間の多い）経路に戻ったことを分かるようにする。
            log_error(&format!("本文の無いリクエストを作れない: {error}"));
            return HttpRequest::try_from(req);
        }
    };
    http.extensions_mut().insert(RawPhotoBody {
        stream: SendWrapper::new(stream),
        length,
    });
    Ok(http)
}

/// `req` と同じメソッド・URL・ヘッダーで、本文の無いリクエストを作る。
fn bare_request(req: &worker::Request) -> worker::Result<HttpRequest> {
    let init = web_sys::RequestInit::new();
    init.set_method(Method::POST.as_str());
    init.set_headers(&req.inner().headers());
    let bare = web_sys::Request::new_with_str_and_init(&req.inner().url(), &init)?;
    HttpRequest::try_from(worker::Request::from(bare))
}

/// ストリームのまま扱えるリクエストなら、本文の大きさを返す。
fn streamable_length(req: &worker::Request) -> worker::Result<Option<usize>> {
    if req.method() != worker::Method::Post || !is_photo_upload_path(&req.path()) {
        return Ok(None);
    }
    let headers = req.headers();
    if photo::is_multipart(headers.get(header::CONTENT_TYPE.as_str())?.as_deref()) {
        return Ok(None);
    }
    let length = headers
        .get(header::CONTENT_LENGTH.as_str())?
        .and_then(|value| value.parse::<usize>().ok());
    Ok(length.filter(|length| (1..=photo::MAX_PHOTO_BYTES).contains(length)))
}

/// 写真を追加するパス（`/api/recipes/{id}/photos` か `/recipes/{id}/photos`）か。
fn is_photo_upload_path(path: &str) -> bool {
    photo::photo_recipe_path(path.strip_prefix("/api").unwrap_or(path)).is_some()
}

/// 頭の部分を読み足すときに読み、Wasm に写す大きさ（64 KiB。本文の残りがそれより短ければ残りのすべて。要る大きさがそれより大きければ、その大きさ）。
///
/// [`metadata::inspect`] は読み足すたびに頭を先頭から調べ直す。本文は小さな塊（手元の wrangler では 4 KiB）で届くので、
/// 塊ごとに調べ直すと、回数が頭の大きさとともに増える。1 回に読む量を決め、調べ直す回数を、
/// 頭の大きさの上限（[`metadata::MAX_HEAD_BYTES`]）÷ 64 KiB = 16 回までに抑える。読みすぎた分は Wasm に写さない。
const READ_BYTES: usize = 64 * 1024;

/// 本文の頭の部分だけを読んでメタデータを取り除き（[`metadata`]）、残りを R2 に流せる形にする。
///
/// 頭の部分の形が正しくないときなどは、ここでは断らず、結果を [`PhotoStream::format`] に入れる
/// （デモか・大きさを先に確かめる。[`crate::photo::add`]）。
pub async fn open(raw: RawPhotoBody) -> Result<PhotoStream, ApiError> {
    let length = raw.length;
    let read = async {
        let state = photo_reader(&raw.stream)?;
        let mut head = Vec::new();
        let format = loop {
            match metadata::inspect(&head, length) {
                Ok(Inspection::NeedMore(n)) => {
                    // 頭の上限（1 MiB）以下なので u32 に収まる。
                    // 要る分と READ_BYTES の大きいほうを読む（本文の残りより多くは読まない）。
                    let want = (n - head.len()).max(READ_BYTES.min(length - head.len()));
                    let want =
                        u32::try_from(want).map_err(|_| JsValue::from_str("頭が大きすぎる"))?;
                    let chunk = JsFuture::from(read_at_least(&state, want, want)?)
                        .await?
                        .dyn_into::<Uint8Array>()?;
                    // 本文が Content-Length より短い（送る途中で切れたときなど）。
                    if chunk.length() == 0 {
                        return Err(JsValue::from_str("本文が途中で終わった"));
                    }
                    head.extend_from_slice(&chunk.to_vec());
                }
                Ok(Inspection::Ready(plan)) => {
                    plan.apply(&mut head);
                    break Ok(plan.format);
                }
                Err(rejection) => break Err(rejection),
            }
        };
        if format.is_err() {
            discard_photo(&state);
        }
        if head.len() > length {
            return Err(JsValue::from_str("本文が Content-Length より長い"));
        }
        Ok::<_, JsValue>((state, head, format))
    };
    // 本文を読めないのは、送る途中で切れたときなど。multipart を読めないときと同じに扱う。
    let (state, head, format) = SendFuture::new(read)
        .await
        .map_err(|_| ApiError::InvalidBody)?;
    Ok(PhotoStream {
        length,
        format,
        head,
        body: raw.stream,
        state: SendWrapper::new(state),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photo_upload_paths() {
        let id = "0123456789abcdef0123456789abcdef";
        assert!(is_photo_upload_path(&format!("/api/recipes/{id}/photos")));
        assert!(is_photo_upload_path(&format!("/recipes/{id}/photos")));
        assert!(!is_photo_upload_path(&format!("/recipes/{id}/photos/{id}")));
        assert!(!is_photo_upload_path("/recipes/abc/photos"));
        assert!(!is_photo_upload_path("/recipes//photos"));
        assert!(!is_photo_upload_path(&format!("/api/recipes/{id}")));
        assert!(!is_photo_upload_path(&format!("/recipes/{id}/photos/")));
        assert!(!is_photo_upload_path(&format!(
            "/other/recipes/{id}/photos"
        )));
    }
}
