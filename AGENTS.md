# AGENTS.md

このリポジトリで作業するコーディングエージェント向けの決まり。
アプリの目的・仕様・技術構成は [README.md](README.md) にある。仕様を変えるときは、README.md も一緒に直す。

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
- push の前に、push するコミット（ベースブランチとの差分）に、次の 2 つを行う。見つかった問題を直してから push する。直さないものは、理由を PR に書く。
  1. コードレビュー: Claude Code の `/code-review high`（不具合を探す）。
  2. セキュリティのレビュー: `security-audit` スキルの guidance mode（範囲を差分に絞ったレビュー）。
     スキルの full audit mode（6 つの段階・多数のサブエージェント・報告のファイル）は重いので、push のたびには行わない。利用者に求められたときだけ行う。
  文書だけの変更（`*.md` など）では、2 を省いてよい。

## SQL

- 標準の SQL の範囲で書く。D1 は SQLite だが、リライト後に PostgreSQL へ移しても差が出ないように、SQLite だけで通じる関数や型を使わない。
  例外は次のものだけで、使う箇所にコメントで理由を書く。例外を増やすときは、利用者の承認を得て、ここに足す。
  - 一覧のページ分けの `LIMIT ? OFFSET ?`。ISO の標準（`OFFSET ... FETCH FIRST ...`）は SQLite が受け付けないが、`LIMIT` と `OFFSET` は PostgreSQL でも同じ意味で動く。
  - 料理名の部分一致の検索の `instr(search_name, ?) > 0`。D1 は `LIKE` のパターンを 50 バイトまでに制限していて、日本語の検索の文字列が 16 文字ほどで通らないため。リライトの後は `strpos` などに書き換える。
- すべてのクエリを `workspace_id` で絞り込む。
  ワークスペースそのものを作る・探すクエリと、自動削除のクエリと、アプリ全体の量の上限を数えるクエリ（README の「費用の対策」）は、`workspaces` の `id` で絞り込むか、全体を対象にする。
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

[cloudflare/security-audit-skill](https://github.com/cloudflare/security-audit-skill) から、次のスキルを入れている（commit を固定。上げるときは公開から 7 日たった commit を選び、中身を読んでから入れ直す）。

- `security-audit`: セキュリティのレビュー。push の前の確認（「確認」）に使う。
  入れ方は `gh skill install cloudflare/security-audit-skill security-audit --agent claude-code --scope project --pin <commit>`。
  スキルは、ビルドやテストなど対象のコードを動かすのを、外部のネットワークを切った OS のサンドボックスの中だけに限る。その用意が無ければ、コードを動かさずに読むだけで確かめる。

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

スキルなしでどこまでできるかも見るため、必要かどうか迷うスキルは入れない。
`replace-strategy` とその姉妹スキルは、リライトを始めるときに入れる。
