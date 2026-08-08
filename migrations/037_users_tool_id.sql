-- LIFIC-17: stable agent identity.
--
-- A connected agent is deduplicated on (owner_id, tool_id), NOT the derived
-- `{tool}-{owner.username}` string. Renaming the owner changes the string but
-- not the pair, so the agent must survive a rename. NULL for human users and
-- for bots minted before this migration (legacy bots are backfilled lazily on
-- their next connect).
ALTER TABLE users ADD COLUMN tool_id TEXT;