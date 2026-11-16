-- 写真の行に、R2 への書き込みか削除が終わっていない印を足す（Issue #48）。
-- 写真を追加するときは、印を付けて行を入れ（上限を R2 に書く前に D1 で判定するため）、R2 に書けたら印を外す。
-- 写真を消すときは、先に印を付けて隠してから、R2 と D1 を消す。
-- 途中で失敗したり Worker が止められたりして印が付いたまま残った行は、毎日の自動削除が R2 のデータと一緒に消す。
-- 印の付いた行は、画面と API に見せないが、上限には数える（同時に追加されても上限を超えないため）。
-- 既定を FALSE（終わった）にし、この列を知らない古いコードが入れた行も、今までどおり見せる。
ALTER TABLE photos ADD COLUMN pending BOOLEAN NOT NULL DEFAULT FALSE;

-- 自動削除が、印の付いた行を、表の全体を読まずに探すための索引。
CREATE INDEX photos_by_pending ON photos (pending, created_at);
