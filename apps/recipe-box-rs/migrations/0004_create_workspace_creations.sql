-- ワークスペースを作った記録。アプリ全体の 1 日に作る数の上限の判定に使う。
-- ワークスペースを消しても記録は消さないので、作ると消すを繰り返しても、1 日の数は戻らない。
-- 古い記録は、自動削除（Issue #7）で消す。
CREATE TABLE workspace_creations (
    -- 作ったワークスペースの ID。
    id VARCHAR(32) NOT NULL PRIMARY KEY,
    -- 作った日時（UNIX 時間のミリ秒）。
    created_at BIGINT NOT NULL
);

-- 1 日に作った数を、表の全体を読まずに数えるための索引。
CREATE INDEX workspace_creations_by_created_at ON workspace_creations (created_at);

-- 今あるワークスペースの記録を入れる（この表を作る前に作られたものも、その日の数に入れるため）。
INSERT INTO workspace_creations (id, created_at)
SELECT id, created_at FROM workspaces;
