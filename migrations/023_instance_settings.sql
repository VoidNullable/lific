-- Instance-wide settings, editable at runtime by admins (LIF-210/211).
--
-- Single-row table: `id` is pinned to 1 by a CHECK so there is exactly one
-- settings row. The app seeds the row at startup (see db::queries::settings),
-- using the TOML `auth.allow_signup` as the initial default; once the row
-- exists the DB value is authoritative and the UI/CLI edit it live.
CREATE TABLE IF NOT EXISTS instance_settings (
    id                    INTEGER PRIMARY KEY CHECK (id = 1),
    -- Whether self-service signup is open.
    allow_signup          INTEGER NOT NULL DEFAULT 1,
    -- Human name for the instance (NULL = fall back to the host).
    instance_name         TEXT,
    -- Comma-separated lowercase email domains that may self-register.
    -- Empty string = no restriction.
    signup_email_domains  TEXT NOT NULL DEFAULT '',
    -- How long a login session stays valid, in days.
    session_lifetime_days INTEGER NOT NULL DEFAULT 30,
    -- Short note shown on the auth screen (NULL = none).
    login_message         TEXT,
    updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
);
