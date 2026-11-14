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

## SQL

- 標準の SQL の範囲で書く。D1 は SQLite だが、リライト後に PostgreSQL へ移しても差が出ないように、SQLite だけで通じる関数や型を使わない。
- すべてのクエリを `workspace_id` で絞り込む。
  ワークスペースそのものを作る・探すクエリと、自動削除のクエリは、`workspaces` の `id` で絞り込むか、全体を対象にする。
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
push せずに再レビューが要るときは、`@claude review` を投稿する代わりに、PR からラベル `claude-review` を外してから付け直す。
付けたときにだけ動くので、付いたままのラベルは先に外す（`gh pr edit <番号> --remove-label claude-review` の後に `--add-label claude-review`）。
`gh workflow run` で動かす方法は、インラインのコメントを書けないので使わない（Issue #18）。
スキルなしでどこまでできるかも見るため、必要かどうか迷うスキルは入れない。
`replace-strategy` とその姉妹スキルは、リライトを始めるときに入れる。
