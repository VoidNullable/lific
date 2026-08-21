/** Parking a settings patch that was refused for a stale sign-in.
 *
 *  Instance settings autosave per field, and a refusal parks the patch in a
 *  single "what is waiting for confirmation" slot. Anything the save queue
 *  drains afterwards would overwrite that slot, so the prompt would confirm
 *  whichever field was touched *last* and silently drop the earlier one. The
 *  admin sees both controls showing their new values and only one of them
 *  actually saves.
 *
 *  So once a patch is parked, later ones merge into it instead of being sent.
 *  One prompt, one replay, covering everything that was asked for.
 *
 *  Kept here as plain functions so the rule is testable without a DOM, and so
 *  the component is left holding state rather than logic.
 */

/** Merge `next` over `parked`, later keys winning. */
export function mergePendingPatch<P extends object>(parked: P, next: P): P {
  return { ...parked, ...next };
}

/** What a caller should do with a patch, given what is already parked. */
export type PatchDisposition<P> =
  | { action: "send" }
  /** Do not send. Park this instead (already merged with what was there). */
  | { action: "park"; patch: P };

/** Context for one disposition decision. */
export interface PatchContext {
  /** True only for a send issued *by* the confirmation drain, which owns the
   *  snapshot it is carrying and must reach the network. */
  replaying: boolean;
  /** True while a settings confirmation is outstanding: a patch is parked, or
   *  a replay request is in flight. Everything that is not the drain's own
   *  send must park while this holds.
   *
   *  Checking "is something parked" is not enough on its own. The drain clears
   *  the slot before each send, so an edit arriving during that request would
   *  find the slot empty and go straight to the network, racing the replay and
   *  landing in an order nobody chose. */
  hold: boolean;
}

export function disposePatch<P extends object>(
  parked: P | null,
  next: P,
  context: PatchContext,
): PatchDisposition<P> {
  if (context.replaying) return { action: "send" };
  if (context.hold || parked !== null) {
    return { action: "park", patch: parked === null ? next : mergePendingPatch(parked, next) };
  }
  return { action: "send" };
}

/** One step of the confirmation drain.
 *
 * Take the current parked patch and clear the slot, so anything edited while
 * the resulting request is in flight starts a *new* parked patch rather than
 * being folded into one already on the wire, or being lost when the drain
 * finishes.
 */
export function takePending<P extends object>(parked: P | null): {
  taken: P | null;
  remaining: null;
} {
  return { taken: parked, remaining: null };
}

/** The drain's decision after one send.
 *
 * Bounded by construction: each iteration consumes exactly one snapshot, and
 * a failure stops. The only way to loop is for the user to keep editing while
 * their own edits keep succeeding, which is progress, not a spin.
 */
export type DrainStep<P> =
  /** Landed, and more arrived while it was in flight. Send that next. */
  | { next: "continue"; patch: P }
  /** Landed, nothing waiting. */
  | { next: "done" }
  /** Did not land. Whatever is parked stays parked for the caller to show. */
  | { next: "stop" };

export function drainStep<P extends object>(landed: boolean, parked: P | null): DrainStep<P> {
  if (!landed) return { next: "stop" };
  return parked === null ? { next: "done" } : { next: "continue", patch: parked };
}

/** Note for the caller: because a parked patch accumulates every field that
 *  was refused, restoring on cancel must walk the whole merged patch, not just
 *  the last field touched. `InstanceSettings.hydrateFields` does that by
 *  keying off which fields the patch defines. */
