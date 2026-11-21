-- The record was read job by job without an index on the job: the board is
-- drawn from every open job's tasks, tries and decisions four times a
-- second, and each of those questions read the whole of its table, which
-- holds every job of the last month. Deleting a job found what went with
-- it the same way, a whole table at a time. The tables keep their rows; each is
-- found by its job from here on.

CREATE INDEX tasks_by_job ON tasks(job_id);
CREATE INDEX assignments_by_job ON assignments(job_id);
CREATE INDEX assignments_by_state ON assignments(state);
CREATE INDEX decisions_by_job ON decisions(job_id);
CREATE INDEX questions_by_job ON questions(job_id);
CREATE INDEX questions_by_assignment ON questions(assignment_id);
CREATE INDEX mail_by_job ON mail(job_id, seq);

-- Unread mail in any box: what the watcher asks about every pass, and only
-- a few rows among every piece of mail ever sent
CREATE INDEX mail_unread_anywhere ON mail(box) WHERE is_read = 0;
