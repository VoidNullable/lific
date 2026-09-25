-- Retire legacy API keys that cannot be checked through the indexed lookup.
UPDATE api_keys SET revoked = 1 WHERE key_id IS NULL AND revoked = 0;
