-- ワークスペース。データを分ける単位で、ログインの代わりに ID で持ち主を見分ける。
-- 標準の SQL の範囲で書く（PostgreSQL に移しても同じ意味になるように）。
-- 日時は、DB ごとに扱いが違う日時型を避け、UNIX 時間のミリ秒を整数で持つ。値はアプリが入れる。
CREATE TABLE workspaces (
    -- 128 ビットの乱数を、16 進数の小文字 32 文字で表したもの。
    id VARCHAR(32) NOT NULL PRIMARY KEY,
    -- 作った日時。
    created_at BIGINT NOT NULL,
    -- 最後に使われた日時。自動削除の判断に使う。
    last_used_at BIGINT NOT NULL,
    -- TRUE なら自動削除の対象から外す。管理用のコマンドでだけ付ける。
    pinned BOOLEAN NOT NULL DEFAULT FALSE
);
