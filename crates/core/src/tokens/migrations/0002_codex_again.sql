-- Codex's counts read again from the start. The first reading counted a
-- count Codex says again when only what is left of its plan changed (the
-- same numbers under a new time, so a key of their own), and read no cache
-- writes. Codex keeps its records, so nothing is lost by reading them again
DELETE FROM calls WHERE cli = 'codex';
DELETE FROM read_files WHERE cli = 'codex';
