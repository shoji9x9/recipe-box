# 方針と技術構成

## 方針

- リライト前は Rust で書き、Cloudflare の Workers・D1・R2 で動かす。
- リライト後の技術スタック（言語・フレームワーク・実行環境・DB・写真の置き場など）は決めない。リライトをする人が選ぶ。
  リライトの題材として差が出るよう、リライト前は、Web アプリでよく使われる TypeScript の構成（React・Hono など）とは異なる技術を選んでいる。
- 画面のほかに API を持たせる。リライトの前後で同じ API を保ち、テストに使う。
- SQL は標準の範囲で書く。D1 は SQLite なので、SQLite だけで通じる書き方を避け、リライトで別の DB（PostgreSQL など）に移しても差が出ないようにする。
- ログイン機能は作らない。利用者の情報を持たずに、ワークスペースの ID でデータを分ける。

## リライトの比較の基準

このリポジトリは、一通り開発した後も、セキュリティの対応と依存の版の更新を続ける。リライトを始める時期と担当は決まっていないので、リライトの間も現行は変わりうる。
そのため、リライトをする人が、基準にする commit を自分で選び、その後に入った変更のうち挙動が変わるものを見分けられるようにしておく。

### 基準はローカルの環境にする

比較の基準（現行の側）は、本番ではなく、選んだ commit をローカルの `wrangler dev` で動かした環境にする。作り方は [development.md](development.md) の「リライトの比較の基準の環境」にある。

- 本番の Workers の実行環境は、Cloudflare が更新し続けるので固定できない。`compatibility_date`（`wrangler.toml`）で挙動は保たれるが、実行環境の版そのものは変わる。
- ローカルなら、同じ commit から、同じ版の組み合わせで何度でも作り直せる。ツールは `mise.lock`、依存は `Cargo.lock` と `pnpm-lock.yaml` で決まる。
  Workers の実行環境（workerd）は、`pnpm-lock.yaml` で固定した wrangler の版で決まる。D1 と R2 も、wrangler がローカルに作るもの（Miniflare）を使う。
- 新しい側の実行環境は、リライトをする人が選び、自分で作り直せる。比べる基準として固定が要るのは、現行の側だけである。
- 基準の環境には、Cloudflare のアカウントは要らない。`wrangler dev` は、何も指定しなければローカルの実行環境だけで動き、ログインを求めない。
  このアプリが使う D1・R2・Rate Limiting・静的アセットは、どれもローカルのものを使う。CI の「Build and smoke test」の job も、Cloudflare の認証情報を渡さずに Worker を起動し、API を確かめている。
  アカウントが要るのは、本番に触る操作（デプロイ・`mise run pin -- --remote`・`mise run seed -- --remote`・本番の `wrangler tail`）だけである。
  ただし、wrangler は、何も指定しなければ利用統計を Cloudflare に送る。止めるときは、環境変数 `WRANGLER_SEND_METRICS=false` を付けて実行する。

### 同じ API で確かめる

基準の環境と新しい側の両方に、同じスモークテスト（`scripts/smoke-test.ts`）を当てて、API が同じに動くかを確かめられる。
新しい側には `API_ONLY=1` を付けて当てる。今の作りに頼る確かめ（画面の文字・静的アセット・`public/_headers`）を外し、API（[spec.md](spec.md) の「API」）だけを確かめる。
定期の自動削除の確かめ（`SCHEDULED_LOG`）も、wrangler でローカルの D1 の表を書き換え、ログの文を読むので、新しい側には付けない（[development.md](development.md) の「テスト」）。

### ローカルと本番で違うもの

| もの | ローカル（基準の環境） | 本番 |
| --- | --- | --- |
| 定期実行（Cron Trigger） | 時刻が来ても動かない。`wrangler dev --test-scheduled` で起動し、`/__scheduled` を呼んだときに動く | `wrangler.toml` の `[triggers]` の時刻に動く |
| 回数の制限（Rate Limiting）の数え方 | 時計にそろえた 60 秒の枠で数え、1 つの枠の中では上限の回数ちょうどで `429` になる（ワークスペースの作成は 11 回目）。枠が変わると数え直すので、枠の境目をまたぐと 11 回目も通る。数は `.wrangler/state` に残り、起動し直しても同じ枠の中なら引き継ぐ | Cloudflare の拠点ごとに数える、おおよその制限である。上限の回数ちょうどで `429` になるとは限らない |
| 回数の制限で見分ける IP アドレス（`cf-connecting-ip`） | 送る側が付けたヘッダーの値が、そのまま使われる。付けなければ、wrangler が接続元のアドレス（`127.0.0.1` など）を入れる | Cloudflare が接続元のアドレスを入れる。送る側は変えられない |
| 共有のデモの ID（`DEMO_WORKSPACE_ID`） | `apps/recipe-box-rs/.dev.vars` に書く | Worker の secret |
| ログ | `wrangler dev` を起動した端末に出る | Workers のログ（`[observability]`）に残る |

次のものは、ローカルと本番で同じかをまだ確かめていない。比べるときに差が出たら、ここに足す。

- Workers の CPU 時間の上限で止まるか。
- D1 の大きさ・1 つのクエリの大きさの上限と、超えたときのエラーの出方。
- R2 の上限と、エラーの出方。

### 挙動が変わった変更を見分ける

外から見える挙動（API の応答・上限・画面）を変えるときは、仕様（[spec.md](spec.md)）と、上限などの費用の対策（[security.md](security.md)）と、作りや運用の説明を変えるならアプリの解説（`guide/about.md`）を一緒に直し、PR のタイトルを `feat:` か `fix:` にする（AGENTS.md の「ブランチと PR」）。
依存の更新とセキュリティの対応でも同じにする。PR は squash でマージするので、PR のタイトルがそのまま main の commit message になる。
そのため、基準の commit（下の `<基準>`）から後に挙動が変わった変更は、次の 2 つで分かる。

```bash
# 仕様と上限の変化
git diff <基準>..origin/main -- docs/spec.md docs/security.md
# 挙動を変えた変更（feat・fix・revert と、! の付いたもの）
git log --format='%h %s' <基準>..origin/main | grep -E '^[0-9a-f]+ (feat|fix|revert)(\([^)]*\))?!?:|^[0-9a-f]+ [a-z]+(\([^)]*\))?!:'
```

Dependabot の更新のうち自動でマージされたもの（`dependabot-automerge.yml`）は、人が確かめる前に `chore(deps):` のまま main に入る。
後から挙動が変わったと分かったときは、`docs/spec.md` などを直す `fix:` の PR の本文に、挙動を変えた元の PR を書く（AGENTS.md）。
そのため、挙動が実際に変わった commit は、上の一覧に出た `fix:` の PR の本文で確かめる。元の PR が基準より前なら、基準の環境もその挙動である。

### リライトで技術を選ぶときの観点

- seed（共有のデモのデータ、`mise run seed`）を、アプリと同じデータの層（ORM など）で入れられるか。
  今は D1 に外から直接つなぐ方法が無いので、seed はレシピを公開 API で入れ、写真を wrangler で D1 の SQL と R2 に直接入れている（デモでは API で写真を追加できないため）。
  そのため、写真の表の列・R2 のキーの形・形式の判定・上限の値を、アプリから seed に写している。写したものが食い違うと、毎日の `seed-demo.yml` が失敗して分かる。

## 技術構成（リライト前）

| 用途 | 使うもの |
| --- | --- |
| 言語 | Rust。開発用の Node のスクリプト（seed など）は TypeScript（Node がそのまま動かす）。画面の小さな JavaScript は、ビルドしないよう JavaScript のまま（型は JSDoc） |
| Web フレームワーク | [axum](https://github.com/tokio-rs/axum)（[workers-rs](https://github.com/cloudflare/workers-rs) の `http` 機能で Workers 上で動かす） |
| 画面 | [askama](https://github.com/askama-rs/askama)（HTML テンプレート）と [htmx](https://htmx.org/)。編集の画面で写真をまとめて追加し、追加・削除の後に写真の欄だけを入れ替える小さな JavaScript（`apps/recipe-box-rs/public/assets/photo-upload.js`）と、検索で日本語の変換中に検索しないための JavaScript（`apps/recipe-box-rs/public/assets/search.js`）と、htmx で入れ替えるときの読み込み中の写真の取りやめと、hx-boost で移った後のフォーカスと、Invoker Commands に対応していないブラウザでの削除のダイアログの開閉を補う JavaScript（`apps/recipe-box-rs/public/assets/page.js`）だけを自分で書く。どれも ES モジュール。ビルドの手順を増やさないよう、TypeScript にせず JavaScript のまま配り、型は JSDoc で書いて `tsc --noEmit` で確かめる |
| スタイル | 1 つの CSS ファイル（`apps/recipe-box-rs/public/assets/style.css`）。色・主な余白・幅を変数にし、CSS の入れ子（`&`）で部品ごとにまとめる。文字の役割などの見た目の決まりは [design.md](design.md) |
| 実行環境 | Cloudflare Workers |
| DB | Cloudflare D1 |
| 写真の保管先 | Cloudflare R2 |
| デプロイと開発用のサーバー | wrangler |
| テスト | Rust は `cargo test`。JavaScript（画面と Node のスクリプト）は [vitest](https://vitest.dev/) で、テストは TypeScript で書く。画面は [happy-dom](https://github.com/capricorn86/happy-dom)（ブラウザの代わりの DOM）で動かす。テストの仕組みを 1 つにそろえるため、Node に入っている `node --test` は使わない |
| リンターとフォーマッター | clippy・rustfmt・taplo（TOML）・rumdl（Markdown）・TypeScript の型の確認（`tsc --noEmit`） |
| ツールの版の管理 | [mise](https://mise.jdx.dev/) |
| Node のパッケージマネージャー | pnpm |
| git フック | lefthook |

使えるブラウザの下限は、画面の CSS が使う入れ子と `:has()` に対応しているかで決まる。
削除の確認に使う Invoker Commands（docs/spec.md）に対応していないブラウザでも、画面の JavaScript（`public/assets/page.js`）が同じダイアログを開くので、下限は変わらない。

Cloudflare の新しい CLI の `cf` は 2026-09-28 に open beta になった。ただし公式ブログによると、Rust の Worker の開発とデプロイは wrangler に任せている。そのため wrangler を使う。リライト前の構成は wrangler で足りるので、`cf` に移すかは検討しない。
