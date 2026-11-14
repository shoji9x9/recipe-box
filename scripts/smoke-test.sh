#!/usr/bin/env bash
# ローカルで動かした Worker（wrangler dev）に API を呼び、実際の D1 を通して動くかを確かめる。
# 単体テスト（cargo test）は D1 の代わりに偽物を使うので、SQL そのものはここで確かめる。
#
# 使い方: scripts/smoke-test.sh [ベースの URL]（省略すると http://localhost:8787）
# ワークスペースとレシピを作るので、本番には使わない。
set -euo pipefail

base_url="${1:-http://localhost:8787}"
body_file="$(mktemp)"
trap 'rm -f "$body_file"' EXIT

fail() {
  echo "::error::$*" >&2
  echo "--- 最後の応答の本文 ---" >&2
  cat "$body_file" >&2 || true
  exit 1
}

# request <メソッド> <パス> [ワークスペースの ID] [JSON の本文]
# 応答の本文を $body_file に書き、状態コードを $status に入れる。
request() {
  local method="$1" path="$2" workspace="${3:-}" body="${4:-}"
  local args=(-sS -o "$body_file" -w '%{http_code}' -X "$method")
  if [ -n "$workspace" ]; then
    args+=(-H "X-Workspace-Id: $workspace")
  fi
  if [ -n "$body" ]; then
    args+=(-H 'Content-Type: application/json' --data "$body")
  fi
  status="$(curl "${args[@]}" "$base_url$path")"
}

# expect <状態コード> <説明>
expect() {
  if [ "$status" != "$1" ]; then
    fail "$2: 状態コードが $1 でなく $status"
  fi
}

# json <jq の式>: 最後の応答の本文から値を取り出す。
json() {
  jq -r "$1" "$body_file"
}

echo "ヘルスチェックと画面"
request GET /api/health
expect 200 "GET /api/health"
[ "$(cat "$body_file")" = '{"status":"ok"}' ] || fail "/api/health の本文が違う"
request GET /
expect 200 "GET /"
grep -q '<h1>recipe-box</h1>' "$body_file" || fail "/ が期待した HTML を返さない"

echo "ワークスペースを 2 つ作る"
request POST /api/workspaces
expect 201 "POST /api/workspaces"
workspace_a="$(json .id)"
[[ "$workspace_a" =~ ^[0-9a-f]{32}$ ]] || fail "ワークスペースの ID の形が違う"
request POST /api/workspaces
expect 201 "POST /api/workspaces（2 つ目）"
workspace_b="$(json .id)"

echo "レシピを登録する"
request POST /api/recipes "$workspace_a" '{"name":"Beef Curry","servings":4,"ingredients":"牛肉\n玉ねぎ","steps":"煮る"}'
expect 201 "POST /api/recipes"
recipe_id="$(json .id)"
[ "$(json .ingredients)" = $'牛肉\n玉ねぎ' ] || fail "材料の改行が保たれていない"
# 一覧の並び順（作った日時の新しい順）を確かめるため、作った日時が同じミリ秒にならないよう少し待つ。
sleep 0.01
request POST /api/recipes "$workspace_a" '{"name":"100% Juice","servings":1}'
expect 201 "POST /api/recipes（2 件目）"

echo "一覧と検索"
request GET /api/recipes "$workspace_a"
expect 200 "GET /api/recipes"
[ "$(json '.recipes | length')" = 2 ] || fail "一覧の件数が 2 でない"
[ "$(json '.recipes[0].name')" = "100% Juice" ] || fail "一覧が新しい順でない"
request GET '/api/recipes?q=CURRY' "$workspace_a"
expect 200 "GET /api/recipes?q=CURRY"
[ "$(json '[.recipes[].name] | join(",")')" = "Beef Curry" ] || fail "大文字の検索が部分一致しない"
request GET '/api/recipes?q=%25' "$workspace_a"
expect 200 "GET /api/recipes?q=%"
[ "$(json '[.recipes[].name] | join(",")')" = "100% Juice" ] || fail "% を文字として検索しない"
# _ と \ を含む料理名も、文字そのものとして探せる（D1 から読んだ結果をアプリで絞り込む）。
request POST /api/recipes "$workspace_a" '{"name":"A_B","servings":1}'
expect 201 "POST /api/recipes（A_B）"
request POST /api/recipes "$workspace_a" '{"name":"AxB","servings":1}'
expect 201 "POST /api/recipes（AxB）"
request POST /api/recipes "$workspace_a" '{"name":"a\\b","servings":1}'
expect 201 "POST /api/recipes（a\\b）"
request GET '/api/recipes?q=_' "$workspace_a"
expect 200 "GET /api/recipes?q=_"
[ "$(json '[.recipes[].name] | join(",")')" = "A_B" ] || fail "_ を文字として検索しない"
request GET '/api/recipes?q=%5C' "$workspace_a"
expect 200 "GET /api/recipes?q=\\"
[ "$(json '[.recipes[].name] | join(",")')" = 'a\b' ] || fail "\\ を文字として検索しない"
# 上限いっぱい（100 文字）の日本語でも検索できる（D1 の LIKE のパターンの上限 50 バイトに当たらない）。
long_name="$(printf 'あ%.0s' $(seq 1 100))"
request POST /api/recipes "$workspace_a" "{\"name\":\"$long_name\",\"servings\":1}"
expect 201 "POST /api/recipes（100 文字の料理名）"
request GET "/api/recipes?q=$(jq -rn --arg q "$long_name" '$q | @uri')" "$workspace_a"
expect 200 "GET /api/recipes?q=<100 文字の日本語>"
[ "$(json '.recipes | length')" = 1 ] || fail "100 文字の日本語で検索できない"

echo "取得・更新"
request GET "/api/recipes/$recipe_id" "$workspace_a"
expect 200 "GET /api/recipes/{id}"
request PUT "/api/recipes/$recipe_id" "$workspace_a" '{"name":"辛口カレー","servings":2}'
expect 200 "PUT /api/recipes/{id}"
[ "$(json .name)" = "辛口カレー" ] || fail "更新した料理名が返らない"
[ "$(json .ingredients)" = "" ] || fail "PUT で省いた材料が空にならない"

echo "入力の検証"
request POST /api/recipes "$workspace_a" '{"name":"","servings":2}'
expect 400 "空の料理名"
[ "$(json .field)" = name ] || fail "空の料理名のエラーの項目が name でない"
request POST /api/recipes "$workspace_a" '{"name":"カレー","servings":0}'
expect 400 "servings が 0"
# servings は書き方ではなく値で決める（2.0 は 2 として受け付け、桁あふれは invalid_field）。
request POST /api/recipes "$workspace_a" '{"name":"小数の書き方","servings":2.0}'
expect 201 "servings が 2.0"
[ "$(json .servings)" = 2 ] || fail "servings の 2.0 が 2 にならない"
request DELETE "/api/recipes/$(json .id)" "$workspace_a"
expect 204 "servings が 2.0 のレシピを消す"
request POST /api/recipes "$workspace_a" '{"name":"カレー","servings":99999999999999999999}'
expect 400 "servings が桁あふれ"
[ "$(json .field)" = servings ] || fail "桁あふれのエラーの項目が servings でない"

echo "ほかのワークスペースからは見えない"
request GET /api/recipes "$workspace_b"
expect 200 "GET /api/recipes（B）"
[ "$(json '.recipes | length')" = 0 ] || fail "B の一覧に A のレシピが出る"
for method in GET PUT DELETE; do
  request "$method" "/api/recipes/$recipe_id" "$workspace_b" '{"name":"乗っ取り","servings":1}'
  expect 404 "$method /api/recipes/{id}（B から）"
done
request GET "/api/recipes/$recipe_id" "$workspace_a"
expect 200 "GET /api/recipes/{id}（A から、B の操作の後）"
[ "$(json .name)" = "辛口カレー" ] || fail "B の操作で A のレシピが変わった"

echo "ワークスペースの ID の誤り"
request GET /api/recipes
expect 400 "X-Workspace-Id が無い"
request GET /api/recipes ffffffffffffffffffffffffffffffff
expect 404 "無いワークスペース"

echo "削除"
request DELETE "/api/recipes/$recipe_id" "$workspace_a"
expect 204 "DELETE /api/recipes/{id}"
request GET "/api/recipes/$recipe_id" "$workspace_a"
expect 404 "消したレシピ"

# 最後に、ワークスペースを作る回数の制限（60 秒に 10 回）を確かめる。
# 上で 2 回作ったので、続けて作ると 10 回以内に 429 になる。
echo "ワークスペースを作る回数の制限"
limited=false
for _ in $(seq 1 10); do
  request POST /api/workspaces
  if [ "$status" = 429 ]; then
    limited=true
    break
  fi
  expect 201 "POST /api/workspaces（制限の確認）"
done
[ "$limited" = true ] || fail "回数の制限を超えても 429 にならない"

echo "すべて通った"
