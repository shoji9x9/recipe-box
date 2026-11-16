# 方針と技術構成

## 方針

- リライト前は Rust で書き、Cloudflare の Workers・D1・R2 で動かす。
- リライト後は TypeScript・React・Hono・Drizzle などで書き、Cloudflare 以外のプラットフォームで動かす想定とする。オブジェクトストレージは使わない。
  リライトの題材として差が出るよう、リライト前の技術スタックはリライト後となるべく異なるものにする。
- 画面のほかに API を持たせる。リライトの前後で同じ API を保ち、テストに使う。
- SQL は標準の範囲で書く。D1 は SQLite なので、SQLite だけで通じる書き方を避け、PostgreSQL に移しても差が出ないようにする。
- ログイン機能は作らない。利用者の情報を持たずに、ワークスペースの ID でデータを分ける。

## 技術構成（リライト前）

| 用途 | 使うもの |
| --- | --- |
| 言語 | Rust。開発用の Node のスクリプト（seed など）は TypeScript（Node がそのまま動かす）。画面の小さな JavaScript は、ビルドしないよう JavaScript のまま（型は JSDoc） |
| Web フレームワーク | [axum](https://github.com/tokio-rs/axum)（[workers-rs](https://github.com/cloudflare/workers-rs) の `http` 機能で Workers 上で動かす） |
| 画面 | [askama](https://github.com/askama-rs/askama)（HTML テンプレート）と [htmx](https://htmx.org/)。写真をまとめて追加する小さな JavaScript（`apps/recipe-box-rs/public/assets/photo-upload.js`。ES モジュール）だけを自分で書く。ビルドの手順を増やさないよう、TypeScript にせず JavaScript のまま配り、型は JSDoc で書いて `tsc --noEmit` で確かめる |
| スタイル | 1 つの CSS ファイル（`apps/recipe-box-rs/public/assets/style.css`）。色・主な余白・幅を変数にし、CSS の入れ子（`&`）で部品ごとにまとめる |
| 実行環境 | Cloudflare Workers |
| DB | Cloudflare D1 |
| 写真の保管先 | Cloudflare R2 |
| デプロイと開発用のサーバー | wrangler |
| テスト | Rust は `cargo test`。JavaScript（画面と Node のスクリプト）は [vitest](https://vitest.dev/) で、テストは TypeScript で書く。画面は [happy-dom](https://github.com/capricorn86/happy-dom)（ブラウザの代わりの DOM）で動かす。テストの仕組みを 1 つにそろえるため、Node に入っている `node --test` は使わない |
| リンターとフォーマッター | clippy・rustfmt・taplo（TOML）・rumdl（Markdown）・TypeScript の型の確認（`tsc --noEmit`） |
| ツールの版の管理 | [mise](https://mise.jdx.dev/) |
| Node のパッケージマネージャー | pnpm |
| git フック | lefthook |

CSS の入れ子に対応したブラウザは、削除の確認に使う Invoker Commands（docs/spec.md）に対応したブラウザより広い。そのため、入れ子を使っても、使えるブラウザは変わらない。

Cloudflare の新しい CLI の `cf` は 2026-09-28 に open beta になった。ただし公式ブログによると、Rust の Worker の開発とデプロイは wrangler に任せている。そのため wrangler を使う。リライト後は Cloudflare 以外で動かす想定なので、`cf` に移すかは検討しない。
