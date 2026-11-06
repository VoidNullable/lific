-- LIF-494: a nightly job returns active issues untouched for this many days to
-- todo, with a comment saying why. 0 turns the job off.
ALTER TABLE instance_settings ADD COLUMN stale_active_reset_days INTEGER NOT NULL DEFAULT 14;
