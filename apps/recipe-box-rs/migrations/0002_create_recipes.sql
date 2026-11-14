-- レシピ。どのワークスペースのものかを workspace_id で持つ。
-- 主キーを (workspace_id, id) にして、ワークスペースを指定しないとレシピを 1 件に絞れないようにする。
-- ワークスペースを消すと、そのレシピも消える（ON DELETE CASCADE）。
CREATE TABLE recipes (
    workspace_id VARCHAR(32) NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    -- 128 ビットの乱数を、16 進数の小文字 32 文字で表したもの。
    id VARCHAR(32) NOT NULL,
    -- 料理名。長さの上限（100 文字）はアプリでも確かめる（SQLite は VARCHAR の長さを確かめないため）。
    name VARCHAR(100) NOT NULL,
    -- 検索に使う料理名。name の ASCII の英字だけを小文字にしたもので、アプリが作る。
    -- 検索の instr は大文字と小文字を区別するので、両側をこの形にそろえて比べる。
    -- DB の LOWER は SQLite と PostgreSQL で ASCII 以外の扱いが違うので、DB に小文字にさせない。
    search_name VARCHAR(100) NOT NULL,
    -- 何人分か。1 から 100 まで。
    servings INTEGER NOT NULL CHECK (servings BETWEEN 1 AND 100),
    -- 材料と手順。改行で区切った複数行の文字列。空でもよい。
    ingredients TEXT NOT NULL,
    steps TEXT NOT NULL,
    -- 作った日時と、最後に変えた日時（UNIX 時間のミリ秒）。
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (workspace_id, id)
);

-- 一覧（作った日時の新しい順、同じ日時なら ID の大きい順）を、並べ替えずに索引の順で読むための索引。
-- ページ分けの LIMIT で、D1 が読む行をそのページまでに抑える。
CREATE INDEX recipes_by_created_at ON recipes (workspace_id, created_at, id);
