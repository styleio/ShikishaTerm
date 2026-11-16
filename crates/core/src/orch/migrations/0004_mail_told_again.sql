-- A tab told of its mail that never came for it was never told again: the
-- line saying so is one line among many on a busy terminal, an AI that was
-- told mid-task can lose it when its context is cut down, and from then on
-- the mail sat unread with nothing left to point at it -- a worker's report
-- nobody collected, a question nobody answered.
--
-- So being told is no longer once and for all. When a tab was last told of
-- a piece of mail, and how many times, are kept: unread mail is pointed out
-- again after a while, a few times, and then left (a tab that will not read
-- its mail is not asked for ever).
--
-- Mail told of before this step has no time written down. It reads as told
-- long ago, once: what was already stuck is pointed out again.

-- when the box's tab was last told this is there, in milliseconds since 1970
ALTER TABLE mail ADD COLUMN told_at INTEGER;
-- how many times it has been told
ALTER TABLE mail ADD COLUMN told_times INTEGER NOT NULL DEFAULT 0;

UPDATE mail SET told_times = 1 WHERE is_told = 1;
