//! ワークスペース。ログインの代わりに、推測できない ID でデータの持ち主を分ける。

use std::{
    fmt,
    net::{IpAddr, Ipv6Addr},
};

use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{HeaderMap, StatusCode, request::Parts},
};
use serde::Serialize;

use crate::{backend::Backend, error::ApiError, id};

/// API がワークスペースの ID を受け取るヘッダー。
pub const WORKSPACE_ID_HEADER: &str = "x-workspace-id";

/// ワークスペースを作る人を見分けるのに使うヘッダー。Cloudflare が接続元の IP アドレスを入れる。
const CLIENT_IP_HEADER: &str = "cf-connecting-ip";

/// 最終利用日時を書き換える間隔（1 時間）。
///
/// 使うたびに書き換えると、読むだけの API でも毎回 DB に書き込むことになる。
/// 自動削除は 14 日使われていないかで決めるので、1 時間の粗さで足りる。
pub const LAST_USED_UPDATE_INTERVAL_MILLIS: i64 = 60 * 60 * 1000;

/// ワークスペースの ID。128 ビットの乱数を、16 進数の小文字 32 文字で表したもの。
///
/// 値を作れるのは [`WorkspaceId::from_bytes`] と [`WorkspaceId::parse`] だけなので、
/// この型の値は、いつも正しい形をしている。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// 16 バイトの乱数から ID を作る。
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(id::hex_from_bytes(bytes))
    }

    /// 文字列が ID の形をしていれば、ID にする。
    pub fn parse(value: &str) -> Option<Self> {
        id::is_valid(value).then(|| Self(value.to_owned()))
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
#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
}

impl<B: Backend> FromRequestParts<B> for Workspace {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, backend: &B) -> Result<Self, Self::Rejection> {
        let mut values = parts.headers.get_all(WORKSPACE_ID_HEADER).iter();
        let value = values.next().ok_or(ApiError::WorkspaceIdMissing)?;
        // 同じヘッダーが 2 つ以上あると、どちらのワークスペースか決められないので受け付けない。
        if values.next().is_some() {
            return Err(ApiError::WorkspaceIdInvalid);
        }
        // ASCII でない値（to_str が失敗する）・空・前後の空白も、ID の形でないので受け付けない。
        let id = value
            .to_str()
            .ok()
            .and_then(WorkspaceId::parse)
            .ok_or(ApiError::WorkspaceIdInvalid)?;
        let last_used_at = backend
            .workspace_last_used_at(&id)
            .await?
            .ok_or(ApiError::WorkspaceNotFound)?;
        // 前に書き換えてから 1 時間以上たっていたときだけ、最終利用日時を書き換える。
        let now = backend.now_millis();
        if now - last_used_at >= LAST_USED_UPDATE_INTERVAL_MILLIS {
            backend.set_workspace_last_used_at(&id, now).await?;
        }
        Ok(Self { id })
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
/// - IPv4 は、アドレスそのものをキーにする。IPv4 射影アドレス（`::ffff:192.0.2.1`）も IPv4 として扱う。
/// - IPv6 は、上位 64 ビットだけをキーにする。1 つの回線には /64 以上の範囲が割り当てられることが多く、
///   アドレスをそのまま使うと、範囲の中でアドレスを変えるだけで制限を避けられるため。
/// - ヘッダーが無いか、IP アドレスとして読めないときは、まとめて `unknown` として数える。
///   Cloudflare を通るリクエストには必ずヘッダーが付くので、これに当たるのはローカルでの開発だけである。
///   IP アドレスとして読んだ値だけを使うので、キーの長さは限られる。
fn rate_limit_key(headers: &HeaderMap) -> String {
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

    use super::rate_limit_key;

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
    fn rate_limit_key_treats_ipv4_mapped_address_as_ipv4() {
        assert_eq!(key_for(Some("::ffff:192.0.2.1")), "192.0.2.1");
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
}
