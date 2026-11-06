-- LIF-485: a step mirroring a cancelled issue counts as resolved.
--
-- 020 only cascaded on `done`, so a step whose issue was cancelled stayed open
-- forever and its plan could never reach 100%. Both terminal statuses now
-- resolve the step.
--
-- Design choice: a cancelled issue checks the step (done = 1) rather than
-- introducing a separate "skipped" step state. Plan progress, list counts and
-- every reader already treat done = 1 as resolved, and the step still carries
-- its issue's status, so readers can tell "done via LIF-42" from
-- "resolved because LIF-42 was cancelled" without a new column. The cascade is
-- audited as `auto-complete`, the same action the done cascade uses.
--
-- Moving between the two terminal statuses (done <-> cancelled) is not a
-- reopen and leaves the steps alone. Leaving both of them reopens the steps.

DROP TRIGGER IF EXISTS plans_issue_done_cascade;
DROP TRIGGER IF EXISTS plans_issue_reopen_cascade;

CREATE TRIGGER plans_issue_done_cascade
AFTER UPDATE OF status ON issues
WHEN NEW.status IN ('done', 'cancelled') AND OLD.status NOT IN ('done', 'cancelled')
BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, issue_id, action, field, old_value, new_value)
    SELECT (SELECT user_id FROM _actor_state WHERE id = 1),
           COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
           'plan_step', ps.id,
           (SELECT identifier FROM projects WHERE id = pl.project_id) || '-PLAN-' || pl.sequence,
           pl.project_id, NEW.id, 'auto-complete', 'done', '0', '1'
    FROM plan_steps ps JOIN plans pl ON pl.id = ps.plan_id
    WHERE ps.issue_id = NEW.id AND ps.done = 0 AND pl.status = 'active';

    UPDATE plan_steps
       SET done = 1, reopened_via_issue_at = NULL, edited_at = datetime('now')
     WHERE issue_id = NEW.id AND done = 0
       AND plan_id IN (SELECT id FROM plans WHERE status = 'active');
END;

CREATE TRIGGER plans_issue_reopen_cascade
AFTER UPDATE OF status ON issues
WHEN OLD.status IN ('done', 'cancelled') AND NEW.status NOT IN ('done', 'cancelled')
BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, issue_id, action, field, old_value, new_value)
    SELECT (SELECT user_id FROM _actor_state WHERE id = 1),
           COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
           'plan_step', ps.id,
           (SELECT identifier FROM projects WHERE id = pl.project_id) || '-PLAN-' || pl.sequence,
           pl.project_id, NEW.id, 'auto-reopen', 'done', '1', '0'
    FROM plan_steps ps JOIN plans pl ON pl.id = ps.plan_id
    WHERE ps.issue_id = NEW.id AND ps.done = 1 AND pl.status = 'active';

    UPDATE plan_steps
       SET done = 0, reopened_via_issue_at = datetime('now'), edited_at = datetime('now')
     WHERE issue_id = NEW.id AND done = 1
       AND plan_id IN (SELECT id FROM plans WHERE status = 'active');
END;

-- Backfill: open steps in active plans that mirror an already-cancelled issue.
-- Audited like the live cascade, attributed to the system.
INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                       project_id, issue_id, action, field, old_value, new_value)
SELECT NULL, 'system', 'plan_step', ps.id,
       (SELECT identifier FROM projects WHERE id = pl.project_id) || '-PLAN-' || pl.sequence,
       pl.project_id, ps.issue_id, 'auto-complete', 'done', '0', '1'
FROM plan_steps ps
JOIN plans pl ON pl.id = ps.plan_id
JOIN issues i ON i.id = ps.issue_id
WHERE ps.done = 0 AND pl.status = 'active' AND i.status = 'cancelled';

UPDATE plan_steps
   SET done = 1, reopened_via_issue_at = NULL, edited_at = datetime('now')
 WHERE done = 0
   AND plan_id IN (SELECT id FROM plans WHERE status = 'active')
   AND issue_id IN (SELECT id FROM issues WHERE status = 'cancelled');

-- SQLite fires triggers on the same event newest first. 020 created the
-- anchor archive after the step cascades, so it ran first and an anchored
-- plan was archived before its steps were touched. Recreating the cascades
-- above made them newer; recreate the archive trigger too, unchanged, so it
-- keeps running first.
DROP TRIGGER IF EXISTS plans_anchor_done_archive;

CREATE TRIGGER plans_anchor_done_archive
AFTER UPDATE OF status ON issues
WHEN NEW.status = 'done' AND OLD.status <> 'done'
BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, issue_id, action, field, old_value, new_value)
    SELECT (SELECT user_id FROM _actor_state WHERE id = 1),
           COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
           'plan', pl.id,
           (SELECT identifier FROM projects WHERE id = pl.project_id) || '-PLAN-' || pl.sequence,
           pl.project_id, NEW.id, 'auto-archive', 'status', 'active', 'archived'
    FROM plans pl
    WHERE pl.issue_id = NEW.id AND pl.status = 'active';

    UPDATE plans
       SET status = 'archived', updated_at = datetime('now')
     WHERE issue_id = NEW.id AND status = 'active';
END;
