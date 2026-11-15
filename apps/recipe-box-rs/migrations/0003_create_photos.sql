-- レシピの写真。写真のデータそのものは R2 に置き、ここには R2 のキーを決める ID と、上限の判定に使う大きさを持つ。
-- R2 のキーは photos/<workspace_id>/<recipe_id>/<id> で、この表の列から決まる。
-- レシピを消すと、この表の行も消える（ON DELETE CASCADE）。R2 のデータは、アプリが先に消す。
CREATE TABLE photos (
    workspace_id VARCHAR(32) NOT NULL,
    recipe_id VARCHAR(32) NOT NULL,
    -- 128 ビットの乱数を、16 進数の小文字 32 文字で表したもの。
    id VARCHAR(32) NOT NULL,
    -- 形式。image/jpeg・image/png・image/webp のどれか。アプリが中身の先頭のバイトから決める。
    content_type VARCHAR(20) NOT NULL,
    -- 大きさ（バイト）。アプリ全体とワークスペースごとの合計の上限の判定に使う。
    size INTEGER NOT NULL,
    -- 追加した日時（UNIX 時間のミリ秒）。
    created_at BIGINT NOT NULL,
    PRIMARY KEY (workspace_id, recipe_id, id),
    FOREIGN KEY (workspace_id, recipe_id) REFERENCES recipes (workspace_id, id) ON DELETE CASCADE
);

-- 写真を追加した記録。アプリ全体の 1 日の追加の回数の上限の判定に使う。
-- 写真を消しても記録は消さないので、追加と削除を繰り返しても回数は戻らない（R2 の書き込みの操作は、消しても戻らないため）。
-- 古い記録は、自動削除（Issue #7）で消す。
CREATE TABLE photo_uploads (
    id VARCHAR(32) NOT NULL PRIMARY KEY,
    created_at BIGINT NOT NULL
);

-- 1 日の追加の回数を、表の全体を読まずに数えるための索引。
CREATE INDEX photo_uploads_by_created_at ON photo_uploads (created_at);
