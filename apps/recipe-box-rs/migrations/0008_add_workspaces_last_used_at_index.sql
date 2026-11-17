-- 使われていないワークスペース（印が無く、最終利用日時が古いもの）を、表の全体を読まずに探すための索引（Issue #56）。
-- 自動削除が対象を選ぶクエリと、失敗を知らせる確かめ（scripts/check-cleanup.ts）が、消し終えているはずのワークスペースを数えるクエリが使う。
CREATE INDEX workspaces_by_pinned_last_used_at ON workspaces (pinned, last_used_at);
