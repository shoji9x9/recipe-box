# recipe-box

料理のレシピを登録・検索できる、小さな Web アプリケーション。

[shoji9x9/skills](https://github.com/shoji9x9/skills) の `replace-strategy` とその姉妹スキル（`golden-dataset`・`parity-suite`・`parity-replace`・`parity-diff` など）を試すための題材として作る。
最初に Rust で作り（リライト前）、後でスキルを使って TypeScript に書き換える（リライト後）。

## 方針

- リライト前は Rust で書き、Cloudflare の Workers・D1・R2 で動かす。
- リライト後は TypeScript・React・Hono・Drizzle などで書き、Cloudflare 以外のプラットフォームで動かす想定とする。オブジェクトストレージは使わない。
  リライトの題材として差が出るよう、リライト前の技術スタックはリライト後となるべく異なるものにする。
- 画面のほかに API を持たせる。リライトの前後で同じ API を保ち、テストに使う。
- SQL は標準の範囲で書く。D1 は SQLite なので、SQLite だけで通じる書き方を避け、PostgreSQL に移しても差が出ないようにする。
- ログイン機能は作らない。利用者の情報を持たずに、ワークスペースの ID でデータを分ける。

## 仕様

### 画面

- レシピの一覧（料理名で検索できる）
- レシピの詳細
- レシピの登録と編集のフォーム
- レシピの削除
- ワークスペースの ID の表示と切り替え

### テーブル

| テーブル | 内容 |
| --- | --- |
| `workspaces` | ワークスペース。最終利用日時と、自動削除の対象から外す印を持つ |
| `recipes` | レシピ。`workspace_id`、`name`（料理名・必須）、`servings`（何人分・正の整数）、`ingredients`（材料・複数行）、`steps`（手順・複数行） |
| 写真 | レシピの写真。`workspace_id`・`recipe_id`・形式・大きさ・保管先のキーなど（列は実装のときに決める） |

### API

| メソッドとパス | 内容 |
| --- | --- |
| `POST /api/workspaces` | ワークスペースを作り、ID を返す |
| `GET /api/recipes` | レシピの一覧 |
| `POST /api/recipes` | レシピの登録 |
| `GET /api/recipes/:id` | レシピの取得 |
| `PUT /api/recipes/:id` | レシピの更新 |
| `DELETE /api/recipes/:id` | レシピの削除（写真も消す） |
| `POST /api/recipes/:id/photos` | 写真を 1 枚追加する |

写真の一覧・取得・削除のパスは、実装のときに決める。

失敗したときは、状態コードと次の形の本文を返す。`error` は機械向けのコード、`message` は人向けの説明である。

```json
{"error": "workspace_not_found", "message": "ワークスペースが見つかりません"}
```

`/api` で始まるが定義されていないパスには `404`（`not_found`）を、パスが受け付けないメソッドには `405`（`method_not_allowed`）を返す。

### ワークスペースによるデータの分離

- `POST /api/workspaces` が、推測できない 128 ビットの乱数の ID を発行し、`201` と `{"id": "<ID>"}` を返す。ID は 16 進数の小文字 32 文字。
- 発行の回数は、1 つの IP アドレス（IPv6 は上位 64 ビットの範囲）から 60 秒に 10 回までにする。超えたら `429`（`too_many_requests`）を返す。
  Cloudflare の Rate Limiting は拠点ごとにおおよそで数えるので、厳密な上限ではない（複数の拠点を通れば、拠点の数だけ増えうる）。
- API は `X-Workspace-Id` ヘッダーで、画面は Cookie（HttpOnly・Secure・SameSite=Lax）で ID を受け取る。
  ヘッダーが無ければ `400`（`workspace_id_missing`）、形が正しくないか 2 つ以上あれば `400`（`workspace_id_invalid`）、そのワークスペースが無ければ `404`（`workspace_not_found`）を返す。
- ワークスペースを使うたびに、最終利用日時を更新する。
- すべてのクエリを `workspace_id` で絞り込む。ID を知っている人だけが、そのワークスペースのデータを読み書きできる。
- 最後に使われてから 30 日たったワークスペースは、定期実行（Cron Trigger）で消す。

### 写真

- 1 つのレシピに複数の写真を付けられる。
- 上限は 1 枚 4,000,000 バイト以下、1 レシピ 5 枚まで。形式は JPEG・PNG・WebP。
- 大きさが上限を超えたら 413、6 枚目を追加しようとしたら 409 を返す。
- リライト前は写真を R2 に、リライト後は DB に保管する。保管先が変わっても、API の仕様は変えない。

### デモ用のデータ

- 自動削除の対象から外した、デモ用のワークスペースを 1 つ持つ。自動削除から外す印は、管理用のコマンドでだけ付けられる。
- seed（`mise run seed`）が、デモ用のワークスペースのレシピを全部消してから、サンプルのレシピと写真を入れ直す。投入先はローカルと本番から選べる。
- seed は公開 API だけを呼ぶので、リライト後も同じ seed を使える。
- サンプルは `seed/recipes.json` と `seed/photos/` に置く。写真は自作かパブリックドメインのものだけを使う。
- デモ用のワークスペースの ID は公開しない。GitHub の Secrets などに置く。

## 技術構成（リライト前）

| 用途 | 使うもの |
| --- | --- |
| 言語 | Rust |
| Web フレームワーク | [axum](https://github.com/tokio-rs/axum)（[workers-rs](https://github.com/cloudflare/workers-rs) の `http` 機能で Workers 上で動かす） |
| 画面 | [askama](https://github.com/askama-rs/askama)（HTML テンプレート）と [htmx](https://htmx.org/) |
| 実行環境 | Cloudflare Workers |
| DB | Cloudflare D1 |
| 写真の保管先 | Cloudflare R2 |
| デプロイと開発用のサーバー | wrangler |
| リンターとフォーマッター | clippy・rustfmt・taplo（TOML）・rumdl（Markdown） |
| ツールの版の管理 | [mise](https://mise.jdx.dev/) |
| Node のパッケージマネージャー | pnpm |
| git フック | lefthook |

Cloudflare の新しい CLI の `cf` は 2026-09-28 に open beta になった。ただし公式ブログによると、Rust の Worker の開発とデプロイは wrangler に任せている。そのため wrangler を使い、リライト後に改めて検討する。

## セキュリティ

- コミットと CI で、gitleaks が秘密情報の混入を調べる。GitHub の secret scanning と push protection も有効にしている。
- 依存の更新と脆弱性の通知は Dependabot が担う。Rust の依存は cargo-deny でも調べる。
- workflow は、job ごとに最小の権限だけを与え、actionlint・ghalint・zizmor・CodeQL で調べる。
- 脆弱性の報告の仕方は [SECURITY.md](SECURITY.md) にある。

### サプライチェーン攻撃の対策

- 公開から 7 日たっていない版は使わない。悪意のある版の多くは、公開から数日のうちに見つかって取り下げられるためである。
  - Dependabot: `cooldown` で 7 日待つ。脆弱性を直す security update は待たずに届く。
  - mise: `minimum_release_age = "7d"`
  - pnpm: `minimumReleaseAge: 10080`（単位は分）
  - Cargo: `registry.global-min-publish-age = "7 days"`（Rust 1.100 から。Rust を 1.100 に上げるときに設定する）
- 依存はすべて lockfile で固定する（`Cargo.lock`・`pnpm-lock.yaml`・`mise.lock`）。CI は lockfile と食い違うと失敗する。
  mise が `cargo install` で入れる worker-build は、配布元の `Cargo.lock` に従って `--locked` でビルドされる。ただし `cargo install` は Cargo の待ち日数の設定を使わないので、worker-build の版は公開から 7 日たったものを手で選ぶ。
- GitHub Actions は workflow lockfile（`.github/workflows/actions.lock`）で、使う action とその action が使う action を commit に固定する。
- pnpm は、許可したパッケージのインストール時のスクリプトだけを実行する。

## 開発の始め方

### 前提

- [mise](https://mise.jdx.dev/)
- [GitHub CLI](https://cli.github.com/)（`gh auth login` で認証しておく）
- Google Chrome（エージェントが chrome-devtools MCP で画面を確かめるときだけ要る）
- OS のパッケージの `pkg-config` と OpenSSL の開発用ファイル。worker-build のビルドに要る。
  Ubuntu では `sudo apt-get install -y pkg-config libssl-dev` で入る。

### 準備

```bash
mise install    # mise.lock のとおりに Rust・Node・pnpm・各種の検査ツールを入れる
pnpm install    # wrangler・commitlint・chrome-devtools-mcp を入れ、lefthook の git フックを有効にする
```

### よく使うコマンド

| コマンド | 内容 |
| --- | --- |
| `mise run dev` | ローカルの D1 にマイグレーションを当て、Worker を起動する（<http://localhost:8787>） |
| `mise run migrate` | ローカルの D1 にマイグレーションを当てる |
| `mise run test` | Rust のテストを実行する |
| `mise run build` | Worker をビルドする |
| `mise run fmt` | Rust・TOML・Markdown を整形する |
| `mise run lint` | 整形の確認と静的解析（clippy・taplo・rumdl・actionlint・ghalint・zizmor） |
| `mise run audit` | 依存の脆弱性・ライセンス・取得元・署名を調べる（cargo-deny・pnpm audit signatures） |

commit のときは、lefthook が整形・静的解析・gitleaks・commitlint を実行する。

### ディレクトリ

| パス | 内容 |
| --- | --- |
| `apps/recipe-box-rs/` | リライト前の Rust の Worker |
| `apps/recipe-box-rs/migrations/` | D1 のマイグレーション（テーブルを作る・変える SQL） |
| `.github/workflows/` | CI・デプロイ・レビューなどの workflow と、その lockfile（`actions.lock`） |

## CI と CD

| workflow | 内容 |
| --- | --- |
| `ci.yml` | PR ごとに、静的解析・依存の検査・テスト・ビルドと動作確認・秘密情報の検査を行う |
| `pr-title.yml` | PR のタイトルが Conventional Commits の形かを調べる（タイトルを直したときにも動く） |
| `codeql.yml` | Rust のコードと workflow を CodeQL で解析する |
| `claude-review.yml` | PR を Claude Code でレビューする |
| `deploy.yml` | main に入った変更を Cloudflare Workers へデプロイする。先に D1 にマイグレーションを当てる |
| `dependabot-automerge.yml` | Dependabot の PR のうち、1.0 以上の依存の minor と patch の更新を自動でマージする |
| `outdated.yml` | mise のツールに新しい版があれば、追跡用の Issue を作る |

main は、ブランチルールで PR を通さない変更と強制 push を禁止し、CI の成功を必須にしている。

D1 のデータベース（`recipe-box`）は、無ければ `deploy.yml` が作る。`wrangler.toml` にはアカウントごとの `database_id` を書かず、wrangler に名前で探させる。

本番は `<名前>.workers.dev` で公開する（`wrangler.toml` の `workers_dev = true`）。版ごとのプレビューの URL は、誰でも開ける URL を増やさないよう作らない（`preview_urls = false`）。

### デプロイとレビューの準備

GitHub の Settings → Environments に、次の値を置く。

| Environment | 種類 | 名前 | 値 |
| --- | --- | --- | --- |
| `production` | secret | `CLOUDFLARE_API_TOKEN` | Cloudflare のダッシュボードの「Account API tokens」で作る。権限は「Edit Cloudflare Workers」と「D1 Edit」にし、対象はデプロイするアカウントだけに絞る |
| `production` | variable | `CLOUDFLARE_ACCOUNT_ID` | Cloudflare のアカウント ID。認証情報ではないが、公開リポジトリには書かない |
| `claude-review` | secret | `CLAUDE_CODE_OAUTH_TOKEN` | `claude setup-token` で作る |

値が無い間、`deploy.yml` は何もせずに終わり、`claude-review.yml` は失敗する。
