-- レシピ。どのワークスペースのものかを workspace_id で持つ。
-- 主キーを (workspace_id, id) にして、ワークスペースを指定しないとレシピを 1 件に絞れないようにする。
-- ワークスペースを消すと、そのレシピも消える（ON DELETE CASCADE）。
CREATE TABLE recipes (
    workspace_id VARCHAR(32) NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    -- 128 ビットの乱数を、16 進数の小文字 32 文字で表したもの。
    id VARCHAR(32) NOT NULL,
    -- 料理名。長さの上限（100 文字）はアプリでも確かめる（SQLite は VARCHAR の長さを確かめないため）。
    name VARCHAR(100) NOT NULL,
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
