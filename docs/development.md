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
| `mise run smoke` | 起動中のローカルの Worker に API を呼び、実際の D1 と R2（をまねたもの）を通して確かめる（先に別の端末で `mise run dev`）。ワークスペースを作る回数の制限の確認は、`CHECK_RATE_LIMIT=1` を付けたときだけ行う。`API_ONLY=1` を付けると、API だけを確かめる（下の「テスト」） |
| `mise run test` | Rust のテストと、JavaScript の単体テスト（vitest。画面と Worker の JavaScript とスクリプト。下の「テスト」）を実行する |
| `mise run build` | Worker をビルドする |
| `mise run fmt` | Rust・TOML・Markdown を整形する |
| `mise run lint` | 整形の確認と静的解析（clippy・taplo・rumdl・actionlint・ghalint・zizmor・shellcheck・JavaScript とテストの型の確認（`tsc`））と、modern-web-guidance のスキルとパッケージの版がそろっているかの確認 |
| `mise run pin -- (--local \| --remote) [--unpin] < <ID のファイル>` | ワークスペースに、自動削除と削除から外す印を付ける（外す）。共有のデモに使う管理用のコマンドで、本番（`--remote`）には wrangler の認証が要る。デモの ID を表示やシェルの履歴に残さないよう、ID は標準入力で渡す |
| `mise run seed -- (--local \| --remote) < <ID のファイル>` | 共有のデモを、サンプルのレシピと写真（`seed/`）の状態に戻す。ローカル（`--local`）では先に別の端末で `mise run dev` を起動しておく。本番（`--remote`）には wrangler の認証が要る。印（`mise run pin`）の付いたワークスペースにだけ入れる。ID は標準入力で渡す |
| `mise run check-cleanup -- (--local \| --remote)` | 定期の自動削除が 3 時間以内に成功し（D1 の `cleanup_runs`）、消し終えているはずのものが D1 に残っていないかを確かめる。そうでなければ終了コード 1 で止まる（下の「失敗の通知」）。本番（`--remote`）には wrangler の認証が要る |
| `mise run guide` | アプリの解説（`/about/`）を、`guide/` の元の文章と図から作り直す（下の「解説」）。`mise run lint` は、作り直したものが commit したものと同じかを確かめる |
| `mise run size [-- --ref <commit>]` | アプリの規模（ファイル数と行数）を、git の commit（既定は `origin/main`）で数え、解説の元（`guide/about.md`）の「アプリの規模」の表を書き換える（下の「解説」） |
| `mise run mwg -- (search \| list \| retrieve) ...` | Web の作り方の指針（modern-web-guidance）を引く。画面の HTML・CSS・JavaScript を書く前に使う（AGENTS.md の「スキル」）。利用統計を送らず、読むだけのサブコマンドに限る |
| `mise run audit` | 依存の脆弱性・ライセンス・取得元・署名を調べる（cargo-deny・pnpm audit signatures） |

commit のときは、lefthook が整形・静的解析・gitleaks・commitlint を実行する。

### テスト

| 対象 | 置く場所 | 仕組み |
| --- | --- | --- |
| Rust | 外から呼ぶ API と画面は `apps/recipe-box-rs/src/tests/`。モジュールの中だけの関数は、そのファイルの `#[cfg(test)] mod tests` | `cargo test` |
| 画面の JavaScript（`apps/<app>/public/assets/`） | `apps/<app>/js-tests/<名前>.test.ts` | vitest。happy-dom の DOM で動かす |
| Worker の中で動く JavaScript（`apps/<app>/src/*.js`。Rust から wasm-bindgen で呼ぶ） | `apps/<app>/js-tests/worker/<名前>.test.ts` | vitest。DOM の無い Node のまま動かす。Workers だけにある機能（`FixedLengthStream` など）は、テストで代わりのものを置き、型は `apps/<app>/src/*.d.ts` に書く |
| スクリプト（`scripts/` の TypeScript とシェルスクリプト） | `scripts/<名前>.test.ts`（スクリプトの隣） | vitest。Node のまま動かす（シェルスクリプトは、子のプロセスとして動かして確かめる） |

- JavaScript のテストは、vitest で TypeScript で書く。どちらの場所かで、vitest が DOM で動かすか Node で動かすかを決める（`vitest.config.ts` の `projects`）。
- 画面のテストを `public/` の下に置かない。`public/` の下のファイルは、Workers の静的アセットとして誰でも取れる。
- 型は `mise run typecheck`（`mise run lint` と commit の前の lefthook から呼ぶ）の `tsc` で確かめる。画面は `tsconfig.json`（DOM の型）、スクリプトとそのテストは `scripts/tsconfig.json`（Node の型）。
- スモークテスト（`scripts/smoke-test.ts`、`mise run smoke`）は、起動した Worker に HTTP で API を呼び、本物に近い D1・R2・Rate Limiting を通して確かめる。値と応答の形は `docs/spec.md` を正にする。
  確かめるものは、次の 2 つに分かれる。リライトの後の新しいアプリには、`API_ONLY=1` を付け、`SCHEDULED_LOG` を付けずに当てる（`docs/architecture.md` の「リライトの比較の基準」）。
  - API（`docs/spec.md` の「API」）だけで確かめるもの。いつも確かめる。回数の制限は `CHECK_RATE_LIMIT=1` のときだけ確かめる。
  - 今の作りに頼るもの。画面と静的アセット（`/` の文字・`/assets/` のファイル・`/about` と `/assets/guide/` の `public/_headers`）は、`API_ONLY=1` のときは確かめない。
    定期の自動削除（`SCHEDULED_LOG` のときだけ。wrangler でローカルの D1 の表を書き換え、ログの文を読む）も、今の作りに頼る。
  - ワークスペースを作る数が 1 日の作成の上限（docs/security.md の「費用の対策」）に近づかないよう、確かめる項目を足すときは、作ったワークスペースを使い回す。

### 言語

- Node で動かすスクリプト（`scripts/`）と設定（`commitlint.config.ts`・`vitest.config.ts`）は、TypeScript で書く。
  スクリプトはビルドせず、Node が型の注釈を取り除いてそのまま動かす（`node scripts/seed.ts`）。そのため、Node が取り除けない書き方（`enum`・`namespace`・コンストラクターの引数のプロパティなど）は使わない（`scripts/tsconfig.json` の `erasableSyntaxOnly`）。
  ほかのスクリプトは `./seed-lib.ts` のように `.ts` の拡張子で import する。
- 画面の JavaScript（`public/assets/`）は、ビルドせずにそのまま配るので、JavaScript のまま書き、型は JSDoc で書く（ブラウザは TypeScript を動かせない）。
  Worker の中で動く JavaScript（`src/*.js`。wasm-bindgen がそのまま束ねる）も同じにする。
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
| `apps/recipe-box-rs/js-tests/` | 画面の JavaScript の単体テスト（上の「テスト」）。`worker/` の下は Worker の中で動く JavaScript（`src/*.js`）の単体テスト |
| `scripts/seed.ts` | 共有のデモを入れ直す seed のスクリプト（`mise run seed`） |
| `scripts/seed-lib.ts` | seed のうち、伏せ字とレシピの検証など外部に触れない部分。単体テストは `scripts/seed-lib.test.ts` |
| `guide/` | アプリの解説（`/about/`）の元の文章（`about.md`）と図の元（`*.d2`。`_style.d2` は図に共通する設定） |
| `scripts/guide.ts` | 解説を作るスクリプト（`mise run guide`）。印からコードを取り出す部分などは `scripts/guide-lib.ts`、単体テストは `scripts/guide-lib.test.ts` |
| `scripts/size.ts` | アプリの規模を数えて、解説の表を書き換えるスクリプト（`mise run size`）。分類と数え方は `scripts/size-lib.ts` にあり、単体テストは `scripts/size-lib.test.ts` |
| `seed/` | デモのサンプルのレシピ（`recipes.json`）と写真（`photos/`。出典と許諾は `photos/SOURCES.md`） |
| `scripts/mwg.sh` | Web の作り方の指針（modern-web-guidance）を、固定した版で、利用統計を送らずに、読むだけのサブコマンドで引く（`mise run mwg`）。単体テストは `scripts/mwg.test.ts` |
| `scripts/check-mwg-version.sh` | modern-web-guidance のスキルとパッケージの版がそろっているかを確かめる（`mise run lint`）。単体テストは `scripts/mwg.test.ts` |
| `scripts/smoke-test.ts` | ローカルの Worker に API を呼んで確かめるスクリプト（`mise run smoke`。CI でも使う） |
| `scripts/pin-workspace.ts` | ワークスペースに、自動削除と削除から外す印を付ける・外す管理用のスクリプト（`mise run pin`）。出力の読み方は `scripts/pin-lib.ts` にあり、単体テストは `scripts/pin-lib.test.ts` |
| `scripts/check-cleanup.ts` | 定期の自動削除の結果（`cleanup_runs`）を確かめるスクリプト（`mise run check-cleanup`。`seed-demo.yml` と CI が使う）。出力の読み方と判定は `scripts/cleanup-check-lib.ts` にあり、単体テストは `scripts/cleanup-check-lib.test.ts` |
| `scripts/wrangler.ts` | seed と pin が wrangler を動かす部分（失敗の文の伏せ字と、ID の読み方）。失敗の文を作る部分は `scripts/wrangler-lib.ts` にあり、単体テストは `scripts/wrangler-lib.test.ts` |
| `.github/workflows/` | CI・デプロイ・レビューなどの workflow と、その lockfile（`actions.lock`） |

### 解説（/about）

アプリの作りの解説（`/about/`）は、元の文章（`guide/about.md`）と図の元（`guide/*.d2`）から `mise run guide` で作る。作ったもの（`apps/recipe-box-rs/public/about/index.html` と `apps/recipe-box-rs/public/assets/guide/*.svg`）も commit する。手で直さない。

- Markdown は markdown-it（`package.json` で版を固定）で HTML にし、図は D2（`mise.toml` で版を固定）で SVG にする。どちらも外部と通信しない。
  - D2 は SVG にフォント（英字だけを持つ Source Sans Pro）を埋め込むが、生成がそれを取り除き、文字を OS のフォント（画面の文字と同じ並び。日本語はゴシック）で描くように書き換える（太字は太さを書く。`scripts/guide-lib.ts` の `systemFonts`）。
    第三者のフォントを配らないためと、英字と日本語の形をそろえるためである。
    D2 の版を上げて出力の形が変わると、生成が失敗する（フォントを埋め込んだままの図は作らない）。そのときは、新しい出力に合わせて `systemFonts` を直す。
    D2 は文字の幅を埋め込みのフォントで測って図を描くので、OS のフォントでは英字が少し広くなることがある。図を変えたら、文字が枠からはみ出していないかを目で確かめる。
  - 線の文字は斜体にしない（日本語は斜めにならず、英字とそろわない）。各図の元の `...@_style` の次に `(** -> **)[*].style.italic: false` を書く（読み込んだ設定の glob は、読み込んだ先の図に効かないため）。
- コードと設定は、写して貼らない。`guide/about.md` に `<!-- code: <パス>#<名前> -->` と書き、引用する範囲をそのファイルのコメントの `guide:start <名前>` と `guide:end <名前>` で囲む。印が無いと、生成が失敗する。
  印のあるコードを動かしたり消したりするときは、印も一緒に動かし、`mise run guide` で作り直す。
- 図は `<!-- diagram: <名前> | <図の説明> -->` と書き、`guide/<名前>.d2` に図の元を置く。
  - 図の元の先頭で `...@_style` と書き、共通の設定（`guide/_style.d2`）を読む。配置は ELK（線を水平と垂直の線分で描く）にし、色は役割ごとの class（`actor`・`app`・`service`・`store` など）で付ける。
  - フロー図では、楕円を始まりと終わり、ひし形を分かれ道（「〜か？」の問いの形で書く）、四角を処理にする。断る（失敗の）流れは必ずひし形から出し、赤の破線（`reject`）で描く。処理の中で決まる失敗は、処理の後に「うまくいったか？」のひし形を置いて、そこから出す（処理の前の確かめに移さない。コードの順と食い違うため）。
  - 線の交差を最も避け、次に曲がりを減らす。直線で結べるものは直線にする。交差が残るときは、`direction`・要素と線を書く順・線の結び先（枠か、中の要素か）を変えて試す。
  - 作った SVG は、ブラウザで開いて目で確かめる（`mise run dev` の後に `http://localhost:8787/assets/guide/<名前>.svg`）。
- 表の列の揃え（`:---:`・`---:`）は、生成のときに `align-center`・`align-right` のクラス（`guide.css`）に置き換える。markdown-it は揃えを `style` 属性で出すが、/about の CSP（`style-src 'self'`）がインラインのスタイルを禁じていて効かないため。ほかに `style` 属性が残れば、生成が失敗する。
- リポジトリのファイルへのリンクは `[文字](repo:<パス>)` と書く。公開リポジトリの main のファイルへのリンクになり、ファイルが無ければ生成が失敗する。
- 上限などの値は、解説に書かずに、原本（docs/spec.md・docs/security.md）へのリンクにする。
- コードや文書を変えて引用の元が変わったら、`mise run guide` で作り直して一緒に commit する。CI（`mise run lint`）は、作り直したものが commit したものと違えば失敗する。
- 「アプリの規模」の表（`<!-- size:start ... -->` と `<!-- size:end -->` の間）は、`mise run size` が書く。手で直さない。
  - 数え直すときは、`git fetch` の後に `mise run size`（既定で `origin/main` を数える）を実行し、`mise run guide` で作り直して commit する。数え直す PR そのものも数を変えるので、作業中のブランチではなく、その時の main の commit で数え、表にその commit を書く。
  - git で管理しているファイルの一覧と中身を、git から読む（作業中のファイルは見ない）。行数は改行の数（最後の行に改行が無ければ 1 を足す）で、バイナリ（NUL のバイトを含むか、写真などの拡張子）は行数に数えない。
  - 分類の決まりは `scripts/size-lib.ts` の `RULES` にある。どの決まりにも当たらないファイルがあると失敗するので、ファイルを足したら、どこに数えるかを決めて足す。スキル（`.claude/skills/<名前>/`）は、別枠の 1 行にまとめ、スキルの数（ディレクトリの数）を書く。
  - 「画面の CSS とアイコン」には `style.css` と `favicon.svg` を、「CI と設定」には Workers の静的アセットのヘッダー（`public/_headers`）を数える。「その他」は `LICENSE` と `THIRD_PARTY_NOTICES.md` だけである。スクリプトは、言語で TypeScript（`scripts/**/*.ts`。`*.test.ts` はテスト）とシェル（`scripts/**/*.sh`）に分け、`scripts/tsconfig.json` は「CI と設定」に数える。`scripts/` のそのほかのファイルは分類できないとして失敗する。
  - Rust の本番のファイルの末尾のテストのモジュール（`#[cfg(test)]` の次の行が `mod tests {`）は、行をテストに、ファイルを本番に数える。モジュールがファイルの最後の項目でなければ失敗する。
    別のファイルのテストのモジュールの宣言（`#[cfg(test)]` の次の行が `mod tests;`）は、その 2 行をテストに数える（ファイルの途中にあってよい）。
    行の頭の `#[cfg(...test...)]` の後にモジュールが続くのに、この 2 つの書き方でなければ、本番に数えずに失敗する。
  - 数は commit のたびに変わるので、`mise run lint` は表が最新かを確かめない。

## CI と CD

| workflow | 内容 |
| --- | --- |
| `ci.yml` | PR ごとに、静的解析・依存の検査・テスト・ビルドと動作確認・秘密情報の検査を行う |
| `pr-title.yml` | PR のタイトルが Conventional Commits の形かを調べる（タイトルを直したときにも動く） |
| `codeql.yml` | Rust のコード・JavaScript と TypeScript・workflow を CodeQL で解析する |
| `claude-review.yml` | PR を Claude Code でレビューする |
| `seed-demo.yml` | 毎日 UTC の 19 時（日本時間の 4 時）に、共有のデモを seed で入れ直す（手でも実行できる）。別の job で、定期の自動削除が最近に成功し、消し終えているはずのものが残っていないかを確かめる（下の「失敗の通知」） |
| `deploy.yml` | main に入った変更を Cloudflare Workers へデプロイする。D1 と R2 のバケットが無ければ作り、先に D1 にマイグレーションを当てる。デプロイの後に、本番の API・トップの画面・静的アセットが 200 を返すかを、GET で確かめる |
| `dependabot-automerge.yml` | Dependabot の PR のうち、1.0 以上の依存の minor と patch の更新を自動でマージする。GITHUB_TOKEN でのマージでは `deploy.yml` の push が動かないので、マージされるのを待って `deploy.yml` を起動する |
| `outdated.yml` | mise のツールに新しい版があれば、追跡用の Issue を作る |

main は、ブランチルールで PR を通さない変更と強制 push を禁止し、CI の成功を必須にしている。

D1 のデータベース（`recipe-box`）は、無ければ `deploy.yml` が作る。`wrangler.toml` にはアカウントごとの `database_id` を書かず、wrangler に名前で探させる。
写真を置く R2 のバケット（`recipe-box-photos`）も、無ければ `deploy.yml` が作る。ローカル（`mise run dev`）では、wrangler が R2 をまねたものを `.wrangler/state` に作る。

使われていないワークスペースの自動削除は、Cron Trigger（`wrangler.toml` の `[triggers]`）で 1 時間ごとに動く。ローカルで確かめるときは、`apps/recipe-box-rs` で `pnpm exec wrangler dev --test-scheduled` を起動し、`curl "http://localhost:8787/__scheduled?cron=0+*+*+*+*"` で呼ぶ。
結果は起動した端末のログに、「使われていないワークスペースの自動削除が終わった」か「使われていないワークスペースの自動削除で失敗があった」で始まる行で出る。
CI は、`wrangler dev --test-scheduled` のログを `SCHEDULED_LOG` で smoke に渡し、ワークスペースを古くして写真の行に印を付けてから（wrangler でローカルの D1 を書き換える）`/__scheduled` を呼び、失敗が無く、消えることと、成功の結果が `cleanup_runs` に 1 行残ることを確かめる。
その後に `mise run check-cleanup -- --local` を動かし、本番の確かめと同じスクリプトと SQL が通ることを確かめる。
手元のローカルの D1 では、cron が動かない間に古いワークスペースがたまるので、`check-cleanup -- --local` は、先に `/__scheduled` で自動削除を動かしてから使う。1 回に消すのは 8 個（写真の行は 2,000 行）までなので、たまっていれば、通るまで繰り返す。
手元で同じことをするときは、`wrangler dev --test-scheduled` の出力をファイルに書き、`SCHEDULED_LOG=<そのファイル> mise run smoke` を実行する。

共有のデモをローカルで試すときは、`mise run dev` を起動した後で、ワークスペースを作って ID をファイルに書き（`curl -s -X POST http://localhost:8787/api/workspaces | jq -r .id > <ID のファイル>`）、`apps/recipe-box-rs/.dev.vars` に `DEMO_WORKSPACE_ID=<ID>` と書いて `mise run dev` を起動し直す。
その後、`mise run pin -- --local < <ID のファイル>` で印を付け、`mise run seed -- --local < <ID のファイル>` でサンプルを入れる。
本番のデモは、毎日 `seed-demo.yml` が入れ直す。すぐに戻したいときは、Actions の画面から手で実行する。
GitHub は、公開リポジトリで 60 日活動（commit など）が無いと、schedule で動く workflow を止める。止まると、デモが入れ直されなくなるので、Actions の画面で `seed-demo.yml` を有効にし直す（下の「最低限のメンテナンス」）。
`seed-demo.yml` など schedule で動く workflow が失敗したことは、GitHub Actions の失敗の通知（メールなど）で知らされる。
通知が届くのは、その workflow の cron の行を最後に変えた人（止まった workflow を有効にし直したときは、その人）で、届くかどうかは GitHub の通知の設定（Actions）による。

本番は `<名前>.workers.dev` で公開する（`wrangler.toml` の `workers_dev = true`）。版ごとのプレビューの URL は、誰でも開ける URL を増やさないよう作らない（`preview_urls = false`）。

### 本番のログ

- アプリが出したログ（`src/error.rs` の、エラーを出す `log_error` と、定期の自動削除が終わったことなどを出す `log_info`）は、Workers のログ（`wrangler.toml` の `[observability]`）に残る。無料のプランでは、2026-11-30 までは 3 日残り、1 日 200,000 件までである。2026-12-01 からは 7 日残り、1 日 0.5 GB までに変わる（docs/security.md の「ログ」）。
  Cloudflare のダッシュボードで、Worker（`recipe-box`）の Observability を開いて見る。ワークスペースの ID は、ログに出す前に伏せている（アプリが自分で出すログは、この 2 つの関数だけから出す。panic の文は伏せない。docs/security.md の「ログ」）。
- リクエストごとの記録（invocation logs）は残さない（docs/security.md の「ログ」）。リクエストごとの状態コードや CPU 時間は、その場で `wrangler tail` で見る。
  `apps/recipe-box-rs` で `pnpm exec wrangler tail recipe-box --format pretty` を実行する（`wrangler login` が要る）。流れている間だけ見え、残らない。
  `--format json` は、リクエストのヘッダー（API の `X-Workspace-Id` を含みうる）も出すので、その出力を人に見せたり、ファイルに残したりしない。
- リクエストの数・エラーの割合・CPU 時間の傾向は、ダッシュボードの、同じ Worker の Metrics で見る。

### 失敗の通知

定期の自動削除の失敗と、無料の範囲への接近は、次の 2 つで知らせる（Issue #56）。

1. 自動削除の結果を D1 に残し、GitHub Actions で確かめる。
   - 自動削除は、最後に結果（成功したか・消した数・失敗した数）を `cleanup_runs` に残す（docs/spec.md の「ワークスペースによるデータの分離」）。途中で止まった実行は結果を残さない。
     workers-rs の `scheduled` は、処理の結果にかかわらず成功で終わるので、一部の失敗は Cloudflare の側（下の Monitors）からは見えない。この記録で見る。
   - `seed-demo.yml` の job `check-cleanup` が、毎日（seed と同じ時刻）`scripts/check-cleanup.ts --remote` を動かし、次のどちらかなら失敗する。
     - 最後の成功から 3 時間を超えた（cron は 1 時間ごとなので、止まったか、落ち続けている）。止まったことを、確かめの時刻の近くで見つける幅である。
     - 自動削除が消し終えているはずのものが、D1 に残っている。次の 2 つを数える（どれも索引で読む）。
       - 印の無いワークスペースで、最後に使われてから 15 日（自動削除の対象になる 14 日に、1 日を足したもの）を過ぎたもの。
       - 書き込みか削除が終わっていない印を付けてから、1 日と 1 時間（片付けの対象になる 1 時間に、1 日を足したもの）を過ぎた写真の行。
       足した 1 日は、アプリ全体のワークスペースと写真の行（最大 40,000）がすべて同時に対象になっても、1 日（24 回の実行）で消し終える量だからである（`src/workspace.rs` の `CLEANUP_OVERDUE_MILLIS` と const の assert）。
       自動削除は対象を実行ごとにずらして選ぶので、消すたびに失敗するワークスペースがあっても、それを選ばない実行は成功になり、最後の成功だけでは見逃す。
       失敗の履歴ではなく、残っているものを見るので、消すたびに失敗するものは残っている間は知らせ続け、消えれば（直れば）次の確かめから通る。次の実行で消えた一時的な失敗は、知らせない。
       1 日の数を数えるための古い記録（`photo_uploads`・`workspace_creations`）は数えない。1 回の batch で全部を消し、失敗すればその実行は成功にならないので、上の 3 時間の条件で足りる。
       D1 から消した後の 2 回目の R2 の削除の失敗（docs/spec.md の「ワークスペースによるデータの分離」）は、D1 に何も残らないので、ここでは知らせない。残るのは、その間に追加された写真のデータだけで、どの行からもたどれないので片付けない（docs/security.md の「費用の対策」）。
       共有のデモは、印（`mise run pin`）を付ける前提で数える。印が無くても毎日の seed で使われるので、seed が 15 日止まらない限り数えない。
     失敗すると、GitHub Actions の失敗の通知（上の「CI と CD」の、schedule で動く workflow の通知）が届く。seed と別の job なので、確かめが失敗してもデモは入れ直す。
     GitHub の schedule は、混んでいると遅れたり、その日に動かなかったりする。動かなかった日は確かめが無いが、残っているものは次の日の確かめで見つかる（3 時間の条件は、確かめの時刻の近くのことしか見ない）。
   - 公開のログには、成功か失敗かと、最後の成功から経った時間と、残っているものの種類だけを出す（数や ID は出さない。docs/security.md の「ログ」）。
   - マイグレーション（`0007_create_cleanup_runs.sql`）を当てた直後は記録が無いので、デプロイの後の最初の cron（毎時 0 分）より前に確かめが動くと、「成功の記録が無い」で失敗する。そのままにする。次の cron の後に Actions で再実行すれば通る。
   - 失敗したら、Workers のログ（上の「本番のログ」）で「自動削除で失敗があった」の行と、その前のエラーを見る。
     いつから失敗しているかは、手元で `apps/recipe-box-rs` から `pnpm exec wrangler d1 execute DB --remote --command "SELECT ran_at, succeeded, deleted, failed FROM cleanup_runs ORDER BY ran_at DESC"` を実行して見る（7 日分ある。結果を Issue などの公開の場所に貼らない）。
   - GitHub が 60 日の無活動で schedule を止めると、この確かめも止まる（下の「schedule が止まらないようにする」）。cron が動いていないことは、下の Monitors でも知らせる。
2. Cloudflare の Monitors（ダッシュボードの Alerts > Monitors。beta の機能）で、cron の起動・無料の範囲・応答のエラー・リクエストの途絶を知らせる（下の「Monitors の作り方」）。
   設定はダッシュボードにだけあり、リポジトリには残らないので、消えていないかを毎月見る（下の「最低限のメンテナンス」）。

#### Monitors の作り方

Monitors は、ダッシュボードでだけ作れる（beta の間は、API・Terraform・wrangler では作れない）。下の表の 5 個を、1 つずつ手で作る（[Monitors](https://developers.cloudflare.com/notifications/monitors/)）。

準備: アカウント ID を調べる。ダッシュボードを開いたときの URL（`https://dash.cloudflare.com/<アカウント ID>/...`）の、最初の 32 文字の部分である。Workers & Pages の概要の画面の右側の「Account ID」でも見られる。
アカウント ID は、下の SQL の `<アカウント ID>` に貼るときだけ使い、リポジトリ・Issue・PR には書かない。

1 個ずつ、次の順に操作する（画面の欄の名前は 2026-11 時点のもの）。

1. ダッシュボードの左のメニューで Alerts > Monitors を開き、Create Monitor を押す。
2. 検出のしかたで Threshold を選ぶ（Anomaly detection・Service-level objective は使わない）。
3. クエリを Custom SQL にし、下の表の番号の SQL を貼って、`<アカウント ID>` を置き換える。最後に `;` を付けない。
   時間の条件は SQL に書かない。SQL は、Run every の長さの直近の区間で計算される（下の説明）。時刻の列の条件を書くと、「query already contains time predicates on `timestamp`; remove them to use time_range」のように断られる（ほかの名前の時刻の列でも同じ）。
4. 表のとおりに、次の欄を入れる。
   - Run every: Custom を選び、表の値にする（Automatic や既定のままにしない。区間が短くなり、1・5 は知らせ続け、2〜4 は少なく数えて知らせなくなる）。
   - Average over: Custom を選び、Run every と同じ値にする。
   - Trigger when: Query result の比べ方（`<`・`>`・`>=`）と値を、表のとおりに入れる。
   - Repeat notification after: 24h（続いている間は、1 日に 1 通だけ届く）。
5. 保存する前に、プレビューの範囲を Last 24 hours などの長いものにし、グラフの点が Run every ごとに並び、値が表の「ふだんの値」の近くで、条件に当たらないことを確かめる。
6. 表の「名前」を付け、Description に下の表の文を入れ、通知の先にメール（自分のアドレス）を足して、作る。

Monitors（custom analytics policies）は、作れる数に上限がある。この手順は、上限に当たらないよう、知らせるものを次の 5 個にまとめている（2026-11）。

| 番号 | 名前 | 知らせるもの | Run every と Average over | Trigger when | ふだんの値 |
| --- | --- | --- | --- | --- | --- |
| 1 | `recipe-box: cron が動いていない` | 自動削除の cron が、2 時間に 1 回も成功しない | 2h | `<` `1` | 2 |
| 2 | `recipe-box: Worker の失敗` | Worker の失敗（成功以外の status か、`500` 以上の応答。接続を切られたものを除く）が、15 分に 3 回以上 | 15m | `>=` `3` | 0 |
| 3 | `recipe-box: D1 の読み書き` | D1 の読んだ行（1 日 500 万行）か書いた行（1 日 10 万行）の、24 時間の量が、無料の範囲の 80 % を超えた（値は、無料の範囲に対する % の大きいほう） | 24h | `>` `80` | 80 より十分に小さい |
| 4 | `recipe-box: Workers のリクエスト` | アカウントの Workers のリクエストが、24 時間で、1 日の無料の範囲（10 万回）の 80 % を超えた | 24h | `>` `80000` | それより十分に小さい |
| 5 | `recipe-box: リクエストが無い` | cron を除く Worker の起動が、26 時間 0 回 | 26h | `<` `1` | 1 以上 |

Description（知らせのメールに出る説明）は、何が起きたかと、どこを見るかを書く。

| 番号 | Description |
| --- | --- |
| 1 | 自動削除の cron（1 時間ごと）が、直近 2 時間に 1 回も成功していない。Worker（recipe-box）のログと Cron Triggers を見る。docs/development.md の「失敗の通知」 |
| 2 | Worker が 15 分に 3 回以上失敗した。Worker のログと `wrangler tail` を見る |
| 3 | D1 の読み書きが、1 日の無料の範囲の 80 % を超えた。D1 の Metrics と、Worker のログを見る |
| 4 | Workers のリクエストが、1 日の無料の範囲の 80 % を超えた。攻撃や想定外の呼び出しを、Worker の Metrics で見る |
| 5 | 26 時間、cron 以外のリクエストが無い。毎日のデモの入れ直し（seed-demo.yml）が止まっていないか、Worker が動いているかを見る |

- SQL は、Run every ごとに、その長さの直近の区間で計算される（公式の文書の「Automatic」の時間の範囲。2026-11、1 の Monitor のプレビューのグラフで確かめた。Run every 1h では 1 時間ごとに 1、2h では 2 時間ごとに 2 だった）。
  Average over の意味（複数の点の平均か、続いた時間か）は確かめていないので、Run every と同じ値にして、点を 1 つだけ比べるようにしている。
- 区間は重ならないので、区間の終わり近くの記録が分析に遅れて届くと、どの実行にも数えられず、知らせないことがある。cron の失敗は、上の 1 の確かめ（GitHub Actions）でも見つかる。
- 1 の cron は 1 時間ごとなので、2 時間に 1 回も成功しなければ、起動しなかったか、例外で落ちた。1 回だけの抜けは知らせない。
  cron が 1 回だけ落ちて次の回で成功したときは、どこからも知らせない（次の回で片付くので、害が残らない）。落ち続ければ、1 と、上の 1 の確かめ（GitHub Actions）が知らせる。自動削除の一部の失敗（`scheduled` は成功で終わる）は、上の 1 の確かめで知らせる。
- 2 は、例外などの成功以外の status と、アプリが自分で返す `500` などの応答（status では成功に数えられうる）を、合わせて数える。`clientDisconnected`（送り手が接続を切った）は、アプリの失敗ではないので除く。
  リクエストが少ないアプリなので、1 回の失敗で知らせないよう 15 分に 3 回以上とした。割合では見ないので、リクエストがとても多いときは、まれな失敗でも 3 回に届いて知らせうる（回数の制限で断る 429 は数えない）。区間をまたいでゆっくり続く失敗（例: 7 分ごと）は、どの区間も 3 回に届かず知らせない。
- 3 は、読んだ行と書いた行を、それぞれの 1 日の無料の範囲に対する % にし、大きいほうを比べる（SQL は `greatest` を使えないので `CASE` で選ぶ）。
- 3・4 は 1 日に 1 回、直近 24 時間の量を比べるので、知らせが最大 1 日遅れる。無料の範囲は 1 日ごとに数え直されるので、これで足りるとした。4 は、無料の範囲がアカウントの全体で数えられるので、Worker で絞らない。1 日の量は、Cloudflare が UTC の 0 時で数え直すが、ここでは直近の 24 時間で近似する。
- 5 は、cron の起動を除いて数える（cron は 1 で見る）。毎日のデモの入れ直し（`seed-demo.yml`）が API を呼ぶので、ふだんは 0 にならない。seed の時刻のずれを見込んで 26 時間にする。
- 5 個に収めるため、次のものは Monitors では知らせない。
  - D1 と R2 の保存量: アプリの上限で、無料の範囲の内側に抑えている（docs/security.md の「費用の対策」。R2 はアプリ全体で 5 GB まで）。毎月、ダッシュボードの D1 と R2 の画面で見る（下の「最低限のメンテナンス」）。R2 がアプリの上限（5 GB）を超えていれば、どの行からもたどれないデータが残っている。
  - cron が 1 回だけ落ちたこと: 上のとおり。
- 無料の範囲の値は 2026-11 時点のもの（docs/security.md の「費用の対策」）。

```sql
-- 1. recipe-box: cron が動いていない
SELECT countIf(status = 'success') AS value FROM events.workersInvocations WHERE accountTag = '<アカウント ID>' AND scriptName = 'recipe-box' AND invocationType = 'cron'
```

```sql
-- 2. recipe-box: Worker の失敗
SELECT countIf(status <> 'clientDisconnected' AND (status <> 'success' OR httpResponseStatus >= 500)) AS value FROM events.workersInvocations WHERE accountTag = '<アカウント ID>' AND scriptName = 'recipe-box'
```

```sql
-- 3. recipe-box: D1 の読み書き
SELECT CASE WHEN SUM(rowsWritten) * 50 > SUM(rowsRead) THEN SUM(rowsWritten) * 100.0 / 100000 ELSE SUM(rowsRead) * 100.0 / 5000000 END AS value FROM events.d1BackendRequest WHERE accountTag = '<アカウント ID>'
```

```sql
-- 4. recipe-box: Workers のリクエスト
SELECT COUNT(*) AS value FROM events.workersInvocations WHERE accountTag = '<アカウント ID>'
```

```sql
-- 5. recipe-box: リクエストが無い
SELECT COUNT(*) AS value FROM events.workersInvocations WHERE accountTag = '<アカウント ID>' AND scriptName = 'recipe-box' AND invocationType <> 'cron'
```

SQL について:

- どの SQL も、Monitors の決まり（`value` という名前の数を 1 つだけ返す・dataset を 1 つだけ読む・アカウントで絞り込む・時間の条件は書かない・最後に `;` を付けない）に合わせてある。
- 結果の行が無いと Monitors は何も知らせないが、`COUNT`・`SUM`・`countIf` の集計は、合う行が無くても 0 の 1 行を返す（確かめ済み。3 の `CASE` も、どちらの `SUM` も 0 なので 0 になる）。
  そのため、dataset や列の名前が変わって合う行が無くなっても、2〜4 は 0 のままで、知らせも失敗も出ない。毎月の確認で、名前と、プレビューで 0 でない値が出ることを確かめる（1・5 は、行が無くなれば知らせが来る）。
- 5 個の SQL は、今の形（2 の `status <> 'clientDisconnected' AND (...)` と、3 の `CASE` を含む）で、SQL API（`POST /client/v4/analytics/sql`）がどれも `value` の 1 列・1 行を返すことを確かめた（2026-11）。SQL API では画面の欄が無いので、末尾に `AND timestamp >= NOW() - INTERVAL '<Run every の長さ>'`（例: `'2' HOUR`・`'15' MINUTE`）を足した形で確かめた。Monitors に貼る、時刻の条件の無い形は、SQL API では保存してある期間の全体を数えるので、そのままでは確かめていない。SQL API で確かめ直すときも、この条件を足す（足さないと、保存してある期間の全体を数える）。`events.workersInvocations` は 1 行が 1 回の起動で、`status` は文字列（`success`・`clientDisconnected` など）である。
- **確かめていない**こと:
  - Average over の意味（上の説明）。Run every と同じ値にしているので、判定には効かない見込みである。
  - `events.` の dataset は、量に応じて間引かれる（adaptive sampling）と公式の文書にある。間引かれると、3・4 の数を少なく数える。introspection で間引きの重みの列（sample interval など）があれば、`COUNT(*)` をその列の `SUM` に替える。間引かれるのは量の多いときなので、しきい値に近いときほど、知らせが遅れうる。
- beta の間は、dataset と列の名前が変わりうる。毎月の確認（下の「最低限のメンテナンス」）で、名前を問い合わせて確かめる。
  `GET /client/v4/analytics/sql/introspection?account_tag=<アカウント ID>&dataset_name=<dataset の名前>&include_columns=true` を、Account Analytics Read の権限を持つトークンで呼ぶ（結果は公開の場所に貼らない）。

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
- main が先に進んで、ブランチが古い（BEHIND）まま止まった Dependabot の PR は、PR に `@dependabot rebase` とコメントして作り直させる（main は、ブランチが最新であることを必須にしている）。作り直すと、マージを待ってデプロイする job（`dependabot-automerge.yml`）も待ち直す。

### 毎月

- `dependabot-alert-issue` スキルで、Dependabot の alert から Issue を作る。`mise run audit` も実行する。
- Cloudflare の Monitors の設定が残っているか（上の「失敗の通知」の 1〜5 があり、Run every・Average over・Trigger when が表のとおりで、通知の先が今のメールか）を、ダッシュボードの Alerts > Monitors で見る。beta の機能で、設定はリポジトリに残らない。
  SQL が使う dataset と列の名前が変わっていないかも、introspection（上の「Monitors の作り方」）で確かめる。
- D1 と R2 の保存量を、ダッシュボードの D1 と R2 の画面で見る。D1 が無料の範囲（1 つのデータベースで 500 MB）の 50 % を超えているか、R2 がアプリの上限（5 GB。無料の範囲の 10 GB-月の 50 %）を超えていれば、原因を調べる（Monitors の数の上限のため、保存量は知らせない。上の「Monitors の作り方」）。
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
| 解説の「アプリの規模」の表 | commit のたびに数が変わるが、lint では確かめない。大きな機能を足したときや、利用者に求められたときに、`mise run size` と `mise run guide` で数え直す（上の「解説（/about）」） |
| Cloudflare の Monitors（上の「失敗の通知」） | 設定が消えていないかを毎月見る（上の「毎月」）。beta が終わって dataset や列の名前が変われば、SQL を直し、この文書も直す |

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
  新しい側のアプリには、同じ smoke を `API_ONLY=1` を付けて当てる（上の「テスト」）。
- 定期実行（使われていないワークスペースの自動削除）を比べるときは、`mise run dev` の代わりに、`mise run migrate` の後で `apps/recipe-box-rs` で `pnpm exec wrangler dev --test-scheduled` を起動する。
  `curl "http://localhost:8787/__scheduled?cron=0+*+*+*+*"` で 1 回動き、結果を起動した端末のログ（「使われていないワークスペースの自動削除」）に出す。
- 共有のデモのデータを入れるときは、上の「CI と CD」の、共有のデモをローカルで試す手順に従う。
