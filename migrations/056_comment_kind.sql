-- LIF-486: a comment can be typed verification evidence recorded when an issue
-- is closed. Every existing row is an ordinary comment. ADD COLUMN cannot fire
-- the seq-stamping or audit triggers, so no row moves in the sync stream.
ALTER TABLE comments ADD COLUMN kind TEXT NOT NULL DEFAULT 'comment'
    CHECK (kind IN ('comment', 'verification'));
