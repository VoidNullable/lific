-- LIF-367: enforce the (owner_id, tool_id) agent identity in the schema.
--
-- Migration 037 added `users.tool_id` and the dedupe was left to application
-- code alone: `ensure_bot` looks the pair up, and mints a bot when the lookup
-- misses. Two connects racing that read-then-write both miss and both mint, so
-- one owner ends up with two bots for the same tool. The fix is a partial
-- unique index, which needs the existing rows deduped first.
--
-- Step 1 collects the duplicates: for every (owner_id, tool_id) group of bots
-- with more than one row, the lowest id survives (it owns the oldest history)
-- and the rest are losers. `owner_id IS NOT NULL` mirrors the index, which
-- SQLite does not apply to NULL owners since NULLs never collide there.
CREATE TEMP TABLE _bot_dupe_map AS
SELECT u.id AS loser_id,
       (SELECT MIN(u2.id)
          FROM users u2
         WHERE u2.is_bot = 1
           AND u2.tool_id IS NOT NULL
           AND u2.owner_id IS NOT NULL
           AND u2.owner_id = u.owner_id
           AND u2.tool_id = u.tool_id) AS survivor_id
  FROM users u
 WHERE u.is_bot = 1
   AND u.tool_id IS NOT NULL
   AND u.owner_id IS NOT NULL;

DELETE FROM _bot_dupe_map WHERE loser_id = survivor_id;

-- Step 2 repoints every column in the schema that names a user at the
-- survivor, so nothing the loser did (its keys, tokens, comments, audit
-- trail) is lost when the row goes away. The list below is the complete set
-- of user-referencing columns as of migration 037, declared foreign key or
-- not.
--
-- Tables with a uniqueness constraint that includes user_id use
-- Tables with a uniqueness constraint that includes user_id cannot simply be
-- repointed, because the survivor may already hold a row for the same key.
-- Dropping the loser's row is only correct where the two rows carry no
-- information beyond the key itself; anywhere they can differ, the two are
-- merged first. Each of the four cases is handled on its own terms below.

-- api_keys.user_id (FK)
UPDATE api_keys
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- sessions.user_id (FK)
UPDATE sessions
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- comments.user_id (FK)
UPDATE comments
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- attachments.uploader_id (FK)
UPDATE attachments
   SET uploader_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = uploader_id)
 WHERE uploader_id IN (SELECT loser_id FROM _bot_dupe_map);

-- projects.lead_user_id (FK)
UPDATE projects
   SET lead_user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = lead_user_id)
 WHERE lead_user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- users.owner_id (self FK) — a loser bot owning another user is not a shape
-- Lific mints today, but the column is a user reference and is treated as one.
UPDATE users
   SET owner_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = owner_id)
 WHERE owner_id IN (SELECT loser_id FROM _bot_dupe_map);

-- oauth_codes.user_id (no declared FK)
UPDATE oauth_codes
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- oauth_tokens.user_id (no declared FK)
UPDATE oauth_tokens
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- oauth_device_codes.user_id (no declared FK)
UPDATE oauth_device_codes
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- audit_log.actor_user_id (no declared FK, deliberately). Repointing keeps the
-- history attributed to the identity that actually wrote it — the two rows
-- were always the same agent.
UPDATE audit_log
   SET actor_user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = actor_user_id)
 WHERE actor_user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- _actor_state.user_id — the single-row write-attribution stamp.
UPDATE _actor_state
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- comment_mentions PRIMARY KEY (comment_id, user_id)
--
-- The only columns are the key and a timestamp: "this comment mentions this
-- user" is a fact that is either recorded or not, so a colliding loser row
-- carries nothing the survivor's row does not already say. Dropping it is
-- lossless.
UPDATE OR IGNORE comment_mentions
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);
DELETE FROM comment_mentions WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- project_members PRIMARY KEY (project_id, user_id), role TEXT
--
-- The two rows can hold *different roles* in the same project, and the
-- loser's may be the stronger one, so the survivor's role is raised to the
-- strongest of the group before the loser's row is dropped. Privilege order
-- is viewer < maintainer < lead, from `Role` in src/db/models.rs (the derived
-- `Ord` on the variant order, guarded there by
-- `role_ordering_is_viewer_lt_maintainer_lt_lead`); the strings match the
-- CHECK constraint on the column.
CREATE TEMP TABLE _bot_member_merge AS
SELECT m.survivor_id AS user_id,
       pm.project_id AS project_id,
       MAX(CASE pm.role WHEN 'lead' THEN 3 WHEN 'maintainer' THEN 2 ELSE 1 END) AS rank
  FROM project_members pm
  JOIN _bot_dupe_map m ON m.loser_id = pm.user_id
 GROUP BY m.survivor_id, pm.project_id;

UPDATE project_members
   SET role = CASE (
           SELECT MAX(
                      mm.rank,
                      CASE project_members.role
                          WHEN 'lead' THEN 3 WHEN 'maintainer' THEN 2 ELSE 1 END)
             FROM _bot_member_merge mm
            WHERE mm.user_id = project_members.user_id
              AND mm.project_id = project_members.project_id)
       WHEN 3 THEN 'lead' WHEN 2 THEN 'maintainer' ELSE 'viewer' END
 WHERE EXISTS (SELECT 1 FROM _bot_member_merge mm
                WHERE mm.user_id = project_members.user_id
                  AND mm.project_id = project_members.project_id);

DROP TABLE _bot_member_merge;

-- The survivor now holds at least the role the loser had, so a colliding
-- loser row really is redundant; a non-colliding one moves across untouched.
UPDATE OR IGNORE project_members
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);
DELETE FROM project_members WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- project_groups UNIQUE (user_id, name), with project_group_items keyed on
-- (group_id, project_id) and cascading on delete.
--
-- Deleting a colliding loser group would take its items with it, so the items
-- are reparented into the group that survives the name clash first. The
-- keeper is the survivor's own group of that name when it has one, otherwise
-- the oldest loser group of that name (which then gets repointed like any
-- other). Items are keyed entirely by (group_id, project_id), so a duplicate
-- landing on the keeper is genuinely the same membership and is dropped.
CREATE TEMP TABLE _bot_group_merge AS
SELECT l.id AS loser_group_id,
       COALESCE(
           (SELECT s.id FROM project_groups s
             WHERE s.user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = l.user_id)
               AND s.name = l.name),
           (SELECT MIN(g.id) FROM project_groups g
             WHERE g.name = l.name
               AND g.user_id IN (SELECT loser_id FROM _bot_dupe_map
                                  WHERE survivor_id = (SELECT survivor_id FROM _bot_dupe_map
                                                        WHERE loser_id = l.user_id)))
       ) AS keep_group_id
  FROM project_groups l
 WHERE l.user_id IN (SELECT loser_id FROM _bot_dupe_map);

DELETE FROM _bot_group_merge WHERE loser_group_id = keep_group_id;

UPDATE OR IGNORE project_group_items
   SET group_id = (SELECT keep_group_id FROM _bot_group_merge WHERE loser_group_id = group_id)
 WHERE group_id IN (SELECT loser_group_id FROM _bot_group_merge);
DELETE FROM project_group_items
 WHERE group_id IN (SELECT loser_group_id FROM _bot_group_merge);
DELETE FROM project_groups
 WHERE id IN (SELECT loser_group_id FROM _bot_group_merge);

DROP TABLE _bot_group_merge;

-- Every remaining loser group has a name nothing else claims, so this cannot
-- collide.
UPDATE project_groups
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- saved_views UNIQUE (project_id, user_id, name), config TEXT
--
-- Two views can share a name and hold completely different filter configs, so
-- neither may be dropped. The clash is resolved by renaming instead: of the
-- rows that will end up under one owner with one name in one project, the
-- oldest keeps the name and the rest get a suffix. The suffix embeds the row
-- id, which keeps the renamed names distinct from each other, and the column
-- has no length limit (`validate_name` in src/db/queries/views.rs only
-- rejects blank names). The rename set is materialised first so the UPDATE
-- never reads a table it is halfway through rewriting.
CREATE TEMP TABLE _bot_view_rename AS
SELECT v.id AS view_id
  FROM saved_views v
 WHERE (v.user_id IN (SELECT loser_id FROM _bot_dupe_map)
        OR v.user_id IN (SELECT survivor_id FROM _bot_dupe_map))
   AND EXISTS (
        SELECT 1 FROM saved_views o
         WHERE o.project_id = v.project_id
           AND o.name = v.name
           AND o.id < v.id
           AND COALESCE((SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = o.user_id), o.user_id)
             = COALESCE((SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = v.user_id), v.user_id));

UPDATE saved_views
   SET name = name || ' (merged ' || id || ')'
 WHERE id IN (SELECT view_id FROM _bot_view_rename);

DROP TABLE _bot_view_rename;

UPDATE saved_views
   SET user_id = (SELECT survivor_id FROM _bot_dupe_map WHERE loser_id = user_id)
 WHERE user_id IN (SELECT loser_id FROM _bot_dupe_map);

-- Step 3: the losers now have nothing pointing at them.
DELETE FROM users WHERE id IN (SELECT loser_id FROM _bot_dupe_map);

DROP TABLE _bot_dupe_map;

-- Step 4: the constraint itself. Partial, because human users and legacy bots
-- awaiting lazy backfill both carry tool_id NULL and must stay unconstrained.
CREATE UNIQUE INDEX idx_users_owner_tool
    ON users(owner_id, tool_id)
    WHERE is_bot = 1 AND tool_id IS NOT NULL;
