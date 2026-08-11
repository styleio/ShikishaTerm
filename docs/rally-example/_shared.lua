-- ラリーの設定。このワークスペース用に書き換える。
-- _shared.lua は同じディレクトリのフックより先に読まれ、名前空間を共有する。
--
-- 置き場所: このディレクトリを、AIセッションタブの automation に指す。
--   例) config.json のAIタブ:  { "name": "claude", "command": "claude",
--         "automation": "scripts/rally" }
--
-- 注意: ラリーの往復は「自動チェーン(透明のボール)」を1回ずつ数える。
-- 全体設定の max_chain を RALLY.max_steps 以上に上げておくこと
-- (既定10だと10往復で止まる)。人間がAIタブに打つとチェーンは0に戻り、
-- on_done は反応しなくなる(暴走・乗っ取り防止)。

RALLY = {
  -- 操作するブラウザタブの id (config.json のブラウザタブの "id")
  browser = "br",

  -- 目的。AIにそのまま渡る
  goal = "（ここに目的を書く。例: 日記SaaSにログインして本文を投稿する）",

  -- 安全のための往復上限
  max_steps = 20,

  -- 1手ごとにAIへ返す画面テキストの最大文字数
  screen_chars = 3000,
}
