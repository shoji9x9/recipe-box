//! 利用量の上限のうち、ハンドラーより前に確かめるもの。
//!
//! 費用を守るため、上限は 3 つの層で設ける（README の「費用の対策」）。
//! - IP アドレスごと: 書き込む API の回数（ここ）と、ワークスペースを作る回数（`workspace`）。
//! - ワークスペースごと: レシピの数（`recipe`）。
//! - アプリ全体: ワークスペースの数と、1 日に作る数（`workspace`）。D1 の行を数えて厳密に判定する。

use std::net::{IpAddr, Ipv6Addr};

use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{backend::Backend, error::ApiError, recipe::MAX_BODY_BYTES};

/// リクエストを送った人を見分けるのに使うヘッダー。Cloudflare が接続元の IP アドレスを入れる。
const CLIENT_IP_HEADER: &str = "cf-connecting-ip";

/// 書き込む API（`POST`・`PUT`・`DELETE`）の前に、次の順に確かめる middleware。
///
/// 1. IP アドレスごとの書き込みの回数。超えたら `429`。ワークスペースを調べる前に断り、D1 を読ませない。
/// 2. `Content-Length` が本文の上限を超えていないか。超えたら、本文を読まずに `413`。
///
/// `Content-Length` の無い本文（chunked）は、ハンドラーが本文を読むときに上限で止める（`DefaultBodyLimit`）。
pub async fn limit_writes<B: Backend>(
    State(backend): State<B>,
    request: Request,
    next: Next,
) -> Response {
    match check_write(&backend, request.method(), request.headers()).await {
        Ok(()) => next.run(request).await,
        Err(error) => error.into_response(),
    }
}

/// [`limit_writes`] の確かめる中身。画面の middleware も使う。読むだけのメソッドなら何もしない。
///
/// `Request` の本文は `Sync` でないので、`Request` ではなくメソッドとヘッダーを受け取る
/// （`await` をまたいで `&Request` を持つと、`Future` が `Send` にならない）。
pub async fn check_write<B: Backend>(
    backend: &B,
    method: &Method,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    if !matches!(*method, Method::POST | Method::PUT | Method::DELETE) {
        return Ok(());
    }
    if !backend.allow_write(&client_key(headers)).await? {
        return Err(ApiError::TooManyRequests);
    }
    check_body_size(headers)
}

/// `Content-Length` が本文の上限を超えていれば `PayloadTooLarge`。本文を読む前に断るために使う。
pub fn check_body_size(headers: &HeaderMap) -> Result<(), ApiError> {
    // usize に収まらないほど大きい値も、上限を超えたものとして扱う。
    if declared_length(headers).is_some_and(|length| {
        usize::try_from(length).map_or(true, |length| length > MAX_BODY_BYTES)
    }) {
        return Err(ApiError::PayloadTooLarge);
    }
    Ok(())
}

/// `Content-Length` の値。無いか、数字だけでなければ `None`。u64 に収まらないほど大きければ `u64::MAX`。
fn declared_length(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get(header::CONTENT_LENGTH)?.to_str().ok()?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(value.parse().unwrap_or(u64::MAX))
}

/// 回数の制限（Rate Limiting）で、リクエストを送った人を表すキー。
///
/// - IPv4 は、アドレスそのものをキーにする。IPv4 射影アドレス（`::ffff:192.0.2.1`）も IPv4 として扱う。
/// - IPv6 は、上位 64 ビットだけをキーにする。1 つの回線には /64 以上の範囲が割り当てられることが多く、
///   アドレスをそのまま使うと、範囲の中でアドレスを変えるだけで制限を避けられるため。
/// - ヘッダーが無いか、IP アドレスとして読めないときは、まとめて `unknown` として数える。
///   Cloudflare を通るリクエストには必ずヘッダーが付くので、これに当たるのはローカルでの開発だけである。
///   IP アドレスとして読んだ値だけを使うので、キーの長さは限られる。
pub fn client_key(headers: &HeaderMap) -> String {
    let ip = headers
        .get(CLIENT_IP_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<IpAddr>().ok())
        // `::ffff:192.0.2.1` のような IPv4 射影アドレスは、IPv4 のアドレスにそろえる。
        .map(|ip| ip.to_canonical());
    match ip {
        Some(IpAddr::V4(v4)) => v4.to_string(),
        Some(IpAddr::V6(v6)) => {
            // to_bits() は 128 ビットの整数を返す。下位 64 ビットを 0 にして、/64 の範囲の先頭にする。
            let prefix = Ipv6Addr::from_bits(v6.to_bits() & !u128::from(u64::MAX));
            format!("{prefix}/64")
        }
        None => "unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderMap;

    use super::client_key;

    fn key_for(value: Option<&str>) -> String {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert("cf-connecting-ip", value.parse().unwrap());
        }
        client_key(&headers)
    }

    #[test]
    fn client_key_uses_ipv4_address() {
        assert_eq!(key_for(Some("192.0.2.1")), "192.0.2.1");
    }

    #[test]
    fn client_key_treats_ipv4_mapped_address_as_ipv4() {
        assert_eq!(key_for(Some("::ffff:192.0.2.1")), "192.0.2.1");
    }

    #[test]
    fn client_key_uses_ipv6_prefix() {
        assert_eq!(key_for(Some("2001:db8:1:2:3:4:5:6")), "2001:db8:1:2::/64");
        // 同じ /64 の中の別のアドレスは、同じキーになる。
        assert_eq!(key_for(Some("2001:db8:1:2:ffff::1")), "2001:db8:1:2::/64");
    }

    #[test]
    fn client_key_falls_back_to_unknown() {
        assert_eq!(key_for(None), "unknown");
        assert_eq!(key_for(Some("not-an-ip")), "unknown");
        assert_eq!(key_for(Some(&"1".repeat(1000))), "unknown");
    }
}
