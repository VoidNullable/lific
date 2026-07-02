-- RFC 8628 OAuth 2.0 Device Authorization Grant (LIF-252).
--
-- Backs the agent-drivable `lific login` device flow: the CLI POSTs to
-- /oauth/device_authorization, the human approves the short user_code on any
-- device at /oauth/device, and the CLI polls the token endpoint with the
-- device grant until approved/denied/expired.
--
-- We store only the SHA-256 hash of the high-entropy device_code (never the
-- raw value), mirroring how oauth_tokens are stored. The user_code is the
-- short human-typed code (e.g. BCDF-GHJK) and is stored in the clear because
-- it must be matched case-insensitively on the verification page.
CREATE TABLE IF NOT EXISTS oauth_device_codes (
    device_code_hash TEXT PRIMARY KEY,
    user_code        TEXT NOT NULL UNIQUE,
    client_name      TEXT,                       -- optional label shown on the approval page
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at       TEXT NOT NULL,
    interval_seconds INTEGER NOT NULL DEFAULT 5,
    -- pending | approved | denied | consumed
    status           TEXT NOT NULL DEFAULT 'pending',
    -- user who approved (bound at approval time); NULL until approved
    user_id          INTEGER,
    -- last time the token endpoint was polled, for slow_down enforcement
    last_polled_at   TEXT
);

CREATE INDEX IF NOT EXISTS idx_oauth_device_codes_user_code
    ON oauth_device_codes(user_code);
