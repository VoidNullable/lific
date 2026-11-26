// LIF-484 follow-up: the server's calendar day, for deciding whether a date
// wait is holding, due or overdue.
//
// The server evaluates waits on its own local day, so the web client must do
// the same or it will call an issue blocked that REST and MCP call workable.
// `GET /api/clock` reports the server's UTC offset; "today" is then computed
// at that offset from the browser's (UTC-correct) clock, which keeps a tab
// left open across the server's midnight right without a refetch. The offset
// is re-read hourly so a daylight-saving change is picked up.

//
// LIF-525: the same request reports the server's time, and the difference
// from the browser's clock (`skewMs`) decides when a claim ends, so a
// browser whose clock runs fast or slow still agrees with the server.

import { getServerClock } from "./api";
import { dayAtOffset } from "./issues/waits";
import { clockSkew, serverTime } from "./issues/claims";

const REFRESH_MS = 60 * 60_000;

let offsetMinutes = $state<number | null>(null);
let skewMs = $state<number | null>(null);
let fetchedAt = 0;
let inFlight = false;

/** Fetch the offset if it is missing or older than an hour. Safe to call
 *  from any number of `$effect`s: one request at a time. */
export function ensureServerClock(nowMs: number = Date.now()): void {
  if (inFlight || (offsetMinutes !== null && nowMs - fetchedAt < REFRESH_MS)) return;
  inFlight = true;
  const sentMs = Date.now();
  void getServerClock()
    .then((res) => {
      if (res.ok) {
        offsetMinutes = res.data.utc_offset_minutes;
        fetchedAt = Date.now();
        if (typeof res.data.now_ms === "number") {
          skewMs = clockSkew(res.data.now_ms, sentMs, fetchedAt);
        }
      }
    })
    .finally(() => {
      inFlight = false;
    });
}

/** The server's `YYYY-MM-DD` at `nowMs`, or null until its offset is known.
 *  Reactive: reads the offset `$state`. */
export function serverDay(nowMs: number): string | null {
  return offsetMinutes === null ? null : dayAtOffset(nowMs, offsetMinutes);
}

/** The server's time at browser time `nowMs`, or null until it is known.
 *  Reactive: reads the skew `$state`. */
export function serverNow(nowMs: number): number | null {
  return serverTime(nowMs, skewMs);
}
