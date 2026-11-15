#!/usr/bin/env bash
# ローカルで動かした Worker（wrangler dev）に API を呼び、実際の D1 と R2 を通して動くかを確かめる。
# 単体テスト（cargo test）は D1 と R2 の代わりに偽物を使うので、SQL と R2 の読み書きそのものはここで確かめる。
#
# 使い方: scripts/smoke-test.sh [ベースの URL]（省略すると http://localhost:8787）
# ワークスペースとレシピを作るので、本番には使わない。
# 環境変数 CHECK_RATE_LIMIT=1 のときだけ、最後に回数の制限を確かめる（CI で使う）。
# この確認は 60 秒の枠を使い切るので、続けて実行すると最初のワークスペースの作成が 429 になる。
# 1 回の実行でワークスペースを最大 22 個作りうるので、同じローカルの D1 で同じ日に 2 回以上実行すると、
# アプリ全体の 1 日の上限（20 個）に当たることがある。そのときは 503 で止まり、作り直し方を示す。
set -euo pipefail

base_url="${1:-http://localhost:8787}"
body_file="$(mktemp)"
photo_file="$(mktemp)"
big_photo_file="$(mktemp)"
trap 'rm -f "$body_file" "$photo_file" "$big_photo_file"' EXIT

fail() {
  echo "::error::$*" >&2
  echo "--- 最後の応答の本文 ---" >&2
  cat "$body_file" >&2 || true
  exit 1
}

# 書き込む API が回数の制限（60 秒に 30 回）で 429 になったら、待ってからやり直すか。
# 制限そのものを確かめるときだけ false にする。
retry_on_write_limit=true

# request <メソッド> <パス> [ワークスペースの ID] [JSON の本文、または @<ファイル>]
# 本文が @ で始まれば、そのファイルの中身をバイト列のまま送る（写真の API）。
# 応答の本文を $body_file に書き、状態コードを $status に入れる。
request() {
  local method="$1" path="$2" workspace="${3:-}" body="${4:-}"
  local args=(-sS -o "$body_file" -w '%{http_code}' -X "$method")
  if [ -n "$workspace" ]; then
    args+=(-H "X-Workspace-Id: $workspace")
  fi
  if [[ "$body" == @* ]]; then
    args+=(-H 'Content-Type: application/octet-stream' --data-binary "$body")
  elif [ -n "$body" ]; then
    args+=(-H 'Content-Type: application/json' --data "$body")
  fi
  local attempt
  for attempt in $(seq 1 8); do
    status="$(curl "${args[@]}" "$base_url$path")"
    if [ "$status" != 429 ] || [ "$retry_on_write_limit" != true ] \
      || [ "$method" = GET ] || [[ "$path" != /api/recipes* ]]; then
      return
    fi
    echo "  書き込みの回数の制限に当たったので、10 秒待ってやり直す（$attempt 回目）"
    sleep 10
  done
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
# デモ（.dev.vars の DEMO_WORKSPACE_ID）があればデモを、無ければ「はじめる」画面を返す。
grep -q -e 'はじめる' -e '共有のデモ' "$body_file" || fail "Cookie が無いのに / が「はじめる」画面もデモも返さない"
# CSS と htmx は静的アセットとして配る（wrangler.toml の [assets] と、ビルドでの htmx のコピーを確かめる）。
# 画面の動きは単体テスト（src/tests/screens.rs）で確かめるので、ここでは操作しない。
request GET /assets/style.css
expect 200 "GET /assets/style.css"
request GET /assets/htmx.min.js
expect 200 "GET /assets/htmx.min.js"
grep -q 'htmx' "$body_file" || fail "/assets/htmx.min.js が htmx でない"

echo "ワークスペースを 2 つ作る"
request POST /api/workspaces
if [ "$status" = 429 ]; then
  fail "ワークスペースを作る回数の制限に当たった。60 秒待ってから、もう一度実行する"
fi
if [ "$status" = 503 ]; then
  fail "アプリ全体のワークスペースの上限（1 日 20 個）に当たった。ローカルの D1 を作り直す（apps/recipe-box-rs/.wrangler/state/v3/d1 を消す）か、UTC の翌日に実行する"
fi
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
# _ と \ を含む料理名も、文字そのものとして探せる（instr はワイルドカードを持たない）。
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
# 上限いっぱい（100 文字）の日本語でも検索できる（instr は D1 の LIKE のパターンの上限 50 バイトを受けない）。
long_name="$(printf 'あ%.0s' $(seq 1 100))"
request POST /api/recipes "$workspace_a" "{\"name\":\"$long_name\",\"servings\":1}"
expect 201 "POST /api/recipes（100 文字の料理名）"
request GET "/api/recipes?q=$(jq -rn --arg q "$long_name" '$q | @uri')" "$workspace_a"
expect 200 "GET /api/recipes?q=<100 文字の日本語>"
[ "$(json '.recipes | length')" = 1 ] || fail "100 文字の日本語で検索できない"

echo "ページ分けと、レシピの数の上限（40 件）"
# 別のワークスペースに、項目の長さも上限いっぱい（材料 2,000 文字・手順 5,000 文字）のレシピを 40 件作る。
ingredients="$(printf 'あ%.0s' $(seq 1 2000))"
steps="$(printf 'い%.0s' $(seq 1 5000))"
for n in $(seq 1 40); do
  request POST /api/recipes "$workspace_b" \
    "{\"name\":\"料理 $n\",\"servings\":1,\"ingredients\":\"$ingredients\",\"steps\":\"$steps\"}"
  expect 201 "POST /api/recipes（料理 $n）"
  # 並びを確かめるため、作った日時が同じミリ秒にならないよう少し待つ。
  sleep 0.01
done
request POST /api/recipes "$workspace_b" '{"name":"41 件目","servings":1}'
expect 409 "41 件目のレシピ"
[ "$(json .error)" = recipe_limit_reached ] || fail "41 件目のエラーのコードが recipe_limit_reached でない"
request GET /api/recipes "$workspace_b"
expect 200 "GET /api/recipes（1 ページ目）"
[ "$(json '.recipes | length')" = 20 ] || fail "1 ページ目が 20 件でない"
[ "$(json .total)" = 40 ] || fail "総数が 40 でない"
[ "$(json '.recipes[0].name')" = "料理 40" ] || fail "1 ページ目が新しい順でない"
request GET '/api/recipes?page=2' "$workspace_b"
expect 200 "GET /api/recipes?page=2"
[ "$(json '.recipes | length')" = 20 ] || fail "2 ページ目が 20 件でない"
[ "$(json '.recipes[19].name')" = "料理 1" ] || fail "2 ページ目の最後が最も古いレシピでない"
request GET '/api/recipes?q=%E6%96%99%E7%90%86%201&page=1' "$workspace_b"
expect 200 "GET /api/recipes?q=料理 1"
[ "$(json .total)" = 11 ] || fail "検索の総数が 11（料理 1・10〜19）でない"
request GET '/api/recipes?page=3' "$workspace_b"
expect 400 "page が 3（上限の 2 を超える）"
[ "$(json .field)" = page ] || fail "page が 3 のエラーの項目が page でない"

echo "取得・更新"
request GET "/api/recipes/$recipe_id" "$workspace_a"
expect 200 "GET /api/recipes/{id}"
request PUT "/api/recipes/$recipe_id" "$workspace_a" '{"name":"辛口カレー","servings":2}'
expect 200 "PUT /api/recipes/{id}"
[ "$(json .name)" = "辛口カレー" ] || fail "更新した料理名が返らない"
[ "$(json .ingredients)" = "" ] || fail "PUT で省いた材料が空にならない"
# 更新で検索用の列（search_name）も作り直される（偽物の Backend では確かめられないので、ここで確かめる）。
request GET '/api/recipes?q=%E8%BE%9B%E5%8F%A3' "$workspace_a"
expect 200 "GET /api/recipes?q=辛口（A、更新の後）"
[ "$(json .total)" = 1 ] || fail "更新した料理名で検索できない（search_name が更新されていない）"
request GET '/api/recipes?q=beef' "$workspace_a"
expect 200 "GET /api/recipes?q=beef（A、更新の後）"
[ "$(json .total)" = 0 ] || fail "更新前の料理名で検索に当たる"

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
request GET '/api/recipes?q=%E8%BE%9B%E5%8F%A3' "$workspace_b"
expect 200 "GET /api/recipes?q=辛口（B）"
[ "$(json .total)" = 0 ] || fail "B の一覧に A のレシピが出る"
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

echo "写真（R2）"
# PNG の先頭のバイトで始まる、小さなデータ。
printf '\x89PNG\r\n\x1a\nsmoke-test' >"$photo_file"
request POST "/api/recipes/$recipe_id/photos" "$workspace_a" "@$photo_file"
expect 201 "POST /api/recipes/{id}/photos"
[ "$(json .content_type)" = image/png ] || fail "写真の形式が image/png でない"
photo_id="$(json .id)"
request GET "/api/recipes/$recipe_id/photos" "$workspace_a"
expect 200 "GET /api/recipes/{id}/photos"
[ "$(json '.photos | length')" = 1 ] || fail "写真の一覧が 1 件でない"
request GET "/api/recipes/$recipe_id/photos/$photo_id" "$workspace_a"
expect 200 "GET /api/recipes/{id}/photos/{photo_id}"
cmp -s "$body_file" "$photo_file" || fail "R2 から読んだ写真が、送ったものと違う"
request GET "/api/recipes/$recipe_id/photos/$photo_id" "$workspace_b"
expect 404 "GET /api/recipes/{id}/photos/{photo_id}（B から）"
# 4,000,000 バイトを 1 バイト超える写真は、Content-Length で断る。
head -c 4000001 /dev/zero >"$big_photo_file"
request POST "/api/recipes/$recipe_id/photos" "$workspace_a" "@$big_photo_file"
expect 413 "4,000,001 バイトの写真"

echo "削除"
request DELETE "/api/recipes/$recipe_id" "$workspace_a"
expect 204 "DELETE /api/recipes/{id}"
request GET "/api/recipes/$recipe_id" "$workspace_a"
expect 404 "消したレシピ"
# レシピを消すと、写真の行も（D1 の ON DELETE CASCADE で）消える。
request GET "/api/recipes/$recipe_id/photos/$photo_id" "$workspace_a"
expect 404 "消したレシピの写真"

# 最後に、回数の制限（Rate Limiting）を確かめる。
if [ "${CHECK_RATE_LIMIT:-}" = 1 ]; then
  # 数え始めが 60 秒の枠の途中でも確かめられるよう、枠 2 つ分より 1 回多く試す。
  echo "書き込む API の回数の制限（60 秒に 30 回）"
  retry_on_write_limit=false
  limited=false
  for _ in $(seq 1 61); do
    request DELETE "/api/recipes/$recipe_id" "$workspace_a"
    if [ "$status" = 429 ]; then
      limited=true
      break
    fi
    expect 404 "DELETE /api/recipes/{id}（制限の確認）"
  done
  [ "$limited" = true ] || fail "書き込みの回数の制限を超えても 429 にならない"

  # ワークスペースを作る回数の制限（60 秒に 10 回）。上と同じく、枠 2 つ分より 1 回多く試す。
  echo "ワークスペースを作る回数の制限"
  limited=false
  for _ in $(seq 1 21); do
    request POST /api/workspaces
    if [ "$status" = 429 ]; then
      limited=true
      break
    fi
    expect 201 "POST /api/workspaces（制限の確認）"
  done
  [ "$limited" = true ] || fail "回数の制限を超えても 429 にならない"
fi

echo "すべて通った"
