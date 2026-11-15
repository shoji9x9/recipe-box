#!/usr/bin/env bash
# ワークスペースに、自動削除と削除から外す印（workspaces.pinned）を付ける・外す。管理用で、公開の API にはしない。
# 共有のデモのワークスペースに付ける（docs/spec.md の「デモ用のデータ」）。
#
# 使い方: scripts/pin-workspace.sh (--local | --remote) [--unpin] < <ID を書いたファイル>
#   --local:  ローカルの D1（mise run dev が使うもの）
#   --remote: 本番の D1。wrangler の認証（wrangler login か CLOUDFLARE_API_TOKEN）が要る
# デモの ID は公開しないので、引数ではなく標準入力で受け取る（引数は mise の表示やシェルの履歴に残る）。ID は表示しない。
# 例: mise run pin -- --remote < ~/demo-workspace-id
set -euo pipefail

usage() {
  echo "使い方: $0 (--local | --remote) [--unpin] < <ID を書いたファイル>" >&2
  exit 2
}

[ $# -ge 1 ] && [ $# -le 2 ] || usage
target="$1"
value=TRUE
case "$target" in
  --local | --remote) ;;
  *) usage ;;
esac
if [ $# = 2 ]; then
  [ "$2" = --unpin ] || usage
  value=FALSE
fi
# 1 行目だけを読む（末尾の改行や空白は除く）。
IFS= read -r id || true
id="${id//[[:space:]]/}"
# ID は SQL に埋め込むので、形（16 進数の小文字 32 文字）を必ず確かめる。
if ! [[ "$id" =~ ^[0-9a-f]{32}$ ]]; then
  echo "ワークスペースの ID の形が正しくない（16 進数の小文字 32 文字）" >&2
  exit 2
fi

cd "$(dirname "$0")/../apps/recipe-box-rs"
# wrangler d1 execute は値を別に渡せないので、形を確かめた ID を埋め込む。
# ローカルの D1 は変えた件数を返さないので、続けて同じ ID の印を読み、変わったかを確かめる。
sql="UPDATE workspaces SET pinned = $value WHERE id = '$id'; SELECT pinned FROM workspaces WHERE id = '$id'"
if ! output="$(pnpm exec wrangler d1 execute DB "$target" --json --command "$sql" 2>&1)"; then
  printf '%s\n' "${output//$id/***}" >&2
  exit 1
fi
# D1（SQLite）は BOOLEAN を 0 と 1 で返す。
pinned="$(printf '%s' "$output" | jq -r '.[-1].results[0].pinned // "none"')"
case "$value:$pinned" in
  TRUE:1 | FALSE:0) ;;
  *:none)
    echo "その ID のワークスペースが無い" >&2
    exit 1
    ;;
  *)
    echo "印を変えられなかった" >&2
    exit 1
    ;;
esac
if [ "$value" = TRUE ]; then
  echo "印を付けた（自動削除と削除の対象から外した）"
else
  echo "印を外した"
fi
