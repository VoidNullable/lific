-- Add key_id column for O(1) API key lookup instead of scanning all hashes.
-- key_id is a deterministic BLAKE3 hash of the API key (32 hex chars).
ALTER TABLE api_keys ADD COLUMN key_id TEXT;
CREATE INDEX IF NOT EXISTS idx_api_keys_key_id ON api_keys(key_id) WHERE revoked = 0;
