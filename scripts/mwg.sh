#!/usr/bin/env bash
# modern-web-guidance（Web の作り方の指針）を引く。mise run mwg から呼ぶ（AGENTS.md の「スキル」）。
# - 版は package.json で固定したものを使う（npx の @latest を使わない）。
# - 利用統計（検索の語を含む）を Google に送らない。
# - 読むだけのサブコマンド（search・list・retrieve）に限る。install・update などは、版を固定していない
#   パッケージを npx で取ってきて、スキルを書き換えるため。
set -euo pipefail

# ツール（Node の parseArgs）と同じく、オプションを除いた最初の引数をサブコマンドとみなす。
# - --skill-version は値を 1 つ取る。
# - -- の後は、- で始まる語も位置の引数になる。- だけの語も位置の引数である。
command=""
skip_value=false
after_double_dash=false
for arg in "$@"; do
  if [[ "$after_double_dash" == true ]]; then
    command="$arg"
    break
  elif [[ "$skip_value" == true ]]; then
    skip_value=false
  elif [[ "$arg" == "--" ]]; then
    after_double_dash=true
  elif [[ "$arg" == "--skill-version" ]]; then
    skip_value=true
  elif [[ "$arg" == "-" || "$arg" != -* ]]; then
    command="$arg"
    break
  fi
done
case "$command" in
  search | list | retrieve) ;;
  # サブコマンドが無ければ、ツールは版（-v）か使い方を出すだけである。
  "") ;;
  *)
    echo "使えるのは search・list・retrieve だけです（scripts/mwg.sh）" >&2
    exit 2
    ;;
esac
export DISABLE_TELEMETRY=1
exec pnpm exec modern-web-guidance "$@"
