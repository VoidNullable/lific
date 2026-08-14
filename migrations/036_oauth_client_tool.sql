-- LIFIC-15: remember which tool a registered OAuth client is, so a reconnect
-- pre-fills (rather than re-asks) the approval pick-list.
--
-- `client_id` comes from DCR (minted once, reused across reconnects), so the
-- tool choice is a stable attribute of the persistent client, not something to
-- re-derive on every visit. NULL for clients registered before this migration
-- and for clients that have never been approved (no tool chosen yet).
ALTER TABLE oauth_clients ADD COLUMN tool_id TEXT;