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
- GitHub Actions の `uses:` を足すか変えたら、`gh actions-lock` で `.github/workflows/actions.lock` を作り直す。

## SQL

- 標準の SQL の範囲で書く。D1 は SQLite だが、リライト後に PostgreSQL へ移しても差が出ないように、SQLite だけで通じる関数や型を使わない。
- すべてのクエリを `workspace_id` で絞り込む。

## MCP

- 今は使わない。使うときは、プロジェクトの `.mcp.json` に定義する（誰が clone しても同じ MCP が使えるようにするため）。

## スキル

`.claude/skills/` に、[shoji9x9/skills](https://github.com/shoji9x9/skills) から次のスキルを入れている。

- `pr-finalize-loop`: PR の CI の失敗とレビューの指摘が無くなるまで対応を繰り返す。
- `dependabot-merge`: Dependabot の PR を確かめてマージする。
- `dependabot-alert-issue`: Dependabot の alert から Issue を作る。

入れ方は `gh skill install shoji9x9/skills <スキル名> --agent claude-code --scope project`。
スキルなしでどこまでできるかも見るため、必要かどうか迷うスキルは入れない。
`replace-strategy` とその姉妹スキルは、リライトを始めるときに入れる。
