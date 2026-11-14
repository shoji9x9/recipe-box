//! アプリが外の世界（DB・レート制限・乱数・時計）に頼る部分。
//!
//! 本番では Cloudflare の機能を使う実装（`crate::cloudflare`）を、テストではメモリの中で動く偽物を使う。
//! D1 などは wasm の上でしか動かないので、こうして分けておくと、普通の `cargo test` で API を確かめられる。

use std::fmt;
use std::future::Future;

use crate::workspace::WorkspaceId;

/// 外の世界とのやり取りで起きた失敗。中身はログに出すだけで、利用者には見せない。
#[derive(Debug)]
pub struct BackendError(pub String);

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// ハンドラーが使う外の機能。
///
/// axum はハンドラーの `Future` に `Send`（別のスレッドに渡せること）を求めるので、戻り値に `+ Send` を付ける。
/// `Clone` は、axum がリクエストごとに状態を複製するために要る。
pub trait Backend: Clone + Send + Sync + 'static {
    /// 暗号に使える質の 128 ビットの乱数を返す。
    fn random_128(&self) -> [u8; 16];

    /// 今の時刻を UNIX 時間のミリ秒で返す。
    fn now_millis(&self) -> i64;

    /// ワークスペースを作ってよいかを、作る人を表すキー（IP アドレスなど）ごとの回数で決める。
    fn allow_workspace_creation(
        &self,
        client_key: &str,
    ) -> impl Future<Output = Result<bool, BackendError>> + Send;

    /// ワークスペースを保存する。
    fn insert_workspace(
        &self,
        id: &WorkspaceId,
        now: i64,
    ) -> impl Future<Output = Result<(), BackendError>> + Send;

    /// ワークスペースの最終利用日時を `now` にする。ワークスペースが無ければ `false` を返す。
    fn touch_workspace(
        &self,
        id: &WorkspaceId,
        now: i64,
    ) -> impl Future<Output = Result<bool, BackendError>> + Send;
}
