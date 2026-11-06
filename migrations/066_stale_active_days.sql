-- LIF-492: an active issue untouched for this many days is reported as stale
-- by get_briefing and list_issues(stale=true). Instance-wide and admin-editable
-- like the other runtime settings in this row.
ALTER TABLE instance_settings ADD COLUMN stale_active_days INTEGER NOT NULL DEFAULT 3;
