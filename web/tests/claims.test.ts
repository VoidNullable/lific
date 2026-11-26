import { describe, expect, test } from "bun:test";
import type { IssueClaim } from "../src/lib/api";
import {
  claimHolder,
  claimRemaining,
  claimSession,
  liveClaim,
  serverMs,
} from "../src/lib/issues/claims";

const claim: IssueClaim = {
  user_id: 3,
  username: "maintainer",
  display_name: "Maintainer",
  worker: "p-1a2b3c4d",
  claimed_at: "2026-11-26 12:00:00",
  expires_at: "2026-11-26 12:15:00",
};

const at = (ts: string) => serverMs(ts);

describe("liveClaim", () => {
  test("holds until the expiry, then lapses without a refetch", () => {
    expect(liveClaim(claim, at("2026-11-26 12:14:59"))).toBe(claim);
    expect(liveClaim(claim, at("2026-11-26 12:15:00"))).toBeNull();
  });

  test("no claim is no claim", () => {
    expect(liveClaim(undefined, at("2026-11-26 12:00:00"))).toBeNull();
    expect(liveClaim(null, at("2026-11-26 12:00:00"))).toBeNull();
  });

  test("reads server time as UTC whatever the browser's zone", () => {
    expect(serverMs("2026-11-26 12:15:00")).toBe(Date.UTC(2026, 10, 26, 12, 15, 0));
  });
});

describe("labels", () => {
  test("names the holder by display name, else username", () => {
    expect(claimHolder(claim)).toBe("Maintainer");
    expect(claimHolder({ ...claim, display_name: null })).toBe("maintainer");
  });

  test("time left rounds up to the minute", () => {
    expect(claimRemaining(claim, at("2026-11-26 12:00:00"))).toBe("15m left");
    expect(claimRemaining(claim, at("2026-11-26 12:14:30"))).toBe("1m left");
    const long = { ...claim, expires_at: "2026-11-26 13:20:00" };
    expect(claimRemaining(long, at("2026-11-26 12:00:00"))).toBe("1h 20m left");
    const hours = { ...claim, expires_at: "2026-11-26 14:00:00" };
    expect(claimRemaining(hours, at("2026-11-26 12:00:00"))).toBe("2h left");
  });

  test("says which session holds it", () => {
    expect(claimSession(claim)).toBe("worker p-1a2b3c4d");
    expect(claimSession({ ...claim, worker: undefined })).toBe("any session of the account");
  });
});
