//! ワークスペース。ログインの代わりに、推測できない ID でデータの持ち主を分ける。

use std::{
    fmt::{self, Write as _},
    net::{IpAddr, Ipv6Addr},
};

use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{HeaderMap, StatusCode, request::Parts},
};
use serde::Serialize;

use crate::{backend::Backend, error::ApiError};

/// API がワークスペースの ID を受け取るヘッダー。
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "ワークスペースを使う API（Issue #4 のレシピの API）で使う"
    )
)]
pub const WORKSPACE_ID_HEADER: &str = "x-workspace-id";

/// ワークスペースを作る人を見分けるのに使うヘッダー。Cloudflare が接続元の IP アドレスを入れる。
const CLIENT_IP_HEADER: &str = "cf-connecting-ip";

/// ワークスペースの ID。128 ビットの乱数を、16 進数の小文字 32 文字で表したもの。
///
/// 値を作れるのは [`WorkspaceId::from_bytes`] と [`WorkspaceId::parse`] だけなので、
/// この型の値は、いつも正しい形をしている。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// ID の文字数。
    const LEN: usize = 32;

    /// 16 バイトの乱数から ID を作る。
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        let mut text = String::with_capacity(Self::LEN);
        for byte in bytes {
            // String への書き込みは失敗しないので、結果は捨てる。
            let _ = write!(text, "{byte:02x}");
        }
        Self(text)
    }

    /// 文字列が ID の形をしていれば、ID にする。大文字の 16 進数は受け付けない。
    pub fn parse(value: &str) -> Option<Self> {
        let valid = value.len() == Self::LEN
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        valid.then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// リクエストが使うワークスペース。
///
/// ハンドラーの引数に書くと、`X-Workspace-Id` ヘッダーを調べ、
/// ワークスペースが在れば最終利用日時を更新したうえで、その ID を渡す。
/// 無い・形が正しくない・見つからないときは、ハンドラーを呼ばずにエラーを返す。
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "ワークスペースを使う API（Issue #4 のレシピの API）で使う"
    )
)]
#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
}

impl<B: Backend> FromRequestParts<B> for Workspace {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, backend: &B) -> Result<Self, Self::Rejection> {
        let value = parts
            .headers
            .get(WORKSPACE_ID_HEADER)
            .ok_or(ApiError::WorkspaceIdMissing)?;
        let id = value
            .to_str()
            .ok()
            .and_then(WorkspaceId::parse)
            .ok_or(ApiError::WorkspaceIdInvalid)?;
        // 在るかどうかの確認と、最終利用日時の更新を 1 回のクエリで済ませる。
        if backend.touch_workspace(&id, backend.now_millis()).await? {
            Ok(Self { id })
        } else {
            Err(ApiError::WorkspaceNotFound)
        }
    }
}

/// `POST /api/workspaces` の応答の本文。
#[derive(Serialize)]
pub struct CreatedWorkspace {
    id: String,
}

/// `POST /api/workspaces`: ワークスペースを作り、ID を返す。
pub async fn create<B: Backend>(
    State(backend): State<B>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<CreatedWorkspace>), ApiError> {
    let client_key = rate_limit_key(&headers);
    if !backend.allow_workspace_creation(&client_key).await? {
        return Err(ApiError::TooManyRequests);
    }
    let id = WorkspaceId::from_bytes(backend.random_128());
    backend.insert_workspace(&id, backend.now_millis()).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedWorkspace { id: id.to_string() }),
    ))
}

/// ワークスペースを作る回数を数えるときの、作る人を表すキー。
///
/// - IPv4 は、アドレスそのものをキーにする。
/// - IPv6 は、上位 64 ビットだけをキーにする。1 つの回線には /64 以上の範囲が割り当てられることが多く、
///   アドレスをそのまま使うと、範囲の中でアドレスを変えるだけで制限を避けられるため。
/// - ヘッダーが無いか、IP アドレスとして読めないときは、まとめて `unknown` として数える。
///   Cloudflare を通るリクエストには必ずヘッダーが付くので、これに当たるのはローカルでの開発だけである。
///   IP アドレスとして読んだ値だけを使うので、キーの長さは限られる。
fn rate_limit_key(headers: &HeaderMap) -> String {
    let ip = headers
        .get(CLIENT_IP_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<IpAddr>().ok());
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

    use super::{WorkspaceId, rate_limit_key};

    fn key_for(value: Option<&str>) -> String {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert("cf-connecting-ip", value.parse().unwrap());
        }
        rate_limit_key(&headers)
    }

    #[test]
    fn rate_limit_key_uses_ipv4_address() {
        assert_eq!(key_for(Some("192.0.2.1")), "192.0.2.1");
    }

    #[test]
    fn rate_limit_key_uses_ipv6_prefix() {
        assert_eq!(key_for(Some("2001:db8:1:2:3:4:5:6")), "2001:db8:1:2::/64");
        // 同じ /64 の中の別のアドレスは、同じキーになる。
        assert_eq!(key_for(Some("2001:db8:1:2:ffff::1")), "2001:db8:1:2::/64");
    }

    #[test]
    fn rate_limit_key_falls_back_to_unknown() {
        assert_eq!(key_for(None), "unknown");
        assert_eq!(key_for(Some("not-an-ip")), "unknown");
        assert_eq!(key_for(Some(&"1".repeat(1000))), "unknown");
    }

    #[test]
    fn from_bytes_makes_lowercase_hex() {
        let id = WorkspaceId::from_bytes([
            0x00, 0x01, 0x0a, 0xff, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0,
            0xb0, 0xc0,
        ]);
        assert_eq!(id.as_str(), "00010aff102030405060708090a0b0c0");
    }

    #[test]
    fn parse_accepts_valid_id() {
        let text = "0123456789abcdef0123456789abcdef";
        assert_eq!(WorkspaceId::parse(text).unwrap().as_str(), text);
    }

    #[test]
    fn parse_rejects_wrong_length_or_characters() {
        for text in [
            "",
            "0123456789abcdef0123456789abcde",   // 31 文字
            "0123456789abcdef0123456789abcdef0", // 33 文字
            "0123456789ABCDEF0123456789ABCDEF",  // 大文字
            "0123456789abcdef0123456789abcdeg",  // 16 進数でない文字
            "0123456789abcdef-123456789abcdef",
        ] {
            assert_eq!(WorkspaceId::parse(text), None, "{text:?}");
        }
    }
}
