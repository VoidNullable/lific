-- Bind OAuth authorization codes and access tokens to the user who
-- approved them (LIF-79). Without this, OAuth-authenticated MCP clients
-- resolve to an anonymous identity (None) and cannot perform any
-- user-scoped action (comments, project-lead operations).
--
-- Nullable for backward compatibility: codes/tokens issued before this
-- migration have no associated user and continue to resolve to None.
ALTER TABLE oauth_codes ADD COLUMN user_id INTEGER;
ALTER TABLE oauth_tokens ADD COLUMN user_id INTEGER;
