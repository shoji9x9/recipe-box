# このアプリの作り

recipe-box は、料理のレシピを登録・検索できる Web アプリです。ログインは無く、推測できない ID の「ワークスペース」でデータを分けます。
このページは、アプリの作りを次の 2 者に向けて説明します。

- リライト前の技術スタック（Rust・Cloudflare Workers など）を初めて使う人。全体の構成と、それぞれの技術を選んだ理由・使い方。
- リライトをする人。言語やフレームワークのほかに、このアプリならではの仕組み（定期実行・写真の保管・レート制限など）の代わりに何を選ばなければならないかと、選ぶときに守る仕様。

仕様の原本は [docs/spec.md](repo:docs/spec.md)、上限などの費用の対策は [docs/security.md](repo:docs/security.md)、方針は [docs/architecture.md](repo:docs/architecture.md) です。
このページは、それらへの案内と、コードの読み方の説明です。値が食い違うときは、原本が正しいものとします。

このページは、元の文章（[guide/about.md](repo:guide/about.md)）から `mise run guide` で作ります。引用するコードは、作るたびにリポジトリのファイルから取り出すので、コードと食い違いません。

## 全体の構成

<!-- diagram: architecture | 全体の構成。ブラウザと API の利用者が Cloudflare の Worker と静的アセットにアクセスし、Worker が D1・R2・Rate Limiting を使う。Cron Trigger が 1 時間ごとに Worker を呼ぶ。GitHub Actions がデプロイとデモの入れ直しをする -->

図の見方: 枠は、まとまり（使う人・Cloudflare・GitHub Actions）で、名前は枠の左上の外に書きます。円柱はデータを置くところ（D1・R2）です。四角は、灰色が使う人と GitHub Actions のジョブ、橙が Cloudflare の機能、赤（画面のボタンと同じ色）がこのアプリのコード（Worker）です。

- 画面（HTML）と API（JSON）を、同じ 1 つの Worker が返します。画面は API を呼ばず、Worker の中で同じ処理を使います。
- CSS・JavaScript・このページなどのファイルは、Workers の静的アセットとして配ります。Worker は動きません。
- データは D1（SQLite）に、写真のデータは R2（オブジェクトストレージ）に置きます。
- 使われていないワークスペースは、Cron Trigger で 1 時間ごとに消します。
- デプロイと、共有のデモの入れ直しは、GitHub Actions が行います。

## Cloudflare の各サービス

使っているサービスは、すべて [apps/recipe-box-rs/wrangler.toml](repo:apps/recipe-box-rs/wrangler.toml) に書いています。binding（`DB` など）は、コードからサービスを使うときの名前です。

<!-- code: apps/recipe-box-rs/wrangler.toml#bindings -->

### Workers

Cloudflare の拠点で、リクエストごとにコードを動かす実行環境です。サーバーを用意せず、リクエストが来たときだけ動きます。
このアプリは Rust を WebAssembly（Wasm）にして動かします（[workers-rs](#workers-rs)）。

制約で作りが変わるのは、次の 2 つです（値は [docs/security.md の「費用の対策」](repo:docs/security.md#費用の対策)）。

- 無料のプランでは、1 回のリクエストの CPU 時間が 10 ms に決まっています。写真のように大きな本文を Wasm に読みためると、これを大きく超えます（[写真の保管](#写真の保管)）。
- 無料のプランでは、1 日のリクエストの数に上限があり、超えると止まります。課金はされません。

### 静的アセット

`apps/recipe-box-rs/public/` の下のファイルを、Worker を起動せずに配ります。パスがファイルに当たらないリクエストだけを、Worker が受け取ります。
Worker のリクエストの数に入らないので、費用がかかりません。このページ（`/about`）も静的アセットです。

画面の応答のヘッダー（CSP など）は Worker が付けますが、静的アセットには Worker が動かないので、[public/_headers](repo:apps/recipe-box-rs/public/_headers) で付けます。

### D1

SQLite を元にしたデータベースです。ワークスペース・レシピ・写真の行（大きさなど）を置きます。
SQL は標準の範囲で書き、リライトで別の DB（PostgreSQL など）へ移しても差が出にくいようにしています（[AGENTS.md の「SQL」](repo:AGENTS.md)）。
標準から外れるのは、ページ分けの `LIMIT ? OFFSET ?` と、料理名の部分一致の `instr` です。

D1 は、複数の文を `batch` で 1 つのトランザクションとして実行できます。上限を数えることと保存することを 1 つにまとめ、同時に作られても上限を超えないようにしています（[費用の対策](#費用の対策)）。
テーブルを作る SQL は、番号を付けたマイグレーションのファイル（[apps/recipe-box-rs/migrations/](repo:apps/recipe-box-rs/migrations)）です。デプロイでは、コードを入れ替える前にマイグレーションを当てるので、マイグレーションは表や列を足すだけにします。

### R2

写真のデータを置くオブジェクトストレージです。キーと、写真のバイト列を置きます。
無料の範囲を超えると課金され、利用を止める上限の設定がありません。そのため、R2 に書き込む前に、D1 で上限を判定します（[写真の保管](#写真の保管)）。

### Rate Limiting

キー（このアプリでは IP アドレス）ごとに、決まった時間の中の回数を数える機能です。Worker の binding として使います。
Cloudflare の拠点ごとに数える、おおよその制限です。厳密な上限は、D1 で数えるアプリ全体の上限が担います（[費用の対策](#費用の対策)）。

### Cron Triggers

決まった時刻に Worker を呼ぶ機能です。1 時間ごと（毎時 0 分）に、14 日アクセスが無いワークスペースを消します。1 回の実行で呼べる D1 と R2 の数に上限があるので、1 回に消す数を絞り、残りは次の実行に回します（[ワークスペースによるデータの分離](#ワークスペースによるデータの分離)）。
Worker は、リクエストの入口（`fetch`）とは別の入口（`scheduled`）で呼ばれます。

<!-- code: apps/recipe-box-rs/src/lib.rs#entry-scheduled -->

### ローカルの開発と本番へのデプロイ

`wrangler` は、Workers の開発とデプロイの道具です。`wrangler dev`（`mise run dev`）は、Workers の実行環境（workerd）と、D1・R2・Rate Limiting をまねたもの（Miniflare）を手元で動かします。
Cloudflare のアカウントは要りません。データは `apps/recipe-box-rs/.wrangler/state` に置かれます。
手順は [docs/development.md](repo:docs/development.md) にあります。

main に入った変更は、GitHub Actions の `deploy.yml` が本番にデプロイします。D1 と R2 のバケットが無ければ作り、D1 にマイグレーションを当ててから、Worker を入れ替え、本番が応答するかを GET のリクエストで確かめます。
失敗したときの戻し方は、[docs/development.md の「デプロイが失敗したとき」](repo:docs/development.md#デプロイが失敗したとき)にあります。
ローカルと本番で動きが違うもの（Cron Trigger の起動のしかた、回数の制限の数え方など）は、[docs/architecture.md の「リライトの比較の基準」](repo:docs/architecture.md#リライトの比較の基準)に表があります。

### 運用（ログ・wrangler tail・Metrics）

- Workers のログ（`[observability]`）: アプリが `console` に出したもの（エラーなど）を、後からダッシュボードで見られます。
  リクエストごとの記録（invocation logs）は残しません。API の `X-Workspace-Id`（ワークスペースの ID）が残るかもしれないためです。
- `wrangler tail`: 本番のリクエストとログを、その場で流して見ます。残りません。リクエストごとの CPU 時間は、ここで見ます。
- ダッシュボードの Metrics: リクエストの数・エラーの割合・CPU 時間の傾向を見ます。
- 失敗の通知: 定期の自動削除が最近に成功し、この 1 日に失敗した実行が無いかを、`seed-demo.yml` の job が D1 の記録（`cleanup_runs`）で毎日確かめ、そうでなければ job が失敗して GitHub がメールで知らせます。
  cron が動いていないこと・無料の範囲に近づいたこと・エラーの割合などは、Cloudflare のダッシュボードの Monitors（ダッシュボードで設定し、リポジトリには残りません）で知らせます。手順と SQL は [docs/development.md の「失敗の通知」](repo:docs/development.md#失敗の通知)にあります。

ワークスペースの ID は、知ればそのワークスペースを読み書きできる鍵なので、ログに残しません。アプリがエラーを出すときは、ID の形の文字の並びを伏せてから出します。
理由は [docs/security.md の「ログ」](repo:docs/security.md#ログ)に、見方は [docs/development.md の「本番のログ」](repo:docs/development.md#本番のログ)にあります。

### 無料枠などの制約

サービスごとの無料の範囲と、使いすぎたときの動き（止まるか、課金されるか）は、[docs/security.md の「費用の対策」](repo:docs/security.md#費用の対策)の表にあります（2026-11 時点の公式の文書による）。
無料の範囲は変わることがあるので、使う前に公式の文書で確かめてください。

- [Workers の制限](https://developers.cloudflare.com/workers/platform/limits/)
- [D1 の制限](https://developers.cloudflare.com/d1/platform/limits/)
- [R2 の料金](https://developers.cloudflare.com/r2/pricing/)

## フロントエンドとバックエンドの技術スタック

| 用途 | 使うもの | 似た TypeScript の技術 |
| --- | --- | --- |
| 言語 | Rust（Wasm にして動かす） | TypeScript |
| Workers との橋渡し | workers-rs | Workers の JavaScript の API をそのまま使う |
| Web フレームワーク | axum | Hono・Express |
| 入力の確かめ | serde と、自分で書いた確かめの関数 | Zod |
| DB の読み書き | SQL を直接書く（D1 の API） | Drizzle・Kysely |
| HTML | askama（テンプレート） | React（JSX）・Hono の JSX |
| 画面の部分の入れ替え | htmx | React・Alpine.js |

全体の一覧は [docs/architecture.md の「技術構成（リライト前）」](repo:docs/architecture.md#技術構成リライト前)にあります。
リライト後の技術スタックは決めていません。リライトの題材として差が出るよう、リライト前は、Web アプリでよく使われる TypeScript の構成とは異なる技術を選んでいます。表の右の列は、読む人が知っていそうな技術との対応で、リライト後の候補を示すものではありません。

### Rust

型と所有権で、メモリの安全とデータの競合の無さをコンパイルのときに確かめる言語です。
Workers では Wasm にして動かします。JavaScript と違い、`null` の代わりに `Option`、例外の代わりに `Result` を使い、失敗しうる処理をすべて型で表します。

TypeScript と比べた違い:

- 型を実行のときに消さず、コンパイルのときにすべて確かめます。`any` に当たる抜け道がありません。
- ガベージコレクションが無く、値の持ち主（所有権）がスコープを出たときに片付けます。
- 非同期の処理（`async`）は同じ考え方ですが、`Future` を動かす仕組み（ランタイム）を選ぶ必要があります。Workers では、JavaScript の Promise の上で動かします。

### workers-rs

Rust から Workers の機能（リクエスト・D1・R2・Rate Limiting など）を使うためのライブラリです。
`#[event(fetch)]` を付けた関数が、リクエストの入口になります。このアプリは、受け取ったリクエストを axum の Router に渡します。

<!-- code: apps/recipe-box-rs/src/lib.rs#entry-fetch -->

ビルドは `worker-build` が行います。Rust を Wasm にし、wasm-bindgen で JavaScript との橋渡しのコードを作り、wasm-opt で小さくします。

#### JavaScript と Rust（Wasm）の分担

Rust で書いていても、Worker の中では JavaScript と Rust（Wasm）が一緒に動きます。Workers のランタイム（V8）が動かすのは JavaScript で、Wasm はその JavaScript から読み込まれるためです。
Hono で書いた Worker なら全部が JavaScript ですが、このアプリでは次のように分かれます。

1. **Workers のランタイム（V8）**: 実行の土台です。リクエストを受け取り、`wrangler.toml` の `main`（`build/index.js`）と、そこから読み込まれる Wasm を、この上で動かします。D1・R2 などの binding と、リクエストの本文のストリームもランタイムが持ちます。
2. **`build/index.js`**: `worker-build` が作る JavaScript です。wasm-bindgen が作る橋渡し（glue）のコードと、手で書いた [src/photo-stream.js](repo:apps/recipe-box-rs/src/photo-stream.js) を 1 つにまとめ、Wasm（`index_bg.wasm`）を読み込みます。
   リクエストが来ると、ランタイムがこのファイルの fetch のハンドラーを呼び、ハンドラーが Rust の `#[event(fetch)]` の関数を呼びます。
   定期実行（Cron Trigger）のときも同じ道で、ランタイムが scheduled のハンドラーを呼び、ハンドラーが Rust の `#[event(scheduled)]` の関数を呼びます（図では省いています）。
3. **Rust を Wasm にしたもの**: axum の Router・ハンドラー・上限の判定・写真のメタデータの書き換え（[src/metadata.rs](repo:apps/recipe-box-rs/src/metadata.rs)）など、決めることはすべてここで行います。
   画面の HTML のテンプレートも、ここに入っています。askama は、テンプレートをコンパイルのときに Rust のコードにするので（[askama](#askama)）、テンプレートのファイルは Worker と一緒には配らず、Wasm の一部になります。

<!-- diagram: worker-layers | Worker の中の層と呼ぶ向き。ブラウザのリクエストを Workers のランタイム（V8）が受け、build/index.js の fetch のハンドラーを呼び、ハンドラーが Rust を Wasm にしたもの（#[event(fetch)] と axum・askama のテンプレート・metadata.rs・workers-rs）を呼ぶ。Rust は workers-rs と wasm-bindgen を通して build/index.js の橋渡しと photo-stream.js を呼び、それがランタイムの D1・R2 の binding を呼び、ランタイムが D1・R2 と通信する。静的アセットは、Worker も V8 も動かさずに Cloudflare が返す。応答と結果は、同じ線を逆にたどって戻る -->

図の矢印は、呼ぶ向きです。応答と、呼んだ結果は、同じ線を逆にたどって戻ります。ランタイムから JavaScript、JavaScript から Rust を呼ぶのは、リクエスト（fetch）と定期実行（scheduled）のときです。Rust から JavaScript を呼ぶのは、D1・R2・Rate Limiting・乱数・時計・ログ・photo-stream.js などを使うときです。

- **D1・R2 を使うとき**: Rust が D1・R2 と直接通信するのではありません。D1・R2 の binding は、ランタイムが Worker に渡す JavaScript の値（オブジェクト）です。
  Rust は workers-rs を通して、wasm-bindgen の橋渡しでその JavaScript の値の関数を呼び、実際の通信はランタイムが行います。
- **静的アセット**: CSS・画面の JavaScript・解説（`public/`）は、Cloudflare がファイルとして返し、Worker（`build/index.js` と Wasm）も V8 も動きません（[静的アセット](#静的アセット)）。図では、Workers の枠の中に描いています。

JavaScript の値（Request・Response、D1・R2 の binding、ReadableStream）は、Wasm のメモリーに写さず、JavaScript の側に置いたまま、参照（Rust の `JsValue` と、js-sys・web-sys の型）として Rust に渡します。
中身（バイト列や文字列）を Rust で読むときだけ、Wasm のメモリーに写します。

Rust から JavaScript を呼ぶ道は 3 つあります。

- **workers-rs**: D1・R2・Rate Limiting などを、Rust の型と関数で包んだものです。Hono で `c.env.DB` を使うところを、Rust では workers-rs の型で使います（[src/cloudflare.rs](repo:apps/recipe-box-rs/src/cloudflare.rs)）。
- **wasm-bindgen で JavaScript の関数を宣言する**: workers-rs に無いものは、`extern "C"` の中に JavaScript の関数の形を書いて直接呼びます。乱数は Web Crypto の `crypto.getRandomValues()` を呼びます。

<!-- code: apps/recipe-box-rs/src/cloudflare.rs#get-random-values -->

- **手で書いた JavaScript のファイルを読み込む**: `#[wasm_bindgen(module = ...)]` で、同じクレートの JavaScript のファイルの関数を、Rust の関数として呼びます。`worker-build` が、そのファイルを `build/index.js` にまとめます。

<!-- code: apps/recipe-box-rs/src/stream.rs#photo-stream-extern -->

写真の本文を JavaScript と Rust でどう分けて扱うか（先頭を含む最初の部分だけを Rust に渡し、残りは JavaScript の側で流す）は、[本文を Wasm に写すときの CPU 時間](#本文を-wasm-に写すときの-cpu-時間)にあります。

##### 境目を越えるときの費用と、テスト

- JavaScript と Rust の境目を越えるたびに、wasm-bindgen の橋渡しのコードが動きます。値を参照で渡すだけなら軽いのですが、バイト列や文字列を写すと、大きさに比例して CPU 時間がかかります。
  そのため、写真では先頭を含む最初の部分だけを、64 KiB ずつ（足りなければその大きさで）写します（先頭は 1 MiB までなので、読み足しは 16 回まで）。読みすぎた分は Wasm に写さず、JavaScript の側に残して、そのまま R2 に流します。
- Rust のテスト（`cargo test`）は手元の OS で動き、JavaScript を動かせません。そのため、Rust の側は `Backend` トレイトの偽物で確かめ（[テストの仕組み](#テストの仕組み)）、本文を本文のまま渡す経路を通ります。
  手で書いた JavaScript は、vitest で、DOM の無い Node の環境で確かめます（[apps/recipe-box-rs/js-tests/worker/](repo:apps/recipe-box-rs/js-tests/worker)。`FixedLengthStream` は代わりのものを置きます）。
  両方をつないだ経路は、ローカルの Worker に当てるスモークテスト（`mise run smoke`）で確かめます。

### axum

Rust の Web フレームワークです。パスとメソッドの組（Router）に、ハンドラーの関数を結び付けます。
Hono と同じく、ミドルウェア（middleware）で前後の処理を足せます。このアプリは、書き込む API の前に、回数と本文の大きさを確かめる middleware を付けています。

<!-- code: apps/recipe-box-rs/src/lib.rs#recipe-routes -->

axum らしい点は、extractor です。ハンドラーの引数の型を書くと、axum がリクエストからその値を取り出してから、ハンドラーを呼びます。
取り出せなければ、ハンドラーを呼ばずに失敗の応答を返します。このアプリは、ワークスペースの ID を確かめる extractor（`Workspace`）を作り、ID の要る API の引数にしています。

<!-- code: apps/recipe-box-rs/src/workspace.rs#workspace-extractor -->

Hono なら、`c.req.header()` で値を取り、ハンドラーの中で確かめます。axum では、引数に `Workspace` を書き忘れると ID を使えないので、確かめ忘れが起きにくくなります。
入力の JSON は serde で型に読み込み、長さなどは自分で書いた関数で確かめます。Zod のように、1 つの定義から型と確かめの両方を作るものではありません。

### askama

HTML のテンプレートを、コンパイルのときに Rust のコードにするライブラリです。テンプレートが参照する値の名前と型を、コンパイルのときに確かめます。
値は既定で HTML のエスケープを行います。テンプレートは [apps/recipe-box-rs/templates/](repo:apps/recipe-box-rs/templates) にあります。

React は、ブラウザ（か、サーバーの JavaScript）で部品を組み立てます。askama は、サーバーで HTML の文字列を作るだけで、ブラウザには HTML だけが届きます。

### htmx

HTML の属性で、「この操作でサーバーに HTML を取りに行き、ページのこの部分を入れ替える」を書くライブラリです。
一覧の検索では、入力しながら一覧の部分だけを入れ替えます。サーバーは、ふつうの一覧の画面の HTML を返し、htmx がその中の `#results` だけを取り出して入れ替えます。

<!-- code: apps/recipe-box-rs/templates/list.html#htmx-search -->

React で同じことをすると、API から JSON を取り、ブラウザで HTML を組み立てます。htmx では、HTML を作るのはいつもサーバーで、画面の状態をブラウザに持ちません。
CSP でインラインのスクリプトと `eval` を禁じているので、htmx もそれらを使わない設定にしています（[apps/recipe-box-rs/templates/base.html](repo:apps/recipe-box-rs/templates/base.html) の `htmx-config`）。

### 使って分かった難しい点

#### `Send` の扱い

axum は、ハンドラーの `Future` に `Send`（別のスレッドに渡せること）を求めます。ところが、Workers の値（D1 のデータベースなど）は JavaScript の値なので `Send` ではありません。
Workers は 1 つのスレッドで動くので、実際には別のスレッドに渡ることはありません。そこで、workers-rs の `SendWrapper`・`SendFuture` で包んで、`Send` として扱います。

<!-- code: apps/recipe-box-rs/src/cloudflare.rs#cloudflare-backend -->

#### Wasm の上で動かす制約

- 普通の `cargo test` は Wasm でなく手元の OS で動くので、D1 などの Workers の機能を使えません。そのため、外の機能を `Backend` トレイトに分け、テストでは偽物に差し替えます（[テストの仕組み](#テストの仕組み)）。
- 乱数は、Workers の Web Crypto（`crypto.getRandomValues()`）を wasm-bindgen で呼びます。
- リリースのビルドは、`strip` を使いません。wasm-bindgen が Wasm の機能を調べる節まで消えるためです（ルートの [Cargo.toml](repo:Cargo.toml)）。

#### 本文を Wasm に写すときの CPU 時間

写真を送るリクエストの本文を、いったん Wasm のメモリーに読みためると、4 MB の写真で CPU 時間が数十 ms かかりました。無料のプランの上限（1 回 10 ms）を大きく超えます。
本文の全体を JavaScript の関数に流して書き換えても、1 回の追加で CPU 時間が 150〜200 ms 増えました。
そこで、形式を決めてメタデータ（撮った場所など）を取り除く判断は Rust に置き、JavaScript には、写真の最初の部分を読んで Rust に渡すことと、書き換えた最初の部分の後に残りを流すことだけをさせます。
残りは、ランタイムの組み込みの `pipeTo` で R2 へ流すので、JavaScript の関数も Wasm も通りません。
Rust は、最初の部分を長さを変えずに書き換えます（向きだけを残し、ほかのメタデータは 0 で埋める）。長さが変わらないので、R2 には `Content-Length` のまま流せます（[apps/recipe-box-rs/src/stream.rs](repo:apps/recipe-box-rs/src/stream.rs)）。

ここで「先頭」は、写真のファイルの初めから、画像のデータ（画素）の直前までです（docs/spec.md とコードでは「頭の部分」と呼びます）。ファイルの形式を示す目印のほか、撮った場所や日時を持つ EXIF、XMP、色の情報（ICC プロファイル）、PNG の文字のチャンクなどが入ります。ふつうの写真では数十 KB で、取り除くメタデータはここにあります。
Rust に渡す「最初の部分」は、先頭を含むように 64 KiB ずつ読んだもので、先頭の後ろの画像のデータも少し入ります（64 KiB より小さい写真では全体です）。それでも、Rust に写すのは本文のうち最初の 64 KiB ほどだけです。

<!-- diagram: photo-stream | 写真のバイト列を追加するときに、JavaScript と Rust が何を渡し合うか。Rust が photo-stream.js に、先頭を含む本文の最初の部分だけを読むよう指示し、photo-stream.js は最初の部分だけを返す。Rust の metadata.rs が、先頭の位置情報などを 0 で埋める（要点）。Rust が、書き換えた最初の部分だけを渡して R2 へ流すストリームを作るよう指示し、そのストリームを R2 の put に渡す。R2 が読み始めると、書き換えた最初の部分と、JavaScript の側に残した読みすぎた分が書かれ、それを書き終えると、joinPhoto が予約しておいた pipeTo が自動で始まり、本文の残りが JavaScript の関数も Wasm も通らずに R2 へ流れる（要点）。最初の部分のほかは、Rust を通らない。最後に R2 が、書き込みが終わったことを Rust に知らせる（完了通知） -->

図の線の意味: 実線は呼び出し（指示）と、R2 に書かれるデータの流れです。破線は、呼んだ側に返る結果です。赤の太い線は、この図の要点です。

- photo-stream.js は、本文を少しずつ読み（`reader.read()`）、64 KiB たまってから（先頭を調べるのにもっと要るときは、その大きさまで）、まとめて Rust に返します。Rust はそれを待ち、先頭がまだ終わっていなければ、もう一度読むよう指示します。最後に読んだ塊のうち、渡す大きさを超えた分（読みすぎた分）は、Rust に渡さず JavaScript の側に残します。
- `joinPhoto` は、「書き換えた最初の部分を書く → 読みすぎた分を書く → 本文の残りを `pipeTo` で流す」という順番を予約しておき、R2 に渡すストリームをすぐに返します（この時点では、まだ何も流れません）。
  R2 の `put` がそのストリームを読み始めると、まず書き換えた最初の部分と読みすぎた分が書かれ、それを書き終えると、予約しておいた `pipeTo` が自動で始まります。`put` を呼んだ Rust は、ほかに何もせず、R2 から完了通知（書き込みが終わったこと）が返るのを待つだけです。

この図は、誰が読み、誰が決め、誰が流すかだけを描いています（D1 に行を入れるところは省いています）。D1 と R2 に書く順序と上限の判定は、[写真の保管](#写真の保管)の図にあります。

<!-- code: apps/recipe-box-rs/src/photo-stream.js#join-photo -->

測った値と経緯は [Issue #31](https://github.com/shoji9x9/recipe-box/issues/31) と [Issue #52](https://github.com/shoji9x9/recipe-box/issues/52) に、決まりは [docs/spec.md の「写真」](repo:docs/spec.md#写真)にあります。

### リクエストの流れ

<!-- diagram: request-flow | リクエストの流れ。静的アセットに当たればファイルを返す。当たらなければ Worker の入口に進む。ルートが無ければ 404 か 405、書き込みの回数・本文の大きさ・送り元が許す範囲になければ 429・413・403 で断る。通れば、ワークスペースを使うルートなら使うワークスペースを決め（API で ID が誤っていれば 400 か 404 で断る。画面で自分のワークスペースも共有のデモも無ければ「はじめる」画面へ移す）、ハンドラーが入力を確かめ（通らなければ断る）、D1・R2 を使う。上限などで失敗すれば断り、うまくいけば応答を返す -->

図の見方: 楕円は始まりと終わりです。紫のひし形は「はい」と「いいえ」で先が分かれる問いで、断る応答は必ずひし形から出ます。赤の四角はこのアプリのコードの処理です。処理の中で決まる失敗（D1 に書くときに数える上限など）は、処理の後の「うまくいったか？」から断ります。赤の破線は断る応答を返す流れで、色の見分けにくい人や白黒の印刷でも通常の流れと区別できるよう、色に加えて破線にしています。

- 静的アセットに当たるパスは、Worker を通りません。
- 画面の応答には、middleware が CSP などのヘッダーを付けます（図では省いています）。
- 書き込む API と画面の書き込みには、middleware が IP アドレスごとの回数と本文の大きさを確かめます。
  画面のうち、レシピの登録・更新と、ワークスペースの作成・切り替えは、回数の制限に当たっても middleware が断らず、印を付けてハンドラーに渡します。ハンドラーは、何も書き込まずに、書きかけの入力を保ったフォームや「はじめる」画面に理由を出します（図では省いています）。
- 画面の無いパスは、一覧に戻れる 404 の画面にします。POST だけのパスに GET が届いたときは、405 で断らずに、関係する画面へ移します（図では省いています）。
- ワークスペースは、API では extractor が `X-Workspace-Id` で確かめ、画面では middleware が Cookie の ID を使い、無ければ共有のデモを見せます（Cookie の ID のワークスペースが消えていれば、Cookie を消し、そのことを画面で一度だけ知らせます。そのときの書き込みは、共有のデモに書かずに断ります）。ID の要らない API（ワークスペースの作成など）は、図の「ワークスペースを使うルートか？」で「いいえ」に進みます。どちらも、ワークスペースを使うと最終利用日時を更新します（自動削除が見る日時です。前の更新から 1 時間以上たっているときだけ書き換えます）。
- ハンドラーは、`Backend` トレイトを通して D1・R2 を使います。

### JavaScript が動かなくても使える作り

画面は、JavaScript が動かなくても、すべての操作ができます（[docs/spec.md の「画面」](repo:docs/spec.md#画面)）。JavaScript は、使い心地を良くするためだけに使います。

- 画面の移動とフォームは、ふつうのリンクとフォーム（`<a>`・`<form method="post">`）です。`<body hx-boost="true">` で、htmx が動けば、ページの全体を読み直さずに中身を入れ替えます。htmx が動かなければ、ふつうに移動します。
- 書き込みは、POST の後に別の画面へ移す（リダイレクトする）ので、再読み込みで二重に送りません。
- 削除の確認のダイアログは、Invoker Commands（`commandfor`・`command` 属性）で開閉します。ボタンの属性だけで `<dialog>` を開けるので、スクリプトが要りません。
  Invoker Commands に対応していない少し古いブラウザでは、小さなスクリプト（`page.js`）が同じ属性を読んで、同じダイアログを開きます。

<!-- code: apps/recipe-box-rs/templates/delete_dialog.html#invoker-dialog -->

自分で書いた JavaScript は、写真をまとめて 1 枚ずつ送り、その後に一覧と詳細に出す小さな版を作って送るもの（`photo-upload.js`）と、日本語の入力の変換中に検索しないもの（`search.js`）と、画面が移った後のフォーカスと古いブラウザでのダイアログを補うもの（`page.js`）だけです。写真と検索のものは、動かなければふつうのフォームとして働きます。

### テストの仕組み

アプリが外の世界（DB・オブジェクトストレージ・レート制限・乱数・時計）に頼る部分は、`Backend` トレイトにまとめています。

次は、トレイトの先頭（乱数・時計・回数の制限）だけです。ワークスペース・レシピ・写真を読み書きするメソッドが続きます（全体は [apps/recipe-box-rs/src/backend.rs](repo:apps/recipe-box-rs/src/backend.rs)）。

<!-- code: apps/recipe-box-rs/src/backend.rs#backend-trait -->

本番では Cloudflare の機能を使う実装（`CloudflareBackend`）を、テストではメモリの中で動く偽物（`FakeBackend`）を使います。
テストは、偽物を入れた Router にリクエストを送り、応答と、偽物に残ったデータを確かめます。

<!-- code: apps/recipe-box-rs/src/tests/workspaces.rs#fake-backend-test -->

| テスト | 確かめること | 動かし方 |
| --- | --- | --- |
| Rust のテスト（`apps/recipe-box-rs/src/tests/` など） | API と画面の応答、上限、データの分離 | `mise run test`（`cargo test`） |
| JavaScript のテスト（vitest） | 画面の JavaScript と、Node のスクリプト（seed など） | `mise run test` |
| スモークテスト（[scripts/smoke-test.ts](repo:scripts/smoke-test.ts)） | ローカルの Worker に API を呼び、本物に近い D1・R2・Rate Limiting を通して確かめる | `mise run smoke`（CI でも動く） |

スモークテストの確かめのうち、API だけを使うものは、リライトの後の新しいアプリにも、そのまま当てられます。画面と静的アセットの確かめは今の作りに頼るので、新しいアプリには `API_ONLY=1` を付けて外します（[docs/development.md の「テスト」](repo:docs/development.md#テスト)）。

## 特徴的な機能

### ワークスペースによるデータの分離

ログインの代わりに、推測できない ID でデータを分けます（[docs/spec.md の「ワークスペースによるデータの分離」](repo:docs/spec.md#ワークスペースによるデータの分離)）。

- **ID の作り方**: 128 ビットの乱数（Web Crypto）を、16 進数の小文字 32 文字で表します。
- **受け取り方**: API は `X-Workspace-Id` ヘッダーで、画面は Cookie（`__Host-` で始まる名前、HttpOnly・Secure・SameSite=Lax）で受け取ります。
- **データの分け方**: すべてのクエリを `workspace_id` で絞り込みます。ID を知っている人だけが、そのワークスペースを読み書きできます。
- **ワークスペースの削除**: 画面と API から、レシピと写真ごと消せます。共有のデモは消させません。
- **自動削除**: 最後に使われてから 14 日たったワークスペースを、Cron Trigger で 1 時間ごとに消します。対象を選んでから 1 つずつ消すので、写真に印を付けて隠すときと D1 で消すときに、最終利用日時を確かめ直して、使われていれば消しません。同じ定期実行で、書き込みの途中で失敗して残った写真の行も片付けます（[写真の保管](#写真の保管)）。最後に、結果（成功したか・消した数・失敗した数）を D1 の `cleanup_runs` に残し、毎日の GitHub Actions がそれを読んで、3 時間成功していないか、この 1 日に失敗した実行があれば、失敗して知らせます。

最終利用日時は、ワークスペースを使うたびに書き換えると、読むだけの API でも DB に書き込むことになります。そのため、前の書き換えから 1 時間以上たっているときだけ書き換えます。

### 費用の対策

ログインの無い公開アプリなので、誰でも API を呼べます。上限は 3 つの層で設けます（値は [docs/security.md の「費用の対策」](repo:docs/security.md#費用の対策)の表）。

| 層 | 仕組み | 回避の手段 |
| --- | --- | --- |
| IP アドレスごと | Rate Limiting。おおよその回数 | IP アドレスを変える |
| ワークスペースごと | D1 で数える。レシピの数・本文の大きさ・写真の枚数と合計 | ワークスペースを作り直す |
| アプリ全体 | D1 で数えて厳密に判定する。ワークスペースの数・1 日に作る数・1 日の追加の回数。写真の合計は、ワークスペースの数とワークスペースごとの合計の上限で守る | 回避できない |

IP アドレスごとの回数は、Cloudflare が入れる `cf-connecting-ip` ヘッダーで数えます。IPv6 は、1 人が使える範囲（上位 64 ビット）でまとめて数えます。

<!-- code: apps/recipe-box-rs/src/limits.rs#client-key -->

アプリ全体の上限は、数えることと保存することを 1 つのトランザクションにして、同時に作られても超えないようにします。
1 日に作った数は、ワークスペースを消しても残る記録（`workspace_creations`）で数え、作ると消すを繰り返して超えさせません。

<!-- code: apps/recipe-box-rs/src/cloudflare.rs#insert-workspace -->

### 写真の保管

写真は R2 に置き、行（大きさ・形式など）を D1 に置きます。上限は [docs/spec.md の「写真」](repo:docs/spec.md#写真)にあります。
R2 は無料の範囲を超えると課金されるので、R2 に書く前に D1 で上限を判定します。書く順序は次のとおりです。

<!-- diagram: photo-add | 写真を追加するときの順序。Worker が先頭で形式を決めてメタデータを取り除き、D1 で上限を数えて、書き込みの終わっていない印を付けた行を入れてから、本文をストリームのまま R2 に書く。書けなければ D1 の行を消す。書けたら行の印を外し、行が無くなっていて外せなければ R2 のデータを消す -->

- 印の付いた写真は、画面と API に見せません。上限には数えるので、同時に追加されても上限を超えません。
- 消すときは、行に印を付けて隠してから、R2 のデータを先に消し、D1 の行を消します。逆にすると、R2 の削除が失敗したときに、どの行からもたどれないデータが R2 に残るためです。R2 の削除が失敗したら、印を外して元に戻します。
- レシピやワークスペースを消すときも、先に写真の行に印を付けて隠してから、R2、D1 の順に消します。途中で失敗しても、R2 のデータの無い写真は見えず、定期の自動削除が片付けます。
- 追加と削除が同時に起きても、どの行からもたどれない（合計に数えられない）データを R2 に残さないよう、追加の側は R2 に書いた後で行の印を外し、行が無くなっていれば R2 のデータを消します。
- D1 に行を入れてから R2 への書き込みと後始末までは、送り手との接続が切れても最後まで行います（Workers の `waitUntil`）。
- それでも途中で失敗するか Worker が止められて、印が付いたまま残った行は、定期の自動削除が R2 のデータと一緒に片付けます。印には付けた日時を持たせ、1 時間を過ぎたものだけを片付けて、追加や削除の途中の行には触れません。

一覧と詳細で原寸の写真（1 枚 4 MB まで）を送らないよう、写真の小さな版（長い辺を 1280 ピクセルまでに縮めた JPEG）も保存します。
長い辺が 1280 ピクセル以下で、ファイルも小さい写真には作りません（作っても小さくならないため）。
小さな版は、Worker ではなくブラウザ（`photo-upload.js`）が canvas で作ります。Worker で画像を展開して縮めると、CPU 時間が 1 回 10 ms を大きく超えるためです。
写真を送り終えてから、別のリクエストで送ります。決まりは [docs/spec.md の「小さな版」](repo:docs/spec.md#小さな版)にあります。

<!-- diagram: photo-thumbnail | 写真の小さな版を足すときの順序。ブラウザが写真を送って写真の ID を受け取り、canvas で長い辺を 1280 ピクセルまでに縮めた JPEG を作って送る（小さな写真には作らない）。Worker は形を確かめてメタデータを 0 で埋め、D1 で合計の上限を数えて枠を取ってから R2 に書き、幅を入れて画面に出す。書けなければ枠を返す。写真が消えていれば、R2 のデータを消して枠を返す -->

- 一覧と詳細は、`srcset` と `sizes` で、表示に足りる方（ふつうは小さな版）をブラウザに選ばせます。
- 小さな版が無い写真（JavaScript の無いフォーム・API・小さな写真・作れなかった写真）は、元の写真を出します。作れなくても写真は残るので、失敗を知らせません。
- 小さな版はワークスペースの合計の上限に数え、写真を消すときは、元の写真と同じ R2 の呼び出しで一緒に消します。

### 共有のデモと seed

初めて開いた人には、誰でも見て編集できる共有のデモを見せます（[docs/spec.md の「デモ用のデータ」](repo:docs/spec.md#デモ用のデータ)）。デモの ID は公開しません。

- サンプルのレシピと写真は [seed/](repo:seed) にあります。`mise run seed` が、デモをこの状態に戻します（[scripts/seed.ts](repo:scripts/seed.ts)）。
- GitHub Actions の `seed-demo.yml` が、毎日 UTC の 19 時に seed を流し、デモを入れ直します。
- デモでは、写真を API で追加・削除させません。デモの ID が漏れても R2 に書き込ませないためです。そのため seed は、レシピを公開 API で入れ、写真は wrangler で D1 の行と R2 のデータを直接入れます。
- デモのワークスペースには、自動削除と削除から外す印（`mise run pin`）を付けます。

### CI・セキュリティ・サプライチェーン攻撃への対策

画面の応答には、CSP（スクリプトとスタイルは自分のドメインのファイルだけ、インラインは禁止）などのヘッダーを付けます。

<!-- code: apps/recipe-box-rs/src/screen.rs#security-headers -->

| 対策 | 内容 |
| --- | --- |
| ツールの版 | mise で入れ、`mise.lock` で版とチェックサムを固定する |
| 依存の版 | `Cargo.lock`・`pnpm-lock.yaml` で固定する。CI は lockfile と食い違うと失敗する |
| 7 日の待ち | 公開から 7 日たっていない版を使わない（Dependabot・mise・pnpm） |
| 依存の検査 | cargo-deny（脆弱性・ライセンス・取得元）、pnpm の署名の確かめ |
| GitHub Actions | workflow lockfile で、使う action を commit に固定する。job ごとに最小の権限だけを与える |
| 秘密情報 | gitleaks（commit の前と CI）、GitHub の secret scanning |
| 静的解析 | clippy・CodeQL・actionlint・ghalint・zizmor |
| レビュー | PR ごとに Claude Code が自動でレビューする。エージェントは、push の前に不具合とセキュリティのレビューを行う |

詳しくは [docs/security.md](repo:docs/security.md) と、エージェントの決まりの [AGENTS.md](repo:AGENTS.md) にあります。

## リライトで選ぶもの

リライト後の技術スタック（言語・フレームワーク・実行環境・DB・写真の置き場など）は決めていません（[docs/architecture.md の「方針」](repo:docs/architecture.md#方針)）。リライトをする人が選びます。
ここでは、選ばなければならないものと、どれを選んでも守る仕様を挙げます。正解は書かず、選ぶ材料を書きます。

比較の基準にする現行の環境は、選んだ commit をローカルで動かしたものです（[docs/architecture.md の「リライトの比較の基準」](repo:docs/architecture.md#リライトの比較の基準)）。

| 項目 | リライト前のやり方 | 守る仕様 | 選ぶときの注意 |
| --- | --- | --- | --- |
| フロントエンドの言語・フレームワーク | askama で HTML を作り、htmx で一部を入れ替える | [画面](repo:docs/spec.md#画面) | 画面は JavaScript が動かなくても使える仕様である。ブラウザだけで画面を組み立てる SPA にすると守れないので、サーバーで HTML を返すか、段階的な強化（progressive enhancement）にするかを決める |
| バックエンドの言語・フレームワーク | Rust・axum・workers-rs | [API](repo:docs/spec.md#api) | 画面と API の両方を返す。API はリライトの前後で同じに保つ（スモークテストに使う） |
| 実行環境（ホスティング） | Cloudflare Workers | [費用の対策](repo:docs/security.md#費用の対策) | Cloudflare のまま動かすか、ほかのプラットフォームに移すか。1 回の CPU 時間・本文の大きさの上限など、プラットフォームの制約で作りが変わる |
| DB とクエリの書き方 | D1（SQLite）、標準の SQL、`LIMIT ? OFFSET ?`、`instr` | [テーブル](repo:docs/spec.md#テーブル) | どの DB にするか。SQL は標準の範囲で書いているが、標準から外れる `LIMIT ? OFFSET ?` と `instr`（PostgreSQL なら `strpos`）は、選んだ DB で確かめる。ほかに、上限の判定で条件を左から評価する前提（[費用の対策](repo:docs/security.md#費用の対策)）と、batch のトランザクションの意味も DB で変わる。`LOWER` を使わずアプリで作る `search_name` の決まりを保つ。マイグレーションは足すだけにする |
| トランザクション | D1 の batch で、数えることと保存することを 1 つにまとめる | [ワークスペースによるデータの分離](repo:docs/spec.md#ワークスペースによるデータの分離) | 上限の判定（同時に追加されても超えない）を、どの分離レベル・ロックで守るか |
| 写真の保管 | R2。D1 に行を入れて上限を判定してから R2 に書く。小さな版も R2 に置き、合計に数える | [写真](repo:docs/spec.md#写真) | オブジェクトストレージ（R2・S3 など）に置くか、DB に置くか。どちらでも API の仕様は変えない。置き場の料金と制限が変わるので、大きさの上限・合計の上限を見直す（費用の対策を作り直す） |
| 写真の受け取り | multipart のふつうのフォームと、API はバイト列そのもの。`Content-Length` で先に断る。画面の JavaScript は、写真の後に、ブラウザで作った小さな版を別のリクエストで送る。一覧と詳細は `srcset` で選ばせ、小さな版が無ければ元の写真を出す | [写真の API](repo:docs/spec.md#写真の-api)・[小さな版](repo:docs/spec.md#小さな版) | 本文の上限、形式を先頭で決めること、メタデータを取り除くこと。小さな版をどこで作るか（ブラウザか、サーバーか、画像の変換のサービスか）は、CPU 時間の上限と料金で決める。作れないときに元の写真に戻ることを保つ |
| 定期実行 | Cron Trigger（1 時間ごと、使われないワークスペースを消す。結果を `cleanup_runs` に残す） | [ワークスペースによるデータの分離](repo:docs/spec.md#ワークスペースによるデータの分離) | 移した先の定期実行の仕組み。途中で使われたワークスペースを消さない確かめ直しを保つ。途中で止まったときに成功の結果を残さないこと |
| デモのリセット | GitHub Actions（`seed-demo.yml`）が毎日 seed を流す。写真は wrangler で D1 と R2 に直接入れる | [デモ用のデータ](repo:docs/spec.md#デモ用のデータ) | 写真を入れる部分を書き直す。アプリと同じデータの層（ORM など）で入れられるか |
| レート制限 | Cloudflare の Rate Limiting（IP アドレスごと、拠点ごとにおおよそ） | [費用の対策](repo:docs/security.md#費用の対策) | 移した先で、何で IP アドレスごとに数えるか（ミドルウェアと共有の記憶、プラットフォームの機能など）。IP アドレスを、送る側が変えられないヘッダーから取る |
| 静的アセット | Workers の静的アセット（Worker を起動しない） | [画面](repo:docs/spec.md#画面) | CSS・htmx・アイコンの配り方と、CSP（外部の CDN から読まない） |
| ID と乱数 | Web Crypto で 128 ビットの乱数 | [ワークスペースによるデータの分離](repo:docs/spec.md#ワークスペースによるデータの分離) | ID の形と長さを保つ（デモの ID を見分けられないことを含む） |
| ログと監視 | Workers のログ・`wrangler tail`・自動削除の結果を確かめる GitHub Actions の job・Cloudflare の Monitors | [ログ](repo:docs/security.md#ログ)・[失敗の通知](repo:docs/development.md#失敗の通知) | ワークスペースの ID をログに残さない。公開の Actions のログに、成功か失敗かと時間のほかを出さない。移した先で、定期実行の失敗と無料の範囲への接近をどう知らせるか |
| テスト | `Backend` トレイトの偽物・スモークテスト | [API](repo:docs/spec.md#api) | 同じ API に当てるスモークテストは、`API_ONLY=1` を付けて新しいアプリにも使える |
| 開発のツールと依存の管理 | mise・pnpm・cargo-deny・7 日の待ち | [サプライチェーン攻撃の対策](repo:docs/security.md#サプライチェーン攻撃の対策) | TypeScript の側の監査の道具 |
