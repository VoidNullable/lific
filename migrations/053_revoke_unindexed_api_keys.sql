-- Unindexed legacy keys cannot be selected safely during authentication.
-- Revoke them during upgrade so requests never scan every legacy verifier.
UPDATE api_keys SET revoked = 1 WHERE key_id IS NULL AND revoked = 0;
