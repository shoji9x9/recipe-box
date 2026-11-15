#!/usr/bin/env bash
# modern-web-guidance（Web の作り方の指針）を引く。mise run mwg から呼ぶ（AGENTS.md の「スキル」）。
# - 版は package.json で固定したものを使う（npx の @latest を使わない）。
# - 利用統計（検索の語を含む）を Google に送らない。
# - 読むだけのサブコマンド（search・list・retrieve）に限る。install・update などは、版を固定していない
#   パッケージを npx で取ってきて、スキルを書き換えるため。
set -euo pipefail

# サブコマンドは最初の引数に限る（SKILL.md の使い方も、サブコマンドを先に書く）。
# ツールの引数の読み方（Node の parseArgs と、値を取るオプション）を写すと、ツールの版でずれるため。
# 引数が無いか、版か使い方のオプション 1 つだけなら、ツールは版か使い方を出すだけである。
allowed=false
if [[ $# -eq 0 ]]; then
  allowed=true
else
  case "$1" in
    search | list | retrieve) allowed=true ;;
    -v | --version | -h | --help) [[ $# -eq 1 ]] && allowed=true ;;
  esac
fi
if [[ "$allowed" != true ]]; then
  echo "使えるのは search・list・retrieve だけで、サブコマンドを最初に書きます（scripts/mwg.sh）" >&2
  exit 2
fi
export DISABLE_TELEMETRY=1
exec pnpm exec modern-web-guidance "$@"
