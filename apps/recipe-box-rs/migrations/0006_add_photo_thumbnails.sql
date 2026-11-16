-- 写真の小さな版（Issue #52）。画面の JavaScript が、写真を追加した後に canvas で作って送る（JPEG）。
-- 小さな版のデータは R2 のキー thumbnails/<workspace_id>/<recipe_id>/<id> に置く。
-- 列を足すだけにする（デプロイはマイグレーションを当ててからコードを入れ替えるので、古いコードでも動くように）。
-- どの列も既定を NULL（小さな版が無い）にし、今ある写真と、小さな版の無い写真（API や JavaScript の無い経路で追加したもの）は、
-- 元の写真を出す。

-- 小さな版の大きさ（バイト）。ワークスペースの写真の合計の上限に、元の写真と合わせて数える。
-- R2 への書き込みより前に入れ（上限の判定のため）、書き込みに失敗したら NULL に戻す。
ALTER TABLE photos ADD COLUMN thumbnail_size INTEGER;
-- 小さな版の幅（ピクセル）。R2 に書けてから入れ、画面はこれが NULL でないときだけ小さな版を出す（srcset の幅）。
ALTER TABLE photos ADD COLUMN thumbnail_width INTEGER;
-- 元の写真を向きのとおりに置いたときの幅（ピクセル）。画面の JavaScript が測って送る（srcset の幅）。
ALTER TABLE photos ADD COLUMN width INTEGER;
