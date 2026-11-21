-- The conversations held in one folder are found from the folder each was
-- first seen in, and the panel asks every time the conference changes:
-- every row of every conversation was read for it. The folder and the tab
-- are in an index of their own, so only those two are read.
CREATE INDEX conversations_by_folder ON conversations (observed_cwd, tab) WHERE observed_cwd IS NOT NULL;
