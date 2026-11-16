//! 写真のバイト列をそのまま送るリクエストの本文を、読みためずにストリームのまま R2 に渡す。
//!
//! 本文を一度メモリーに読みためると、4 MB の写真で CPU 時間が数十 ms かかる（Wasm に写さず JS の側でためても同じ）。
//! ストリームのまま R2 に渡せば 1 ms ほどで済む（Issue #31 で測った）。
//!
//! 形式は中身の先頭のバイトで決める（[`crate::photo::PhotoFormat::detect`]）ので、先頭だけを読み、
//! 残りは読まずに R2 へ流す。R2 は長さの分かるストリームしか受け付けないので、`FixedLengthStream` を通す。
//! 長さは `Content-Length` で決まり、実際の本文と違えば `FixedLengthStream` が失敗して、R2 への書き込みも失敗する。
//!
//! 使うのは本番（wasm）だけである。テストでは本文をバイト列として読む経路（[`crate::photo::PhotoBody::Bytes`]）を通る。

use axum::http::{Method, header};
use wasm_bindgen::prelude::wasm_bindgen;
use worker::{
    HttpRequest,
    js_sys::{Promise, Reflect, Uint8Array},
    send::{SendFuture, SendWrapper},
    wasm_bindgen::{JsCast, JsValue},
    wasm_bindgen_futures::JsFuture,
    web_sys,
};

use crate::{
    error::{ApiError, log_error},
    photo,
};

/// 形式を決めるのに読む、本文の先頭のバイト数（WebP の判定に 12 バイト要る）。
const HEAD_BYTES: u32 = 16;

#[wasm_bindgen(inline_js = r#"
export async function peekPhoto(body, length, n) {
  const [forHead, forR2] = body.tee();
  const reader = forHead.getReader();
  const chunks = [];
  let got = 0;
  while (got < n) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    got += value.byteLength;
  }
  reader.cancel().catch(() => {});
  const head = new Uint8Array(Math.min(got, n));
  let offset = 0;
  for (const chunk of chunks) {
    if (offset >= head.length) break;
    const part = chunk.subarray(0, head.length - offset);
    head.set(part, offset);
    offset += part.length;
  }
  const { readable, writable } = new FixedLengthStream(length);
  forR2.pipeTo(writable).catch(() => {});
  return { head, readable };
}
"#)]
extern "C" {
    /// 本文の先頭 `n` バイトを読み、`{ head, readable }` を返す。`readable` は本文の全体（長さ `length`）を流す。
    #[wasm_bindgen(catch, js_name = peekPhoto)]
    fn peek_photo(body: &web_sys::ReadableStream, length: u32, n: u32) -> Result<Promise, JsValue>;
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

/// 先頭を読み終えた写真の本文。
pub struct PhotoStream {
    /// 本文の先頭（[`HEAD_BYTES`] まで）。
    pub head: Vec<u8>,
    /// 本文の大きさ（`Content-Length`）。
    pub length: usize,
    /// 本文の全体を流すストリーム。長さが分かるので、そのまま R2 に渡せる。
    pub readable: SendWrapper<web_sys::ReadableStream>,
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

/// 本文の先頭だけを読み、残りを R2 に流せる形にする。
pub async fn open(raw: RawPhotoBody) -> Result<PhotoStream, ApiError> {
    let peeked = async {
        // 写真の上限（4,000,000）以下なので u32 に収まる。
        let length =
            u32::try_from(raw.length).map_err(|_| JsValue::from_str("本文が大きすぎる"))?;
        let promise = peek_photo(&raw.stream, length, HEAD_BYTES)?;
        let result = JsFuture::from(promise).await?;
        let head = Reflect::get(&result, &JsValue::from_str("head"))?.dyn_into::<Uint8Array>()?;
        let readable = Reflect::get(&result, &JsValue::from_str("readable"))?
            .dyn_into::<web_sys::ReadableStream>()?;
        Ok::<_, JsValue>((head.to_vec(), readable))
    };
    // 本文を読めないのは、送る途中で切れたときなど。multipart を読めないときと同じに扱う。
    let (head, readable) = SendFuture::new(peeked)
        .await
        .map_err(|_| ApiError::InvalidBody)?;
    Ok(PhotoStream {
        head,
        length: raw.length,
        readable: SendWrapper::new(readable),
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
