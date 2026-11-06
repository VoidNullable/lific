-- LIF-493: an active plan completes itself when its last open step resolves,
-- and a done plan reopens when one of its steps does.
--
-- These fire on plan_steps, so they cover both a step checked directly and a
-- step checked by the issue cascade (020/061): SQLite runs the triggers of
-- statements made inside a trigger body, it only refuses to re-enter the same
-- trigger. The plan change is audited as `auto-complete` / `auto-reopen` on the
-- plan, attributed to whoever made the change, with the step's issue as the
-- cause when it mirrors one.
--
-- Only active plans complete, and only done plans reopen. Archived plans stay
-- frozen. A plan with no steps never completes on its own.

CREATE TRIGGER plans_step_resolved_auto_complete
AFTER UPDATE OF done ON plan_steps
WHEN NEW.done = 1 AND OLD.done = 0
BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, issue_id, action, field, old_value, new_value)
    SELECT (SELECT user_id FROM _actor_state WHERE id = 1),
           COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
           'plan', pl.id,
           (SELECT identifier FROM projects WHERE id = pl.project_id) || '-PLAN-' || pl.sequence,
           pl.project_id, NEW.issue_id, 'auto-complete', 'status', 'active', 'done'
    FROM plans pl
    WHERE pl.id = NEW.plan_id AND pl.status = 'active'
      AND NOT EXISTS (SELECT 1 FROM plan_steps s WHERE s.plan_id = pl.id AND s.done = 0);

    UPDATE plans
       SET status = 'done', updated_at = datetime('now')
     WHERE id = NEW.plan_id AND status = 'active'
       AND NOT EXISTS (SELECT 1 FROM plan_steps s WHERE s.plan_id = NEW.plan_id AND s.done = 0);
END;

CREATE TRIGGER plans_step_reopened_auto_reopen
AFTER UPDATE OF done ON plan_steps
WHEN NEW.done = 0 AND OLD.done = 1
BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, issue_id, action, field, old_value, new_value)
    SELECT (SELECT user_id FROM _actor_state WHERE id = 1),
           COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
           'plan', pl.id,
           (SELECT identifier FROM projects WHERE id = pl.project_id) || '-PLAN-' || pl.sequence,
           pl.project_id, NEW.issue_id, 'auto-reopen', 'status', 'done', 'active'
    FROM plans pl
    WHERE pl.id = NEW.plan_id AND pl.status = 'done';

    UPDATE plans
       SET status = 'active', updated_at = datetime('now')
     WHERE id = NEW.plan_id AND status = 'done';
END;

-- A done plan still hears its issues reopen, so a plan that completed itself
-- is not stranded at done when the work behind one of its steps regresses.
-- The step unchecks and the trigger above reopens the plan. Completions still
-- reach active plans only. This is 061's reopen cascade with `done` plans
-- added; the anchor archive does not react to reopens, so recreating this
-- trigger does not change which of them runs first.
DROP TRIGGER IF EXISTS plans_issue_reopen_cascade;

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
    WHERE ps.issue_id = NEW.id AND ps.done = 1 AND pl.status IN ('active', 'done');

    UPDATE plan_steps
       SET done = 0, reopened_via_issue_at = datetime('now'), edited_at = datetime('now')
     WHERE issue_id = NEW.id AND done = 1
       AND plan_id IN (SELECT id FROM plans WHERE status IN ('active', 'done'));
END;

-- Backfill: active plans that are already fully resolved.
INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                       project_id, action, field, old_value, new_value)
SELECT NULL, 'system', 'plan', pl.id,
       (SELECT identifier FROM projects WHERE id = pl.project_id) || '-PLAN-' || pl.sequence,
       pl.project_id, 'auto-complete', 'status', 'active', 'done'
FROM plans pl
WHERE pl.status = 'active'
  AND EXISTS (SELECT 1 FROM plan_steps s WHERE s.plan_id = pl.id)
  AND NOT EXISTS (SELECT 1 FROM plan_steps s WHERE s.plan_id = pl.id AND s.done = 0);

UPDATE plans
   SET status = 'done', updated_at = datetime('now')
 WHERE status = 'active'
   AND EXISTS (SELECT 1 FROM plan_steps s WHERE s.plan_id = plans.id)
   AND NOT EXISTS (SELECT 1 FROM plan_steps s WHERE s.plan_id = plans.id AND s.done = 0);
