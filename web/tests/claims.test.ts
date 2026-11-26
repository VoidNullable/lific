import { describe, expect, test } from "bun:test";
import type { IssueClaim } from "../src/lib/api";
import {
  claimHolder,
  claimRemaining,
  claimSession,
  clockSkew,
  liveClaim,
  serverMs,
  serverTime,
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

  test("until the server's time is known, a claim the server sent stands", () => {
    expect(liveClaim(claim, null)).toBe(claim);
  });
});

describe("server time, whatever the browser's clock says", () => {
  const MIN = 60_000;
  // The server reads its clock halfway through a 200 ms round trip.
  const measure = (browserAtServerNoon: number) =>
    clockSkew(at("2026-11-26 12:00:00"), browserAtServerNoon - 100, browserAtServerNoon + 100);

  test("a browser 20 minutes fast still shows a fresh claim", () => {
    const browserNoon = at("2026-11-26 12:00:00") + 20 * MIN;
    const skew = measure(browserNoon);
    expect(skew).toBe(-20 * MIN);
    // Five minutes later on the browser's clock: 12:25 there, 12:05 here.
    const later = browserNoon + 5 * MIN;
    expect(liveClaim(claim, later)).toBeNull(); // what the raw browser clock would say
    const server = serverTime(later, skew)!;
    expect(server).toBe(at("2026-11-26 12:05:00"));
    expect(liveClaim(claim, server)).toBe(claim);
    expect(claimRemaining(claim, server)).toBe("10m left");
  });

  test("a browser 20 minutes slow hides a claim the server has let go", () => {
    const browserNoon = at("2026-11-26 12:00:00") - 20 * MIN;
    const skew = measure(browserNoon);
    expect(skew).toBe(20 * MIN);
    // 12:16 on the server, 11:56 on the browser.
    const later = browserNoon + 16 * MIN;
    expect(liveClaim(claim, later)).toBe(claim); // what the raw browser clock would say
    expect(liveClaim(claim, serverTime(later, skew))).toBeNull();
  });

  test("no skew measured yet is no server time", () => {
    expect(serverTime(at("2026-11-26 12:00:00"), null)).toBeNull();
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
