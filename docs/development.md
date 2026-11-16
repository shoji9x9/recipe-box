# 開発

## 開発の始め方

### 前提

- [mise](https://mise.jdx.dev/)
- [GitHub CLI](https://cli.github.com/)（`gh auth login` で認証しておく）
- Google Chrome（エージェントが chrome-devtools MCP で画面を確かめるときだけ要る）
- `curl` と `jq`（自動削除と共有のデモをローカルで試す手順で使う。どちらも下の「CI と CD」にある）
- OS のパッケージの `pkg-config` と OpenSSL の開発用ファイル。worker-build のビルドに要る。
  Ubuntu では `sudo apt-get install -y pkg-config libssl-dev` で入る。

### 準備

```bash
mise install    # mise.lock のとおりに Rust・Node・pnpm・各種の検査ツールを入れる
pnpm install    # wrangler・commitlint・vitest・TypeScript・chrome-devtools-mcp・modern-web-guidance などを入れ、lefthook の git フックを有効にする
```

### よく使うコマンド

| コマンド | 内容 |
| --- | --- |
| `mise run dev` | ローカルの D1 にマイグレーションを当て、Worker を起動する（<http://localhost:8787>） |
| `mise run migrate` | ローカルの D1 にマイグレーションを当てる |
| `mise run smoke` | 起動中のローカルの Worker に API を呼び、実際の D1 と R2（をまねたもの）を通して確かめる（先に別の端末で `mise run dev`）。ワークスペースを作る回数の制限の確認は、`CHECK_RATE_LIMIT=1` を付けたときだけ行う |
| `mise run test` | Rust のテストと、JavaScript の単体テスト（vitest。画面とスクリプト。下の「テスト」）を実行する |
| `mise run build` | Worker をビルドする |
| `mise run fmt` | Rust・TOML・Markdown を整形する |
| `mise run lint` | 整形の確認と静的解析（clippy・taplo・rumdl・actionlint・ghalint・zizmor・shellcheck・JavaScript とテストの型の確認（`tsc`））と、modern-web-guidance のスキルとパッケージの版がそろっているかの確認 |
| `mise run pin -- (--local \| --remote) [--unpin] < <ID のファイル>` | ワークスペースに、自動削除と削除から外す印を付ける（外す）。共有のデモに使う管理用のコマンドで、本番（`--remote`）には wrangler の認証が要る。デモの ID を表示やシェルの履歴に残さないよう、ID は標準入力で渡す |
| `mise run seed -- (--local \| --remote) < <ID のファイル>` | 共有のデモを、サンプルのレシピと写真（`seed/`）の状態に戻す。ローカル（`--local`）では先に別の端末で `mise run dev` を起動しておく。本番（`--remote`）には wrangler の認証が要る。印（`mise run pin`）の付いたワークスペースにだけ入れる。ID は標準入力で渡す |
| `mise run mwg -- (search \| list \| retrieve) ...` | Web の作り方の指針（modern-web-guidance）を引く。画面の HTML・CSS・JavaScript を書く前に使う（AGENTS.md の「スキル」）。利用統計を送らず、読むだけのサブコマンドに限る |
| `mise run audit` | 依存の脆弱性・ライセンス・取得元・署名を調べる（cargo-deny・pnpm audit signatures） |

commit のときは、lefthook が整形・静的解析・gitleaks・commitlint を実行する。

### テスト

| 対象 | 置く場所 | 仕組み |
| --- | --- | --- |
| Rust | 外から呼ぶ API と画面は `apps/recipe-box-rs/src/tests/`。モジュールの中だけの関数は、そのファイルの `#[cfg(test)] mod tests` | `cargo test` |
| 画面の JavaScript（`apps/<app>/public/assets/`） | `apps/<app>/js-tests/<名前>.test.ts` | vitest。happy-dom の DOM で動かす |
| スクリプト（`scripts/` の TypeScript とシェルスクリプト） | `scripts/<名前>.test.ts`（スクリプトの隣） | vitest。Node のまま動かす（シェルスクリプトは、子のプロセスとして動かして確かめる） |

- JavaScript のテストは、vitest で TypeScript で書く。どちらの場所かで、vitest が DOM で動かすか Node で動かすかを決める（`vitest.config.ts` の `projects`）。
- 画面のテストを `public/` の下に置かない。`public/` の下のファイルは、Workers の静的アセットとして誰でも取れる。
- 型は `mise run typecheck`（`mise run lint` と commit の前の lefthook から呼ぶ）の `tsc` で確かめる。画面は `tsconfig.json`（DOM の型）、スクリプトとそのテストは `scripts/tsconfig.json`（Node の型）。

### 言語

- Node で動かすスクリプト（`scripts/`）と設定（`commitlint.config.ts`・`vitest.config.ts`）は、TypeScript で書く。
  スクリプトはビルドせず、Node が型の注釈を取り除いてそのまま動かす（`node scripts/seed.ts`）。そのため、Node が取り除けない書き方（`enum`・`namespace`・コンストラクターの引数のプロパティなど）は使わない（`scripts/tsconfig.json` の `erasableSyntaxOnly`）。
  ほかのスクリプトは `./seed-lib.ts` のように `.ts` の拡張子で import する。
- 画面の JavaScript（`public/assets/`）は、ビルドせずにそのまま配るので、JavaScript のまま書き、型は JSDoc で書く（ブラウザは TypeScript を動かせない）。
- シェルスクリプト（`scripts/*.sh`）で書くのは、ほかのコマンドを呼んで並べるだけの手順に限り、shellcheck で確かめる。
  次のどれかに当たれば、TypeScript で書く。
  - コマンドや API の出力（JSON・HTTP の応答）の中身を読んで、次にすることや結果を決める（`jq` で値を取り出すことも含む）。
  - 確かめる項目を並べたテストである（スモークテストなど）。
  - 分岐やくり返しが増え、関数を分けたくなる。目安は 100 行を超えること。
  シェルが向くのは、ツールを決まった引数と環境変数で起動する（`mwg.sh`）、ファイルの中の文字を取り出して比べる（`check-mwg-version.sh`）といった、終了コードと短い文字列だけで決まる手順である。
  TypeScript にすると、コマンドを呼ぶたびに `execFile` と失敗の扱いを書くことになり、かえって長くなる。

### ディレクトリ

| パス | 内容 |
| --- | --- |
| `apps/recipe-box-rs/` | リライト前の Rust の Worker |
| `apps/recipe-box-rs/migrations/` | D1 のマイグレーション（テーブルを作る・変える SQL） |
| `apps/recipe-box-rs/public/` | Workers の静的アセット（CSS・画面の JavaScript・アイコン）。ここに置いたファイルは誰でも取れるので、テストなどを置かない |
| `apps/recipe-box-rs/js-tests/` | 画面の JavaScript の単体テスト（上の「テスト」） |
| `scripts/seed.ts` | 共有のデモを入れ直す seed のスクリプト（`mise run seed`） |
| `scripts/seed-lib.ts` | seed のうち、伏せ字とレシピの検証など外部に触れない部分。単体テストは `scripts/seed-lib.test.ts` |
| `seed/` | デモのサンプルのレシピ（`recipes.json`）と写真（`photos/`。出典と許諾は `photos/SOURCES.md`） |
| `scripts/mwg.sh` | Web の作り方の指針（modern-web-guidance）を、固定した版で、利用統計を送らずに、読むだけのサブコマンドで引く（`mise run mwg`）。単体テストは `scripts/mwg.test.ts` |
| `scripts/check-mwg-version.sh` | modern-web-guidance のスキルとパッケージの版がそろっているかを確かめる（`mise run lint`）。単体テストは `scripts/mwg.test.ts` |
| `scripts/smoke-test.ts` | ローカルの Worker に API を呼んで確かめるスクリプト（`mise run smoke`。CI でも使う） |
| `scripts/pin-workspace.ts` | ワークスペースに、自動削除と削除から外す印を付ける・外す管理用のスクリプト（`mise run pin`）。出力の読み方は `scripts/pin-lib.ts` にあり、単体テストは `scripts/pin-lib.test.ts` |
| `scripts/wrangler.ts` | seed と pin が wrangler を動かす部分（失敗の文の伏せ字と、ID の読み方） |
| `.github/workflows/` | CI・デプロイ・レビューなどの workflow と、その lockfile（`actions.lock`） |

## CI と CD

| workflow | 内容 |
| --- | --- |
| `ci.yml` | PR ごとに、静的解析・依存の検査・テスト・ビルドと動作確認・秘密情報の検査を行う |
| `pr-title.yml` | PR のタイトルが Conventional Commits の形かを調べる（タイトルを直したときにも動く） |
| `codeql.yml` | Rust のコードと workflow を CodeQL で解析する |
| `claude-review.yml` | PR を Claude Code でレビューする |
| `seed-demo.yml` | 毎日 UTC の 19 時（日本時間の 4 時）に、共有のデモを seed で入れ直す（手でも実行できる） |
| `deploy.yml` | main に入った変更を Cloudflare Workers へデプロイする。D1 と R2 のバケットが無ければ作り、先に D1 にマイグレーションを当てる |
| `dependabot-automerge.yml` | Dependabot の PR のうち、1.0 以上の依存の minor と patch の更新を自動でマージする |
| `outdated.yml` | mise のツールに新しい版があれば、追跡用の Issue を作る |

main は、ブランチルールで PR を通さない変更と強制 push を禁止し、CI の成功を必須にしている。

D1 のデータベース（`recipe-box`）は、無ければ `deploy.yml` が作る。`wrangler.toml` にはアカウントごとの `database_id` を書かず、wrangler に名前で探させる。
写真を置く R2 のバケット（`recipe-box-photos`）も、無ければ `deploy.yml` が作る。ローカル（`mise run dev`）では、wrangler が R2 をまねたものを `.wrangler/state` に作る。

使われていないワークスペースの自動削除は、Cron Trigger（`wrangler.toml` の `[triggers]`）で毎日動く。ローカルで確かめるときは、`apps/recipe-box-rs` で `pnpm exec wrangler dev --test-scheduled` を起動し、`curl "http://localhost:8787/__scheduled?cron=0+18+*+*+*"` で呼ぶ。

共有のデモをローカルで試すときは、`mise run dev` を起動した後で、ワークスペースを作って ID をファイルに書き（`curl -s -X POST http://localhost:8787/api/workspaces | jq -r .id > <ID のファイル>`）、`apps/recipe-box-rs/.dev.vars` に `DEMO_WORKSPACE_ID=<ID>` と書いて `mise run dev` を起動し直す。
その後、`mise run pin -- --local < <ID のファイル>` で印を付け、`mise run seed -- --local < <ID のファイル>` でサンプルを入れる。
本番のデモは、毎日 `seed-demo.yml` が入れ直す。すぐに戻したいときは、Actions の画面から手で実行する。
GitHub は、公開リポジトリで 60 日活動（commit など）が無いと、schedule で動く workflow を止める。止まると、デモが入れ直されなくなるので、Actions の画面で `seed-demo.yml` を有効にし直す。

本番は `<名前>.workers.dev` で公開する（`wrangler.toml` の `workers_dev = true`）。版ごとのプレビューの URL は、誰でも開ける URL を増やさないよう作らない（`preview_urls = false`）。

### 本番のログ

- アプリが出したエラー（`src/error.rs` の `log_error`）は、Workers のログ（`wrangler.toml` の `[observability]`）に残る。無料のプランでは 3 日残り、1 日 200,000 件までである。
  Cloudflare のダッシュボードで、Worker（`recipe-box`）の Observability を開いて見る。ワークスペースの ID は、ログに出す前に伏せている。
- リクエストごとの記録（invocation logs）は残さない（docs/security.md の「ログ」）。リクエストごとの状態コードや CPU 時間は、その場で `wrangler tail` で見る。
  `apps/recipe-box-rs` で `pnpm exec wrangler tail recipe-box --format pretty` を実行する（`wrangler login` が要る）。流れている間だけ見え、残らない。
  `--format json` は、リクエストのヘッダー（API の `X-Workspace-Id` を含みうる）も出すので、その出力を人に見せたり、ファイルに残したりしない。
- リクエストの数・エラーの割合・CPU 時間の傾向は、ダッシュボードの、同じ Worker の Metrics で見る。

### デプロイとレビューの準備

GitHub の Settings → Environments に、次の値を置く。

| Environment | 種類 | 名前 | 値 |
| --- | --- | --- | --- |
| `production` | secret | `CLOUDFLARE_API_TOKEN` | Cloudflare のダッシュボードの「Account API tokens」で作る。権限は「Edit Cloudflare Workers」のテンプレート（R2 の書き込みの権限「Workers R2 Storage Write」を含む）と「D1 Edit」にし、対象はデプロイするアカウントだけに絞る |
| `production` | secret | `DEMO_WORKSPACE_ID` | 共有のデモのワークスペースの ID（Worker の secret と同じ値）。`seed-demo.yml` が使う |
| `production` | secret | `CLOUDFLARE_ACCOUNT_ID` | Cloudflare のアカウント ID。認証情報ではないが、公開リポジトリの Actions のログに出さないよう secret にする |
| `claude-review` | secret | `CLAUDE_CODE_OAUTH_TOKEN` | `claude setup-token` で作る |

値が無い間、`deploy.yml` は何もせずに終わり、`seed-demo.yml` と `claude-review.yml` は失敗する。
