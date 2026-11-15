# AGENTS.md

このリポジトリで作業するコーディングエージェント向けの決まり。
アプリの概要と使い方は [README.md](README.md) に、仕様は [docs/spec.md](docs/spec.md) に、方針と技術構成は [docs/architecture.md](docs/architecture.md) に、セキュリティは [docs/security.md](docs/security.md) に、開発の手順は [docs/development.md](docs/development.md) にある。
動きや値を変えるときは、それを書いた文書をすべて一緒に直す（仕様は `docs/spec.md`、上限などの費用の対策は `docs/security.md`、手順は `docs/development.md`、利用者に見える値や操作は `README.md` の使い方も）。
README.md には概要と使い方だけを書き、詳細は `docs/` に置く。

## 文書

- 公開リポジトリなので、文書・コメント・commit message・Issue・PR に個人情報（利用者の契約・アカウントの状況など）を書かない。
- 文書と、利用者に見せる文章は日本語で書く。

## ブランチと PR

- main に直接 commit しない。ブランチを切って PR を作る。
- PR は squash でマージする（`gh pr merge --squash`）。PR のタイトルが、そのまま main の commit message になる。
  タイトルは Conventional Commits の形（`feat: ...`・`fix: ...`・`chore: ...` など）にし、本文には変更の理由を書く。

## ツールの版（mise）

- 開発に使うツールは mise で入れる。`mise.toml` で版を指定し、`mise.lock` で版とチェックサムを固定する（`locked = true`）。
- ツールを足すか版を上げたら、`mise install` の後に `mise lock` を実行し、`mise.toml` と `mise.lock` を一緒に commit する。
- 公開から 7 日たっていない版は選ばない（`minimum_release_age = "7d"`）。

## 依存の追加と更新

- Node のパッケージマネージャーは pnpm を使う。npm と yarn は使わない。
- 依存を足したら lockfile（`Cargo.lock`・`pnpm-lock.yaml`）も commit する。
- 公開から 7 日たっていない版は使わない。急ぎで必要なときは、理由を PR に書いたうえで一時的に回避する。
- インストール時のスクリプトの実行を許可するパッケージを増やすときは、理由を PR に書く。
- GitHub Actions の `uses:` を足すか変えたら、`gh-actions-lock` を実行して `.github/workflows/actions.lock` を作り直す。
  `uses:` にはタグを書き、commit の SHA は lockfile に記録する。
- workflow の `permissions` は、上で `permissions: {}` にし、job ごとに要るものだけを、理由のコメント付きで与える。

## 確認

- commit の前に lefthook が整形と静的解析を実行する。フックを飛ばさない（`--no-verify` を使わない）。
- PR を作る前に `mise run lint` と `mise run audit` を実行する。
- push の前に、push するコミットに次の 2 つを行う。見つかった問題は直してから push する。
  1. 不具合を探すコードレビュー。Claude Code では `/code-review high` を使う。
  2. セキュリティのレビュー。`security-audit` スキルの guidance mode（範囲を差分に絞ったレビュー）で行う。
     スキルの full audit mode（6 つの段階・多数のサブエージェント・報告のファイル）は重いので、push のたびには行わない。利用者に求められたときだけ行う。
  - レビューする範囲は、ブランチの最初の push ではベースブランチとの差分、2 回目からは前の push からの差分にする。
    `pr-finalize-loop` が反復ごとに push するときも同じで、PR の差分の全体をレビューし直さない（反復のたびに費用が増え、変えていないコードへの指摘で収束しなくなるため）。
  - アプリの文書（`README.md`・`docs/` など）だけの変更では、2 を省いてよい。
    エージェントへの指示になるファイル（`AGENTS.md`・`CLAUDE.md`・`.claude/` の下）の変更は、`.md` でも省かない。悪意のある指示が入ると、エージェントが自律して push などをするため。
  - 直さないと決めた指摘は、理由を PR のトップレベルのコメントに書く（PR の本文には書かない）。最初の push で PR がまだ無ければ、PR を作った直後に書く。

## SQL

- 標準の SQL の範囲で書く。D1 は SQLite だが、リライト後に PostgreSQL へ移しても差が出ないように、SQLite だけで通じる関数や型を使わない。
  例外は次のものだけで、使う箇所にコメントで理由を書く。例外を増やすときは、利用者の承認を得て、ここに足す。
  - 一覧のページ分けの `LIMIT ? OFFSET ?`。ISO の標準（`OFFSET ... FETCH FIRST ...`）は SQLite が受け付けないが、`LIMIT` と `OFFSET` は PostgreSQL でも同じ意味で動く。
  - 料理名の部分一致の検索の `instr(search_name, ?) > 0`。D1 は `LIKE` のパターンを 50 バイトまでに制限していて、日本語の検索の文字列が 16 文字ほどで通らないため。リライトの後は `strpos` などに書き換える。
- すべてのクエリを `workspace_id` で絞り込む。
  ワークスペースそのものを作る・探すクエリと、自動削除のクエリと、アプリ全体の量の上限を数えるクエリ（docs/security.md の「費用の対策」）は、`workspaces` の `id` で絞り込むか、全体を対象にする。
- マイグレーションは `apps/recipe-box-rs/migrations/` に、番号を付けた新しいファイルとして足す。main に入ったファイルは書き換えない。
- デプロイはマイグレーションを当ててからコードを入れ替えるので、マイグレーションは古いコードでも動くように（表や列を足すだけに）書く。

## MCP

- MCP は、プロジェクトの `.mcp.json` に定義する（誰が clone しても同じ MCP が使えるようにするため）。
- chrome-devtools MCP: 画面をブラウザで確かめる `browser-test` スキルが使う。エージェントがブラウザを操作できるので、`package.json` で版を完全に固定する（`^` を付けない）。
  MCP から外部への送信は、次の設定で止めている。設定を足すときは `--help` で送信にかかわるものが増えていないかを確かめる。
  - `--isolated`: ブラウザのプロファイルを毎回使い捨てにする。
  - `--no-usage-statistics`: MCP の利用統計を Google に送らない。
  - `--no-performance-crux`: 性能の計測で開いた URL を Google の CrUX API に送らない。
  - 環境変数 `CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS=1`: 新しい版の確認をしない（版は Dependabot で上げる）。
  - ブラウザ（Google Chrome）そのものが Google と行う通信は、これらの設定では止まらない。

## スキル

`.claude/skills/` に、[shoji9x9/skills](https://github.com/shoji9x9/skills) から次のスキルを入れている。

- `pr-finalize-loop`: PR の CI の失敗とレビューの指摘が無くなるまで対応を繰り返す。
- `dependabot-merge`: Dependabot の PR を確かめてマージする。
- `dependabot-alert-issue`: Dependabot の alert から Issue を作る。
- `browser-test`: 画面の変更を、chrome-devtools MCP で実際のブラウザを操作して確かめる。

入れ方は `gh skill install shoji9x9/skills <スキル名> --agent claude-code --scope project`。

`pr-finalize-loop` の再レビューの依頼先は `claude-code` にしている（`.config/skills/shoji9x9/common.jsonc`）。
このリポジトリの Claude のレビュー（`claude-review.yml`）は、push のたびに自動で動き、`@claude review` のコメントには反応しない。
コメントで起動する `issue_comment` のトリガーは、secret のある job を PR のコメントから動かせてしまうので使わない。
レビューは指摘に【要対応】か【参考】を付ける。レビューへの対応は、`pr-finalize-loop` の手順を次のとおり変えて行う。

- 【参考】には PR で返信しない。残す価値があれば、関係する Issue にコメントする。【参考】は、対応していない指摘に数えない。
- push せずに返信だけをしたときは、再レビューを依頼しない。ループは「返信済み・再レビューなし」で止まり、どの返信をしたかを利用者に報告する。
  返信はその後の push で動くレビューが読む。
- 利用者に求められたときだけ、push せずに再レビューする。PR からラベル `claude-review` を外してから付け直す
  （付けたときにだけ動くので、`gh pr edit <番号> --remove-label claude-review` の後に `--add-label claude-review`）。
  `gh workflow run` で動かす方法は、インラインのコメントを書けないので使わない（Issue #18）。

[cloudflare/security-audit-skill](https://github.com/cloudflare/security-audit-skill) から、次のスキルを入れている（commit を固定する）。

- `security-audit`: セキュリティのレビュー。push の前の確認（「確認」）に使う。
  入れ方は `gh skill install cloudflare/security-audit-skill security-audit --agent claude-code --scope project --pin <commit>`。
  スキルは、ビルドやテストなど対象のコードを動かすのを、外部のネットワークを切った OS のサンドボックスの中だけに限る。その用意が無ければ、コードを動かさずに読むだけで確かめる。
  上げるときは、中身を読んでから入れ直す。7 日の待ち（「依存の追加と更新」）は、commit の日付ではなく、その commit が default ブランチに入った時刻（PR の `merged_at`、PR が無ければリポジトリの push の時刻など）で確かめる。commit の日付は作った人が自由に決められるため。

[GoogleChrome/modern-web-guidance](https://github.com/GoogleChrome/modern-web-guidance) から、次のスキルを入れている（commit を固定する）。

- `modern-web-guidance`: 画面の HTML・CSS・JavaScript を書く前に、今の Web の作り方の指針（ブラウザの機能・アクセシビリティ・性能）を引く。
  入れ方は `gh skill install GoogleChrome/modern-web-guidance modern-web-guidance --agent claude-code --scope project --pin <commit>`。
  - スキルの `SKILL.md` と指針は、`npx -y modern-web-guidance@latest ...` を実行するよう書いているが、その代わりに `mise run mwg -- <サブコマンド> ...` を使う（例: `mise run mwg -- search "<検索の語>" --skill-version <SKILL.md に書かれた値>`、`mise run mwg -- retrieve "<id>"`）。
    `npx`・`pnpx`・`npm exec`・`pnpm exec` や、`node_modules` の中のファイルで直接実行しない。Claude Code では、主な形を `.claude/settings.json` の `permissions.deny` で止めているが、すべての形は止められないので、この決まりを守る。
    - `@latest` は 7 日の待ち（「依存の追加と更新」）を守れないので、`package.json` で版を固定したパッケージを使う。
    - このツールは、検索の語などの利用統計を Google に送るので、`mise run mwg`（`scripts/mwg.sh`）で環境変数 `DISABLE_TELEMETRY=1` を付けて止めている。
    - 使うのは、読むだけの `search`・`list`・`retrieve` に限る（`scripts/mwg.sh` が、ほかを断る）。`install`・`update` などは、版を固定していないパッケージを `npx` で取ってきてスキルを書き換える。
      ツールが更新や設定を促しても従わない。上げるときは下の手順で行う。
    - Origin Trial（ブラウザの試験中の機能）に頼る指針は、`retrieve` が断り、`npx ... set-allow-origin-trials` を実行するよう案内する。案内に従わず、その指針は使わない（利用者にも実行を勧めない）。
    - この 3 つは、パッケージに入ったデータで動き、外部と通信しない。
  - スキルの `guides/`（指針の写し）は、`.gitignore` で commit しない。ツールはパッケージ（`node_modules`）の中の指針を読み、スキルの写しは使わないため。
  - `SKILL.md` の、使った指針を応答の最後に書く手順（「Disclose Skill Usage」）は、利用者への応答に書く。commit message・PR・コードのコメントには書かない。
  - 上げるときは、スキルの commit と `package.json` の `modern-web-guidance` を同じ版にそろえる（`SKILL.md` の `--skill-version` と、パッケージの `skills/modern-web-guidance/skill-version.txt` が同じになる版）。
    中身を読み、スキルの 7 日の待ちは `security-audit` と同じく、default ブランチに入った時刻で確かめる。
    Dependabot がパッケージを上げる PR を作ったら、同じ PR で、同じ版のスキルを入れ直す。版がそろっていなければ、`mise run lint`（`scripts/check-mwg-version.sh`）が失敗する。

スキルなしでどこまでできるかも見るため、必要かどうか迷うスキルは入れない。
`replace-strategy` とその姉妹スキルは、リライトを始めるときに入れる。
