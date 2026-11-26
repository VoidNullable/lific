// LIF-525: a worker's time-limited claim on an issue, as Home and the issue
// views show it.
//
// The server leaves an expired claim out of every read, but a page can sit
// open past the expiry, so each surface checks it again. It checks against
// the SERVER's time, never the browser's: a browser clock that runs a few
// minutes fast would hide a fresh claim, and a slow one would keep showing
// a claim the server has already let go. The server's time is the browser's
// plus the skew measured from `GET /api/clock` (`serverClock.svelte.ts`).
// Until that is known, the server's own answer stands: a claim it sent is
// shown.

import type { IssueClaim } from "../api";
import { personName } from "./assignees";

/** A server timestamp (UTC `YYYY-MM-DD HH:MM:SS`) in epoch milliseconds. */
export function serverMs(ts: string): number {
  return new Date(ts.replace(" ", "T") + "Z").getTime();
}

/** How far the server's clock is ahead of the browser's, from one request:
 *  the server read `serverNowMs` somewhere between `sentMs` and
 *  `receivedMs` on the browser's clock, taken as the midpoint. */
export function clockSkew(serverNowMs: number, sentMs: number, receivedMs: number): number {
  return serverNowMs - (sentMs + receivedMs) / 2;
}

/** The server's time at browser time `browserMs`, or null when the skew is
 *  not known yet. */
export function serverTime(browserMs: number, skewMs: number | null): number | null {
  return skewMs === null ? null : browserMs + skewMs;
}

/** The claim while it lasts at the server's time `serverNowMs`, else null.
 *  A null time (not known yet) trusts the server, which only sends live
 *  claims. */
export function liveClaim(
  claim: IssueClaim | null | undefined,
  serverNowMs: number | null,
): IssueClaim | null {
  if (!claim) return null;
  if (serverNowMs === null) return claim;
  return serverMs(claim.expires_at) > serverNowMs ? claim : null;
}

export function claimHolder(claim: IssueClaim): string {
  return personName(claim);
}

/** Time left at the server's time `serverNowMs`, rounded up to the minute:
 *  "4m left", "1h 20m left". */
export function claimRemaining(claim: IssueClaim, serverNowMs: number): string {
  const minutes = Math.max(1, Math.ceil((serverMs(claim.expires_at) - serverNowMs) / 60_000));
  if (minutes < 60) return `${minutes}m left`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours}h left` : `${hours}h ${rest}m left`;
}

/** Which session holds it: the worker id, or the whole account. */
export function claimSession(claim: IssueClaim): string {
  return claim.worker ? `worker ${claim.worker}` : "any session of the account";
}

/** The full sentence for a tooltip, in the browser's local time. */
export function claimTitle(claim: IssueClaim): string {
  const time = (ts: string) =>
    new Date(serverMs(ts)).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  return (
    `Claimed by @${claim.username} (${claimSession(claim)}) since ${time(claim.claimed_at)}, ` +
    `until ${time(claim.expires_at)} unless renewed`
  );
}
