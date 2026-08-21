/** Recovering from a "recent authentication required" refusal.
 *
 *  A handful of actions mint something durable: an API key, a connected tool's
 *  credential, a new account, an admin grant. The server refuses those unless
 *  the browser session was authenticated in the last 15 minutes, because a
 *  session token lifted from a tab that signed in last week must not be enough
 *  to leave lasting access behind.
 *
 *  That refusal is correct and also, on its own, a dead end: the tab is signed
 *  in, the button does nothing, and the message does not say what to do. This
 *  module is the other half. It obtains a fresh session and hands the caller
 *  one retry, and it is deliberately small: no modal framework, no generic
 *  interceptor, no retry loop. Callers own their own UI and call these two
 *  functions.
 */
import { refreshSession, saveSession, type AuthUser, type RequestResult } from "./api";

/** The exact server refusal this module answers. */
export const RECENT_AUTH_ERROR = "recent authentication required";

/** Whether a failed request was refused for staleness specifically.
 *
 *  Matched on status *and* message. A 403 alone is not enough: these endpoints
 *  also answer 403 for "you are not an admin" and "authentication required",
 *  and prompting for a password would be nonsense for either. */
export function needsReauth(result: { ok: false; error: string; status: number | null }): boolean {
  return result.status === 403 && result.error === RECENT_AUTH_ERROR;
}

export type ReauthOutcome =
  | { ok: true }
  /** `recoverable` means the caller still has somewhere to go: the automatic
   *  route failed (auto-login is off, refused, or minted a session for a
   *  *different* admin), and asking for a password is the sensible next step.
   *  It is false only when the human has already tried and been rejected, so
   *  offering the same prompt again would be a loop with extra steps. */
  | { ok: false; error: string; recoverable: boolean };

/** Adopt the refreshed session.
 *
 *  Both routes go through `POST /api/auth/me/refresh`, whose contract is
 *  "same user, newer session": it refuses anything but a live session bearer
 *  belonging to the caller, and it emits no `Set-Cookie` until that has been
 *  established. So an identity swap is not something the client has to detect
 *  after the fact any more.
 *
 *  The comparison stays anyway, as a cheap assertion that the contract held.
 *  It used to be load-bearing, because the only ways to get a fresh session
 *  were `/auth/login` and `/auth/auto-login`, and the latter signs in as the
 *  instance's *first admin*, cookie already set by the time the client could
 *  look. If this ever fires now, the server is not what this client thinks it
 *  is, and refusing is still the right answer. */
function adopt(
  result: RequestResult<{ user: AuthUser; token: string }>,
  expectedUserId: number,
  recoverable: boolean,
): ReauthOutcome {
  if (!result.ok) return { ok: false, error: result.error, recoverable };
  if (result.data.user.id !== expectedUserId) {
    return {
      ok: false,
      error: "That signed in as a different account, so it was not used.",
      recoverable,
    };
  }
  saveSession(result.data.token);
  return { ok: true };
}

/** Re-authenticate with a password the user has just typed.
 *
 *  On success the fresh session is saved and the caller may retry once. On
 *  failure nothing is saved, so the existing session keeps working and the
 *  caller can show the error and let them try again. The username is not a
 *  parameter: the endpoint refreshes whoever the presented session belongs to,
 *  so there is nothing to get wrong. */
export async function reauthenticateWithPassword(
  password: string,
  expectedUserId: number,
): Promise<ReauthOutcome> {
  // Not recoverable: the human just typed a password and it did not work.
  // Re-offering the prompt is the caller's job, with the error visible.
  return adopt(await refreshSession(password), expectedUserId, false);
}

/** Re-authenticate without a password, on an instance that signs in that way
 *  (`web_auto_login`, or `[auth] required = false`).
 *
 *  There is no password to ask for, so the honest thing is to freshen the
 *  session and get on with it rather than show a prompt with nothing to type
 *  into. The server re-reads the mode inside its own transaction, so an admin
 *  who has just turned it off wins and this comes back recoverable. */
export async function reauthenticateWithoutPassword(
  expectedUserId: number,
): Promise<ReauthOutcome> {
  // Recoverable: if this instance turns out to want a password after all (the
  // mode was off, or an admin turned it off since), the caller falls back to
  // asking for one rather than ending the action.
  return adopt(await refreshSession(), expectedUserId, true);
}

/** Run `attempt`; if it is refused for staleness, re-authenticate once and run
 *  it again. Exactly once: a second staleness refusal is returned as-is rather
 *  than retried, so this can never loop.
 *
 *  `reauthenticate` returns the outcome of whichever route the caller has
 *  available (auto-login, or a password the user just typed). */
export async function retryOnceAfterReauth<T>(
  attempt: () => Promise<RequestResult<T>>,
  reauthenticate: () => Promise<ReauthOutcome>,
): Promise<RequestResult<T>> {
  const first = await attempt();
  if (first.ok || !needsReauth(first)) return first;

  const refreshed = await reauthenticate();
  if (!refreshed.ok) {
    // A recoverable failure is reported as the original staleness refusal, so
    // the caller's `needsReauth` branch fires and it shows its password
    // prompt. An unrecoverable one carries its own message through.
    return refreshed.recoverable
      ? { ok: false, error: RECENT_AUTH_ERROR, status: 403 }
      : { ok: false, error: refreshed.error, status: first.status };
  }
  return await attempt();
}
