// LIF-525: a worker's time-limited claim on an issue, as Home and the issue
// views show it.
//
// The server leaves an expired claim out of every read, but a page can sit
// open past the expiry, so each surface checks it again against the clock.

import type { IssueClaim } from "../api";
import { personName } from "./assignees";

/** A server timestamp (UTC `YYYY-MM-DD HH:MM:SS`) in epoch milliseconds. */
export function serverMs(ts: string): number {
  return new Date(ts.replace(" ", "T") + "Z").getTime();
}

/** The claim while it lasts at `nowMs`, else null. */
export function liveClaim(claim: IssueClaim | null | undefined, nowMs: number): IssueClaim | null {
  if (!claim) return null;
  return serverMs(claim.expires_at) > nowMs ? claim : null;
}

export function claimHolder(claim: IssueClaim): string {
  return personName(claim);
}

/** Time left, rounded up to the minute: "4m left", "1h 20m left". */
export function claimRemaining(claim: IssueClaim, nowMs: number): string {
  const minutes = Math.max(1, Math.ceil((serverMs(claim.expires_at) - nowMs) / 60_000));
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
