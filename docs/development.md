# 開発

## 開発の始め方

### 前提

- [mise](https://mise.jdx.dev/)
- [GitHub CLI](https://cli.github.com/)（`gh auth login` で認証しておく）
- Google Chrome（エージェントが chrome-devtools MCP で画面を確かめるときだけ要る）
- `curl` と `jq`（`mise run smoke` のスモークテストで使う）
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
| `mise run smoke` | 起動中のローカルの Worker に API を呼び、実際の D1 と R2（をまねたもの）を通して確かめる（先に別の端末で `mise run dev`）。ワークスペースを作る回数の制限の確認は、`CHECK_RATE_LIMIT=1` を付けたときだけ行う |
| `mise run test` | Rust のテストを実行する |
| `mise run build` | Worker をビルドする |
| `mise run fmt` | Rust・TOML・Markdown を整形する |
| `mise run lint` | 整形の確認と静的解析（clippy・taplo・rumdl・actionlint・ghalint・zizmor・shellcheck） |
| `mise run pin -- (--local \| --remote) [--unpin] < <ID のファイル>` | ワークスペースに、自動削除と削除から外す印を付ける（外す）。共有のデモに使う管理用のコマンドで、本番（`--remote`）には wrangler の認証が要る。デモの ID を表示やシェルの履歴に残さないよう、ID は標準入力で渡す |
| `mise run audit` | 依存の脆弱性・ライセンス・取得元・署名を調べる（cargo-deny・pnpm audit signatures） |

commit のときは、lefthook が整形・静的解析・gitleaks・commitlint を実行する。

### ディレクトリ

| パス | 内容 |
| --- | --- |
| `apps/recipe-box-rs/` | リライト前の Rust の Worker |
| `apps/recipe-box-rs/migrations/` | D1 のマイグレーション（テーブルを作る・変える SQL） |
| `scripts/smoke-test.sh` | ローカルの Worker に API を呼んで確かめるスクリプト（CI でも使う） |
| `.github/workflows/` | CI・デプロイ・レビューなどの workflow と、その lockfile（`actions.lock`） |

## CI と CD

| workflow | 内容 |
| --- | --- |
| `ci.yml` | PR ごとに、静的解析・依存の検査・テスト・ビルドと動作確認・秘密情報の検査を行う |
| `pr-title.yml` | PR のタイトルが Conventional Commits の形かを調べる（タイトルを直したときにも動く） |
| `codeql.yml` | Rust のコードと workflow を CodeQL で解析する |
| `claude-review.yml` | PR を Claude Code でレビューする |
| `deploy.yml` | main に入った変更を Cloudflare Workers へデプロイする。D1 と R2 のバケットが無ければ作り、先に D1 にマイグレーションを当てる |
| `dependabot-automerge.yml` | Dependabot の PR のうち、1.0 以上の依存の minor と patch の更新を自動でマージする |
| `outdated.yml` | mise のツールに新しい版があれば、追跡用の Issue を作る |

main は、ブランチルールで PR を通さない変更と強制 push を禁止し、CI の成功を必須にしている。

D1 のデータベース（`recipe-box`）は、無ければ `deploy.yml` が作る。`wrangler.toml` にはアカウントごとの `database_id` を書かず、wrangler に名前で探させる。
写真を置く R2 のバケット（`recipe-box-photos`）も、無ければ `deploy.yml` が作る。ローカル（`mise run dev`）では、wrangler が R2 をまねたものを `.wrangler/state` に作る。

使われていないワークスペースの自動削除は、Cron Trigger（`wrangler.toml` の `[triggers]`）で毎日動く。ローカルで確かめるときは、`apps/recipe-box-rs` で `pnpm exec wrangler dev --test-scheduled` を起動し、`curl "http://localhost:8787/__scheduled?cron=0+18+*+*+*"` で呼ぶ。

本番は `<名前>.workers.dev` で公開する（`wrangler.toml` の `workers_dev = true`）。版ごとのプレビューの URL は、誰でも開ける URL を増やさないよう作らない（`preview_urls = false`）。

### デプロイとレビューの準備

GitHub の Settings → Environments に、次の値を置く。

| Environment | 種類 | 名前 | 値 |
| --- | --- | --- | --- |
| `production` | secret | `CLOUDFLARE_API_TOKEN` | Cloudflare のダッシュボードの「Account API tokens」で作る。権限は「Edit Cloudflare Workers」のテンプレート（R2 の書き込みの権限「Workers R2 Storage Write」を含む）と「D1 Edit」にし、対象はデプロイするアカウントだけに絞る |
| `production` | secret | `CLOUDFLARE_ACCOUNT_ID` | Cloudflare のアカウント ID。認証情報ではないが、公開リポジトリの Actions のログに出さないよう secret にする |
| `claude-review` | secret | `CLAUDE_CODE_OAUTH_TOKEN` | `claude setup-token` で作る |

値が無い間、`deploy.yml` は何もせずに終わり、`claude-review.yml` は失敗する。
