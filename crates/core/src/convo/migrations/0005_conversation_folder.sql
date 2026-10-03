-- Keep where a tab first carried this conversation. Moving the tab must not
-- make its earlier conversations look like another folder's records.
ALTER TABLE conversations ADD COLUMN observed_cwd TEXT
    /* NULL: an older sighting, or a tab whose local folder was not known. */;
