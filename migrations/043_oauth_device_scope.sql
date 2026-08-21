-- Persist the validated capability requested by each device authorization.
ALTER TABLE oauth_device_codes
    ADD COLUMN scope TEXT NOT NULL DEFAULT 'mcp';

-- Bind consent text to registered client metadata instead of request text.
ALTER TABLE oauth_device_codes
    ADD COLUMN client_id TEXT REFERENCES oauth_clients(client_id);
