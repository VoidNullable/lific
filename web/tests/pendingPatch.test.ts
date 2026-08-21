import { describe, expect, test } from "bun:test";
import { createSaveQueue } from "../src/lib/saveQueue";
import {
  disposePatch,
  drainStep,
  mergePendingPatch,
  takePending,
} from "../src/lib/pendingPatch";

type Patch = Record<string, unknown>;

describe("parking a refused settings patch", () => {
  test("with nothing parked, a patch is sent", () => {
    expect(disposePatch(null, { a: 1 }, { replaying: false, hold: false })).toEqual({
      action: "send",
    });
  });

  test("with a patch parked, later ones merge instead of being sent", () => {
    const parked = { allow_signup: true };
    expect(
      disposePatch(parked, { instance_name: "Lific" }, { replaying: false, hold: false }),
    ).toEqual({
      action: "park",
      patch: { allow_signup: true, instance_name: "Lific" },
    });
  });

  test("a later edit to the same field wins", () => {
    expect(mergePendingPatch({ allow_signup: true }, { allow_signup: false })).toEqual({
      allow_signup: false,
    });
  });

  /** The replay issued *by* the drain must reach the network even though a
   *  patch is parked: it owns the snapshot it is carrying. */
  test("the drain's own send is always dispatched", () => {
    expect(
      disposePatch({ a: 1 }, { a: 1, b: 2 }, { replaying: true, hold: true }),
    ).toEqual({ action: "send" });
  });

  /** An edit made while the replay request is in flight must NOT be sent
   *  behind the drain's back, and must not be lost. It parks afresh. */
  test("an edit during the replay parks instead of racing it", () => {
    // The drain clears the slot before each send, so `parked` is null here.
    // Only `hold` can stop this reaching the network.
    expect(disposePatch(null, { d: 4 }, { replaying: false, hold: true })).toEqual({
      action: "park",
      patch: { d: 4 },
    });
    // And a second edit merges with the first.
    expect(disposePatch({ d: 4 }, { e: 5 }, { replaying: false, hold: true })).toEqual({
      action: "park",
      patch: { d: 4, e: 5 },
    });
  });

  /** Cancelling must restore every field the merged patch covers, which is
   *  why the parked patch keeps them all rather than only the last. */
  test("a merged patch names every field that has to be restored on cancel", () => {
    const parked = mergePendingPatch(
      { allow_signup: false },
      { instance_name: "Lific" },
    ) as Record<string, unknown>;
    expect(Object.keys(parked).sort()).toEqual(["allow_signup", "instance_name"]);
  });
});

/** The end-to-end shape: one refusal, two more edits, exactly one merged patch
 *  waiting, no further network calls, and the queue idle with nobody hanging. */
describe("save queue with a parked patch", () => {
  test("a stale refusal plus two queued fields yields one merged patch and no more sends", async () => {
    const sent: Patch[] = [];
    let parked: Patch | null = null;

    const send = async (patch: Patch) => {
      const disposition = disposePatch(parked, patch, { replaying: false, hold: false });
      if (disposition.action === "park") {
        parked = disposition.patch;
        return false;
      }
      sent.push(patch);
      // The first attempt is refused for a stale sign-in and parks itself.
      parked = patch;
      return false;
    };
    const queue = createSaveQueue<Patch>({ send });

    const first = await queue.push({ allow_signup: true });
    const second = await queue.push({ instance_name: "Lific" });
    const third = await queue.push({ session_lifetime_days: 14 });

    expect(first).toBe(false);
    expect(second).toBe(false);
    expect(third).toBe(false);
    // Only the first patch reached the network.
    expect(sent).toEqual([{ allow_signup: true }]);
    expect(parked).toEqual({
      allow_signup: true,
      instance_name: "Lific",
      session_lifetime_days: 14,
    });

    // Nothing is left in flight or waiting: no promise hangs.
    expect(queue.busy).toBe(false);
    expect(queue.pending).toBeNull();

    // The confirmation replays the merged patch, once.
    const replay = disposePatch(parked, parked!, { replaying: true, hold: true });
    expect(replay).toEqual({ action: "send" });
  });
});

describe("the confirmation drain", () => {
  test("stops on a failure, leaving whatever is parked for the prompt", () => {
    expect(drainStep(false, { c: 3 })).toEqual({ next: "stop" });
    expect(drainStep(false, null)).toEqual({ next: "stop" });
  });

  test("finishes when a send lands and nothing arrived meanwhile", () => {
    expect(drainStep(true, null)).toEqual({ next: "done" });
  });

  test("continues with whatever arrived while the send was in flight", () => {
    expect(drainStep(true, { c: 3 })).toEqual({ next: "continue", patch: { c: 3 } });
  });

  test("taking a snapshot clears the slot so new edits start a fresh one", () => {
    expect(takePending({ a: 1 })).toEqual({ taken: { a: 1 }, remaining: null });
    expect(takePending(null)).toEqual({ taken: null, remaining: null });
  });

  /** The scenario in full: A is refused, B merges in, the confirmation sends
   *  A+B, C is edited while that request is in flight, and C goes out on the
   *  drain's second pass. Everything the admin asked for is stored. */
  test("A refused, B merged, C edited mid-replay: all three land", async () => {
    const stored: Record<string, unknown> = {};
    let parked: Record<string, unknown> | null = null;
    const sent: Record<string, unknown>[] = [];

    // One send, honouring the parking rule. `replaying` is the drain's own.
    let hold = false;
    async function send(patch: Record<string, unknown>, replaying: boolean, during?: () => void) {
      const disposition = disposePatch(parked, patch, { replaying, hold });
      if (disposition.action === "park") {
        parked = disposition.patch;
        return false;
      }
      sent.push(patch);
      during?.();
      Object.assign(stored, patch);
      return true;
    }

    // A is refused for a stale sign-in and parks itself.
    parked = { allow_signup: true };
    // B arrives while the confirmation is up: it merges, no network call.
    expect(await send({ instance_name: "Lific" }, false)).toBe(false);
    expect(parked).toEqual({ allow_signup: true, instance_name: "Lific" });

    // The confirmation drains: it takes the snapshot, clears the slot, and
    // holds it for the duration. C is edited while the first request is open.
    let patch = parked!;
    parked = null;
    hold = true;
    let rounds = 0;
    for (;;) {
      rounds += 1;
      const landed = await send(patch, true, () => {
        if (rounds === 1) {
          const disposition = disposePatch(parked, { session_lifetime_days: 14 }, {
            replaying: false,
            hold,
          });
          expect(disposition.action).toBe("park");
          if (disposition.action === "park") parked = disposition.patch;
        }
      });
      const step = drainStep(landed, parked);
      if (step.next === "continue") {
        patch = takePending(parked).taken!;
        parked = null;
        continue;
      }
      expect(step.next).toBe("done");
      break;
    }
    hold = false;

    expect(sent).toEqual([
      { allow_signup: true, instance_name: "Lific" },
      { session_lifetime_days: 14 },
    ]);
    expect(stored).toEqual({
      allow_signup: true,
      instance_name: "Lific",
      session_lifetime_days: 14,
    });
    expect(parked).toBeNull();
  });

  /** If the replay fails, the edit made during it must still be parked, so
   *  cancelling can restore it and retrying can send it. */
  test("a failed replay retains an edit made during it", () => {
    const parkedAfter = { session_lifetime_days: 14 };
    expect(drainStep(false, parkedAfter)).toEqual({ next: "stop" });
    // The caller leaves `parkedAfter` in the slot, so cancel restores that
    // field and a retry sends it.
    expect(Object.keys(parkedAfter)).toEqual(["session_lifetime_days"]);
  });
});
