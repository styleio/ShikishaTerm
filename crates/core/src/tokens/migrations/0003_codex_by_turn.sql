-- Codex's counts read again from the start, now that a call is known by its
-- turn and the conversation's total after it. A conversation branched from
-- another begins with the other's counts, written again under new times: by
-- time they were other calls, and what was read before this is kept by
-- time. Read again, both are known the new way and counted once. Codex
-- keeps its records, so nothing is lost by reading them again
DELETE FROM calls WHERE cli = 'codex';
DELETE FROM read_files WHERE cli = 'codex';
