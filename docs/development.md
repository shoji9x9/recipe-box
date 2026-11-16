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
| `mise run guide` | アプリの解説（`/about/`）を、`guide/` の元の文章と図から作り直す（下の「解説」）。`mise run lint` は、作り直したものが commit したものと同じかを確かめる |
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
| `guide/` | アプリの解説（`/about/`）の元の文章（`about.md`）と図の元（`*.d2`。`_style.d2` は図に共通する設定） |
| `scripts/guide.ts` | 解説を作るスクリプト（`mise run guide`）。印からコードを取り出す部分などは `scripts/guide-lib.ts`、単体テストは `scripts/guide-lib.test.ts` |
| `seed/` | デモのサンプルのレシピ（`recipes.json`）と写真（`photos/`。出典と許諾は `photos/SOURCES.md`） |
| `scripts/mwg.sh` | Web の作り方の指針（modern-web-guidance）を、固定した版で、利用統計を送らずに、読むだけのサブコマンドで引く（`mise run mwg`）。単体テストは `scripts/mwg.test.ts` |
| `scripts/check-mwg-version.sh` | modern-web-guidance のスキルとパッケージの版がそろっているかを確かめる（`mise run lint`）。単体テストは `scripts/mwg.test.ts` |
| `scripts/smoke-test.ts` | ローカルの Worker に API を呼んで確かめるスクリプト（`mise run smoke`。CI でも使う） |
| `scripts/pin-workspace.ts` | ワークスペースに、自動削除と削除から外す印を付ける・外す管理用のスクリプト（`mise run pin`）。出力の読み方は `scripts/pin-lib.ts` にあり、単体テストは `scripts/pin-lib.test.ts` |
| `scripts/wrangler.ts` | seed と pin が wrangler を動かす部分（失敗の文の伏せ字と、ID の読み方） |
| `.github/workflows/` | CI・デプロイ・レビューなどの workflow と、その lockfile（`actions.lock`） |

### 解説（/about）

アプリの作りの解説（`/about/`）は、元の文章（`guide/about.md`）と図の元（`guide/*.d2`）から `mise run guide` で作る。作ったもの（`apps/recipe-box-rs/public/about/index.html` と `apps/recipe-box-rs/public/assets/guide/*.svg`）も commit する。手で直さない。

- Markdown は markdown-it（`package.json` で版を固定）で HTML にし、図は D2（`mise.toml` で版を固定）で SVG にする。どちらも外部と通信しない。
  - D2 は文字を英字だけのフォントで描くので、日本語の形が英字とそろうよう、生成が SVG のフォントの指定を書き換える（ゴシックを候補に足し、太字は太さを書く。`scripts/guide-lib.ts` の `fontFallback`）。
    D2 の版を上げて出力の形が変わると、生成が失敗する。そのときは、新しい出力に合わせて `fontFallback` を直す。
  - 線の文字は斜体にしない（日本語は斜めにならず、英字とそろわない）。各図の元の `...@_style` の次に `(** -> **)[*].style.italic: false` を書く（読み込んだ設定の glob は、読み込んだ先の図に効かないため）。
- コードと設定は、写して貼らない。`guide/about.md` に `<!-- code: <パス>#<名前> -->` と書き、引用する範囲をそのファイルのコメントの `guide:start <名前>` と `guide:end <名前>` で囲む。印が無いと、生成が失敗する。
  印のあるコードを動かしたり消したりするときは、印も一緒に動かし、`mise run guide` で作り直す。
- 図は `<!-- diagram: <名前> | <図の説明> -->` と書き、`guide/<名前>.d2` に図の元を置く。
  - 図の元の先頭で `...@_style` と書き、共通の設定（`guide/_style.d2`）を読む。配置は ELK（線を水平と垂直の線分で描く）にし、色は役割ごとの class（`actor`・`app`・`service`・`store` など）で付ける。
  - フロー図では、楕円を始まりと終わり、ひし形を分かれ道（「〜か？」の問いの形で書く）、四角を処理にする。断る（失敗の）流れは必ずひし形から出し、赤の破線（`reject`）で描く。処理の中で決まる失敗は、処理の後に「うまくいったか？」のひし形を置いて、そこから出す（処理の前の確かめに移さない。コードの順と食い違うため）。
  - 線の交差を最も避け、次に曲がりを減らす。直線で結べるものは直線にする。交差が残るときは、`direction`・要素と線を書く順・線の結び先（枠か、中の要素か）を変えて試す。
  - 作った SVG は、ブラウザで開いて目で確かめる（`mise run dev` の後に `http://localhost:8787/assets/guide/<名前>.svg`）。
- リポジトリのファイルへのリンクは `[文字](repo:<パス>)` と書く。公開リポジトリの main のファイルへのリンクになり、ファイルが無ければ生成が失敗する。
- 上限などの値は、解説に書かずに、原本（docs/spec.md・docs/security.md）へのリンクにする。
- コードや文書を変えて引用の元が変わったら、`mise run guide` で作り直して一緒に commit する。CI（`mise run lint`）は、作り直したものが commit したものと違えば失敗する。

## CI と CD

| workflow | 内容 |
| --- | --- |
| `ci.yml` | PR ごとに、静的解析・依存の検査・テスト・ビルドと動作確認・秘密情報の検査を行う |
| `pr-title.yml` | PR のタイトルが Conventional Commits の形かを調べる（タイトルを直したときにも動く） |
| `codeql.yml` | Rust のコード・JavaScript と TypeScript・workflow を CodeQL で解析する |
| `claude-review.yml` | PR を Claude Code でレビューする |
| `seed-demo.yml` | 毎日 UTC の 19 時（日本時間の 4 時）に、共有のデモを seed で入れ直す（手でも実行できる） |
| `deploy.yml` | main に入った変更を Cloudflare Workers へデプロイする。D1 と R2 のバケットが無ければ作り、先に D1 にマイグレーションを当てる。デプロイの後に、本番の API・トップの画面・静的アセットが 200 を返すかを、読むだけのリクエストで確かめる |
| `dependabot-automerge.yml` | Dependabot の PR のうち、1.0 以上の依存の minor と patch の更新を自動でマージする。GITHUB_TOKEN でのマージでは `deploy.yml` の push が動かないので、マージされるのを待って `deploy.yml` を起動する |
| `outdated.yml` | mise のツールに新しい版があれば、追跡用の Issue を作る |

main は、ブランチルールで PR を通さない変更と強制 push を禁止し、CI の成功を必須にしている。

D1 のデータベース（`recipe-box`）は、無ければ `deploy.yml` が作る。`wrangler.toml` にはアカウントごとの `database_id` を書かず、wrangler に名前で探させる。
写真を置く R2 のバケット（`recipe-box-photos`）も、無ければ `deploy.yml` が作る。ローカル（`mise run dev`）では、wrangler が R2 をまねたものを `.wrangler/state` に作る。

使われていないワークスペースの自動削除は、Cron Trigger（`wrangler.toml` の `[triggers]`）で毎日動く。ローカルで確かめるときは、`apps/recipe-box-rs` で `pnpm exec wrangler dev --test-scheduled` を起動し、`curl "http://localhost:8787/__scheduled?cron=0+18+*+*+*"` で呼ぶ。

共有のデモをローカルで試すときは、`mise run dev` を起動した後で、ワークスペースを作って ID をファイルに書き（`curl -s -X POST http://localhost:8787/api/workspaces | jq -r .id > <ID のファイル>`）、`apps/recipe-box-rs/.dev.vars` に `DEMO_WORKSPACE_ID=<ID>` と書いて `mise run dev` を起動し直す。
その後、`mise run pin -- --local < <ID のファイル>` で印を付け、`mise run seed -- --local < <ID のファイル>` でサンプルを入れる。
本番のデモは、毎日 `seed-demo.yml` が入れ直す。すぐに戻したいときは、Actions の画面から手で実行する。
GitHub は、公開リポジトリで 60 日活動（commit など）が無いと、schedule で動く workflow を止める。止まると、デモが入れ直されなくなるので、Actions の画面で `seed-demo.yml` を有効にし直す（下の「最低限のメンテナンス」）。
`seed-demo.yml` など schedule で動く workflow が失敗したことは、GitHub Actions の失敗の通知（メールなど）で知らされる。
通知が届くのは、その workflow の cron の行を最後に変えた人（止まった workflow を有効にし直したときは、その人）で、届くかどうかは GitHub の通知の設定（Actions）による。

本番は `<名前>.workers.dev` で公開する（`wrangler.toml` の `workers_dev = true`）。版ごとのプレビューの URL は、誰でも開ける URL を増やさないよう作らない（`preview_urls = false`）。

### 本番のログ

- アプリが出したエラー（`src/error.rs` の `log_error`）は、Workers のログ（`wrangler.toml` の `[observability]`）に残る。無料のプランでは、2026-11-30 までは 3 日残り、1 日 200,000 件までである。2026-12-01 からは 7 日残り、1 日 0.5 GB までに変わる（docs/security.md の「ログ」）。
  Cloudflare のダッシュボードで、Worker（`recipe-box`）の Observability を開いて見る。ワークスペースの ID は、ログに出す前に伏せている。
- リクエストごとの記録（invocation logs）は残さない（docs/security.md の「ログ」）。リクエストごとの状態コードや CPU 時間は、その場で `wrangler tail` で見る。
  `apps/recipe-box-rs` で `pnpm exec wrangler tail recipe-box --format pretty` を実行する（`wrangler login` が要る）。流れている間だけ見え、残らない。
  `--format json` は、リクエストのヘッダー（API の `X-Workspace-Id` を含みうる）も出すので、その出力を人に見せたり、ファイルに残したりしない。
- リクエストの数・エラーの割合・CPU 時間の傾向は、ダッシュボードの、同じ Worker の Metrics で見る。

### デプロイが失敗したとき

`deploy.yml` は、マイグレーションを当ててからコードを入れ替え、最後に本番が応答するかを確かめる。どこで失敗したかで、することが変わる。

1. マイグレーションが失敗したときは、コードは入れ替わらない。それより前のマイグレーション（同じデプロイで先に当たったもの）は残るが、表や列を足すだけに書いているので、古いコードのまま動く。
   原因を直す PR を入れるか、一時的な失敗なら Actions の画面で失敗した run を再実行する。
2. マイグレーションは当たったが `wrangler deploy` が失敗したときも、1 と同じである（古いコードが新しい表のまま動く）。原因を直して、Actions で再実行する。
3. 新しいコードが本番で壊れたとき（最後の確かめが失敗したときを含む）は、前の版に戻す。
   `apps/recipe-box-rs` で `pnpm exec wrangler rollback [<version-id>]`（`wrangler login` が要る。版を省くと 1 つ前の版）を実行するか、ダッシュボードの Worker（`recipe-box`）の Deployments から戻す。
   戻るのはコードと設定だけで、バインディングとデータ（D1・R2）は戻らない。マイグレーションを足すだけにしているのは、このためである。
   次に main へ push するとまたデプロイされるので、戻した後は、直すか revert する PR を続けて入れる。
   （[Rollbacks](https://developers.cloudflare.com/workers/configuration/versions-and-deployments/rollbacks/)）
4. マイグレーションのファイルは書き換えない。当てたマイグレーションを取り消すときも、新しい番号のマイグレーションを足す（AGENTS.md の「SQL」）。
5. D1 のデータを過去の時点に戻すのは、ほかに手が無いときの最後の手段にする。
   D1 の Time Travel で、無料のプランなら 7 日以内の時点に戻せる。`pnpm exec wrangler d1 time-travel info recipe-box --timestamp=<時刻>` でその時点の bookmark を調べ、`pnpm exec wrangler d1 time-travel restore recipe-box --bookmark=<bookmark>` で戻す。
   - 戻すと、その場でデータベースの全体が上書きされ、その時点より後の書き込みはすべて消える。
   - R2 は戻らない。その後に消した写真は、R2 のデータの無い写真の行になり、その後に足した写真は、どの行からもたどれない R2 のデータになる。
   - マイグレーションの記録（`d1_migrations`）も戻るので、戻した時点の表に合う版のコードにそろえる（次のデプロイで、その後のマイグレーションがもう一度当たる）。
   - コマンドの引数は、実行する前に `pnpm exec wrangler d1 time-travel restore --help` で確かめる。
   （[Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/)）

### デプロイとレビューの準備

GitHub の Settings → Environments に、次の値を置く。

| Environment | 種類 | 名前 | 値 |
| --- | --- | --- | --- |
| `production` | secret | `CLOUDFLARE_API_TOKEN` | Cloudflare のダッシュボードの「Account API tokens」で作る。権限は「Edit Cloudflare Workers」のテンプレート（R2 の書き込みの権限「Workers R2 Storage Write」を含む）と「D1 Edit」にし、対象はデプロイするアカウントだけに絞る |
| `production` | secret | `DEMO_WORKSPACE_ID` | 共有のデモのワークスペースの ID（Worker の secret と同じ値）。`seed-demo.yml` が使う |
| `production` | secret | `CLOUDFLARE_ACCOUNT_ID` | Cloudflare のアカウント ID。認証情報ではないが、公開リポジトリの Actions のログに出さないよう secret にする |
| `claude-review` | secret | `CLAUDE_CODE_OAUTH_TOKEN` | `claude setup-token` で作る |

値が無い間、`deploy.yml` は何もせずに終わり、`seed-demo.yml` と `claude-review.yml` は失敗する。

Environment `production` は、「Deployment branches and tags」を「Selected branches and tags」にし、`main` だけを許す。
本番のトークンとデモの ID を、レビューを経て main に入ったコードにだけ渡すためである（ほかのブランチから `workflow_dispatch` で動かしたり、ブランチの workflow が `environment: production` を名乗ったりしても渡さない）。
これを守るのは Environment の設定である（ブランチの workflow は、自分の手順を書き換えられるため）。`deploy.yml` と `seed-demo.yml` は、誤って main 以外から手で動かしたときに気付けるよう、最初の手順で失敗する。

## 最低限のメンテナンス

新しい機能は足さず、依存とツールを新しく保ち、壊れたら直すだけにする。

### 毎週（自動で届くもの）

- Dependabot の PR と、`outdated.yml` が作る mise のツールの Issue を見る。
- 自動でマージされない PR（ラベル `dependabot/needs-review`）は、`dependabot-merge` スキルで確かめてマージする。挙動が変わる更新なら、`fix(deps):` にして文書も直す（AGENTS.md の「ブランチと PR」）。
- `modern-web-guidance` のパッケージを上げる PR では、同じ PR で、同じ版のスキルを入れ直す（AGENTS.md の「スキル」）。
- main が先に進んで、ブランチが古い（BEHIND）まま止まった Dependabot の PR は、PR に `@dependabot rebase` とコメントして作り直させる（main は、ブランチが最新であることを必須にしている）。

### 毎月

- `dependabot-alert-issue` スキルで、Dependabot の alert から Issue を作る。`mise run audit` も実行する。
- `outdated.yml` の Issue にある mise のツールを、まとめて上げる（`mise install` の後に `mise lock`。AGENTS.md の「ツールの版」）。Node は、使っているメジャーの版のサポートの終わり（EOL）が近くないかも見る。

### schedule が止まらないようにする

GitHub は、公開リポジトリで 60 日活動が無いと、schedule で動く workflow（`seed-demo.yml`・`outdated.yml`・`codeql.yml`）を止める。
60 日の間に 1 回は main に commit が入るようにする（Dependabot の PR のマージでもよい）。止まったら、Actions の画面か `gh workflow enable <workflow>` で有効にし直す。

### 放っておくと古くなるもの

| もの | すること |
| --- | --- |
| `wrangler.toml` の `compatibility_date` | ふだんは上げない（古い日付のままでも動き続ける）。上げるときは、その間に既定になる compatibility flag（[一覧](https://developers.cloudflare.com/workers/configuration/compatibility-flags/)）を読み、挙動が変われば `fix:` の PR にする |
| Cloudflare のデプロイのトークン（`CLOUDFLARE_API_TOKEN`） | 期限を付けたなら、切れる前に、同じ権限（上の「デプロイとレビューの準備」）で作り直し、Environment `production` の secret を差し替えて、古いトークンを消す |
| `CLAUDE_CODE_OAUTH_TOKEN` | `claude setup-token` で作ったトークンには期限がある。切れると `claude-review.yml` が失敗するので、作り直して Environment `claude-review` の secret を差し替える |
| 手元で調べるための読み取り専用の Cloudflare のトークン | 作ったなら、期限が切れる前に作り直すか、要らなければ消す |
| `docs/security.md` と、この文書の「本番のログ」の料金 | 「2026-11-30 まで」と「2026-12-01 から」の 2 つを書いている。2026-12-01 を過ぎたら、新しい料金だけの書き方に直す |
| `docs/security.md` の費用の表の「2026-11 時点」（`guide/about.md` にもある） | 料金と無料の範囲は変わる。公式の文書で確かめ直したら、値と時点を直す |
| mise の Rust・Node の版と、`worker-build`・`worker`（`=0.8.7`）の固定 | `outdated.yml` の Issue と Dependabot で上げる。`worker-build`（`mise.toml`）と `worker`（`apps/recipe-box-rs/Cargo.toml`）は同じ版にそろえて一緒に上げる |
| CI の `runs-on: ubuntu-latest` | 指す Ubuntu の版は GitHub が変える。CI が急に落ちたら、まずランナーの版の変化を疑う |
| Cloudflare の Monitors（定期の処理の失敗の通知） | #56 で入れたら、設定が消えていないかを見る |

## 使うのをやめるとき

本番を止めて、Cloudflare のデータと、デプロイとレビューに使うトークンを消す手順。消したデータは戻せない。

1. workflow を止める。`deploy.yml` は D1 と R2 のバケットが無ければ作るので、先に止めないと、後の手順で消しても作り直される。
   ほかの workflow も、トークンを消した後に失敗し続けないよう止める。

   ```bash
   gh workflow disable deploy.yml
   gh workflow disable seed-demo.yml
   gh workflow disable claude-review.yml
   gh workflow disable dependabot-automerge.yml
   gh workflow disable outdated.yml
   ```

   Dependabot の PR も止めるなら、`.github/dependabot.yml` を消す PR をマージする。
2. `apps/recipe-box-rs` で Worker を消す（`wrangler login` が要る）。Cron Trigger・secret・binding も一緒に消える。

   ```bash
   pnpm exec wrangler delete recipe-box
   ```

3. R2 のバケットを空にする。中身があるとバケットを消せず、wrangler には中身をまとめて消すコマンドが無いので、すべてを 1 日で消すライフサイクルのルールを足す。

   ```bash
   pnpm exec wrangler r2 bucket lifecycle add recipe-box-photos expire-all --expire-days 1 -y
   ```

4. 1〜2 日後に、`pnpm exec wrangler r2 bucket info recipe-box-photos` でオブジェクトが 0 個になったことを確かめ、バケットを消す。この数は遅れて変わることがあるので、消すときに空でないと断られたら、時間をおいてやり直す。

   ```bash
   pnpm exec wrangler r2 bucket delete recipe-box-photos
   ```

5. D1 のデータベースを消す。Time Travel（過去の時点に戻す機能）でも戻せなくなる。

   ```bash
   pnpm exec wrangler d1 delete recipe-box
   ```

6. `pnpm exec wrangler d1 list` と `pnpm exec wrangler r2 bucket list` と、Cloudflare のダッシュボードで、Worker・D1・R2 が消えたことを確かめる。
7. トークンを消す。
   - Cloudflare のダッシュボードで、デプロイ用の API トークン（`CLOUDFLARE_API_TOKEN`）と、ほかに作ったトークンがあれば、それも消す。
   - GitHub の Settings → Environments で、`production` と `claude-review` の secret を消す。Environment そのものは消さない（workflow を有効にし直したときに、main だけを許す設定の無い Environment が作り直されないようにするため）。
   - `claude setup-token` で作ったトークン（`CLAUDE_CODE_OAUTH_TOKEN`）を無効にする。
   - 手元の wrangler からログアウトする（`pnpm exec wrangler logout`）。

## リライトの比較の基準の環境

リライトで比べる現行の側は、選んだ commit をローカルで動かした環境にする（理由と、ローカルと本番の違いは [architecture.md](architecture.md) の「リライトの比較の基準」）。Cloudflare のアカウントは要らない。

```bash
git switch --detach <基準の commit>
mise install                     # mise.lock のとおりにツールを入れる
pnpm install --frozen-lockfile   # pnpm-lock.yaml のとおりに wrangler（workerd を含む）などを入れる
rm -rf apps/recipe-box-rs/.wrangler/state   # 前の環境のローカルの D1・R2・回数の制限の数を消し、空から始める
rm -f apps/recipe-box-rs/.dev.vars          # 前に書いた共有のデモの ID（DEMO_WORKSPACE_ID）を残さない
mise run dev                     # ビルドし、ローカルの D1 にマイグレーションを当て、http://localhost:8787 で起動する
```

- 動いていることは、別の端末で `mise run smoke` を実行して確かめる。smoke はワークスペース・レシピ・写真を作り、回数の制限と 1 日の作成の上限の数も使う。
  そのため、確かめた後は、比べ始める前に Worker を止め、`.wrangler/state` を消してから起動し直す。
- 定期実行（使われていないワークスペースの自動削除）を比べるときは、`mise run dev` の代わりに、`mise run migrate` の後で `apps/recipe-box-rs` で `pnpm exec wrangler dev --test-scheduled` を起動する。
  `curl "http://localhost:8787/__scheduled?cron=0+18+*+*+*"` で 1 回動き、結果を起動した端末のログ（「使われていないワークスペースの自動削除」）に出す。
- 共有のデモのデータを入れるときは、上の「CI と CD」の、共有のデモをローカルで試す手順に従う。
