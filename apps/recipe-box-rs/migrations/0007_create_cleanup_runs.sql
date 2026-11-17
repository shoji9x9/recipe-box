-- 定期の自動削除の、1 回ごとの結果（Issue #56）。失敗を知らせる確かめ（scripts/check-cleanup.ts）が、最後の成功を読む。
-- 自動削除の最後に、1 回の batch で行を足し、古い行を消す。途中で止まった実行は行を残さないので、
-- 「最近に成功の行が無い」ことで、失敗も止まったことも見つけられる。
-- 中身は数と日時だけで、ワークスペースの ID は持たない。
CREATE TABLE cleanup_runs (
    -- 実行を始めた日時（UNIX 時間のミリ秒）。
    ran_at BIGINT NOT NULL,
    -- TRUE なら、どこも失敗しなかった。
    succeeded BOOLEAN NOT NULL,
    -- 消したワークスペースの数。
    deleted INTEGER NOT NULL,
    -- 失敗した数（ワークスペースと、途中で残った写真の行）。選ぶところの失敗は数に入らず、succeeded だけに出る。
    failed INTEGER NOT NULL
);

-- 最後に成功した日時を、表の全体を読まずに探すための索引。古い行を消すときにも使う。
CREATE INDEX cleanup_runs_by_ran_at ON cleanup_runs (ran_at);
