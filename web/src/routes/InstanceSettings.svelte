<script lang="ts">
  // Instance settings (admin-only): edit the DB-backed, runtime instance
  // settings (LIF-210/211/212/213) and view the member roster. Non-admins who
  // reach the URL directly get a friendly gate.
  import {
    me,
    listUsers,
    getInstance,
    getInstanceSettings,
    updateInstanceSettings,
    createUser,
    promoteUser,
    demoteUser,
    deactivateUser,
    reactivateUser,
    type AuthUser,
    type UserSummary,
    type InstanceSettings,
    type InstanceSettingsPatch,
  } from "../lib/api";
  import {
    RECENT_AUTH_ERROR,
    needsReauth,
    reauthenticateWithoutPassword,
    reauthenticateWithPassword,
    retryOnceAfterReauth,
  } from "../lib/reauth";
  import { createSaveQueue } from "../lib/saveQueue";
  import { disposePatch, drainStep, takePending } from "../lib/pendingPatch";
  import SettingsTabs from "../lib/SettingsTabs.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import TimeAgo from "../lib/TimeAgo.svelte";
  import { ShieldCheck, Lock, SlidersHorizontal, Check, AlertTriangle, DoorOpen, DoorClosed, Users, ShieldPlus, ShieldMinus, UserMinus, UserPlus, RotateCcw } from "lucide-svelte";
  import { getContext, onMount } from "svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");
  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  const host = window.location.host;
  let user = $state<AuthUser | null>(null);
  let users = $state<UserSummary[]>([]);
  let settings = $state<InstanceSettings | null>(null);
  let loading = $state(true);

  // Editable copies.
  let fName = $state("");
  let fSignups = $state(true);
  let fDomains = $state("");
  let fSession = $state(30);
  let fMessage = $state("");
  let fAutoLogin = $state(false);
  let fAuthzEnforced = $state(false);

  let saving = $state(false);
  let saveError = $state("");
  let savedAt = $state(0);

  function hydrate(s: InstanceSettings) {
    settings = s;
    fName = s.instance_name ?? "";
    fSignups = s.allow_signup;
    fDomains = s.signup_email_domains.join(", ");
    fSession = s.session_lifetime_days;
    fMessage = s.login_message ?? "";
    fAutoLogin = s.web_auto_login;
    fAuthzEnforced = s.authz_enforced;
  }

  onMount(async () => {
    const meRes = await me();
    if (meRes.ok) user = meRes.data;
    if (user?.is_admin) {
      // `/api/instance` reports the *effective* sign-in mode: it is true both
      // for `web_auto_login` and for `[auth] required = false`, which the
      // stored settings row does not capture on its own.
      const [u, s, instance] = await Promise.all([
        listUsers(),
        getInstanceSettings(),
        getInstance(),
      ]);
      if (u.ok) users = u.data;
      if (s.ok) hydrate(s.data);
      if (instance.ok) webAutoLogin = instance.data.web_auto_login;
    }
    loading = false;
  });

  function parseDomains(csv: string): string[] {
    return csv.split(/[,\s]+/).map((d) => d.trim()).filter(Boolean);
  }

  // ── Field-level autosave (no Save button) ───────────────
  // Mirrors ProjectSettings: every control commits its own field — text inputs
  // on blur, toggles on click. We re-sync only the fields named in the patch
  // from the normalized server response, so an in-progress edit in a different
  // field is never clobbered.
  /** Re-read every editable control from the authoritative settings row.
   *  Used after a save lands (so the displayed values are the stored ones) and
   *  after one fails (so an optimistic toggle does not sit there claiming a
   *  change the server refused). */
  function hydrateFields(from: InstanceSettings, patch: InstanceSettingsPatch) {
    if (patch.instance_name !== undefined) fName = from.instance_name ?? "";
    if (patch.signup_email_domains !== undefined)
      fDomains = from.signup_email_domains.join(", ");
    if (patch.session_lifetime_days !== undefined) fSession = from.session_lifetime_days;
    if (patch.login_message !== undefined) fMessage = from.login_message ?? "";
    if (patch.allow_signup !== undefined) fSignups = from.allow_signup;
    if (patch.web_auto_login !== undefined) fAutoLogin = from.web_auto_login;
    if (patch.authz_enforced !== undefined) fAuthzEnforced = from.authz_enforced;
  }

  /** One save. `recover` is false once the password prompt has already
   *  refreshed the session, so the retry is genuinely the last try. */
  async function sendPatch(patch: InstanceSettingsPatch, replaying = false): Promise<boolean> {
    // `reauthFor` is a single slot, and a settings refusal parks a patch in
    // it. Anything sent afterwards would overwrite that slot and strand the
    // first patch: the prompt would confirm the *last* field touched and
    // silently drop the earlier one. So while a confirmation is outstanding,
    // every patch merges into the parked one and goes nowhere near the
    // network. That includes edits made *during* the replay request, which the
    // drain below picks up on its next pass rather than losing.
    //
    // `replaying` is true only for the drain's own sends, which own the
    // snapshot they carry.
    const parked = parkedSettingsPatch();
    const disposition = disposePatch(parked, patch, {
      replaying,
      hold: settingsReplayInFlight,
    });
    if (disposition.action === "park") {
      reauthFor = { kind: "settings", patch: disposition.patch };
      return false;
    }
    saveError = "";
    const res = await retryOnceAfterReauth(() => updateInstanceSettings(patch), () =>
      replaying
        ? Promise.resolve({ ok: false as const, error: RECENT_AUTH_ERROR, recoverable: false })
        : autoReauth(),
    );
    if (res.ok) {
      settings = res.data;
      hydrateFields(res.data, patch);
      savedAt = Date.now();
      window.setTimeout(() => { if (Date.now() - savedAt >= 1900) savedAt = 0; }, 2000);
      return true;
    }
    if (needsReauth(res)) {
      // Hold the exact patch. The control keeps showing the requested value so
      // the intent stays visible while the prompt is up; cancelling restores it
      // from `settings`.
      reauthFor = { kind: "settings", patch };
      return false;
    }
    saveError = res.error;
    // An ordinary refusal: put the controls back to what is actually stored,
    // rather than leaving a toggle asserting a change that did not happen.
    if (settings) hydrateFields(settings, patch);
    return false;
  }

  // Serialized and coalescing. `if (saving) return` used to *drop* a second
  // edit made during an in-flight save, with no error anywhere.
  const saveQueue = createSaveQueue<InstanceSettingsPatch>({
    send: (patch) => sendPatch(patch),
    onStateChange: (state) => { saving = state === "sending"; },
  });

  function commit(patch: InstanceSettingsPatch): Promise<boolean> {
    return saveQueue.push(patch);
  }

  /** Send everything the confirmation is holding, one snapshot at a time.
   *
   *  Each pass takes the parked patch and clears the slot, so edits arriving
   *  while a request is in flight start a fresh parked patch instead of being
   *  folded into one already on the wire or dropped when the drain ends. A
   *  failure stops immediately, leaving whatever has accumulated parked for
   *  the prompt to show. Bounded: every pass consumes exactly one snapshot,
   *  and the only way to keep going is for edits to keep landing. */
  /** The settings patch currently waiting for confirmation, if any. */
  function parkedSettingsPatch(): InstanceSettingsPatch | null {
    return reauthFor?.kind === "settings" ? reauthFor.patch : null;
  }

  async function drainParkedSettings(first: InstanceSettingsPatch): Promise<boolean> {
    settingsReplayInFlight = true;
    // The drain owns `first` now, so clear the slot: an edit made during the
    // request below starts a fresh parked patch instead of being merged into
    // one already on the wire.
    reauthFor = null;
    let patch = first;
    try {
      for (;;) {
        const landed = await sendPatch(patch, true);
        const parked = parkedSettingsPatch();
        const step = drainStep(landed, parked);
        if (step.next === "continue") {
          const { taken } = takePending(parked);
          reauthFor = null;
          patch = step.patch;
          void taken;
          continue;
        }
        // "done" leaves the slot already null. "stop" leaves whatever
        // `sendPatch` parked: a second staleness refusal re-parks the exact
        // patch, and an ordinary failure parks nothing and shows `saveError`.
        return step.next === "done";
      }
    } finally {
      settingsReplayInFlight = false;
    }
  }

  // Per-field commits: only write when the value actually changed, so a blur
  // with no edit (or re-clicking the already-active toggle) is a no-op.
  function commitName() {
    if (settings && fName.trim() !== (settings.instance_name ?? ""))
      commit({ instance_name: fName.trim() });
  }
  function commitDomains() {
    if (settings && parseDomains(fDomains).join(",") !== settings.signup_email_domains.join(","))
      commit({ signup_email_domains: parseDomains(fDomains) });
  }
  function commitSession() {
    // Guard against an emptied number input (NaN/null) — the server would
    // treat it as "no change" anyway, but don't bother round-tripping it.
    if (settings && Number.isFinite(fSession) && fSession !== settings.session_lifetime_days)
      commit({ session_lifetime_days: fSession });
  }
  function commitMessage() {
    if (settings && fMessage.trim() !== (settings.login_message ?? ""))
      commit({ login_message: fMessage.trim() });
  }
  function setSignups(v: boolean) {
    if (settings && v !== fSignups) {
      fSignups = v;
      commit({ allow_signup: v });
    }
  }
  function setAutoLogin(v: boolean) {
    if (settings && v !== fAutoLogin) {
      fAutoLogin = v;
      commit({ web_auto_login: v });
    }
  }
  function setAuthzEnforced(v: boolean) {
    if (settings && v !== fAuthzEnforced) {
      fAuthzEnforced = v;
      commit({ authz_enforced: v });
    }
  }

  function initials(name: string): string {
    return name.split(/[\s_-]+/).slice(0, 2).map((w) => w[0]?.toUpperCase() ?? "").join("");
  }

  const adminCount = $derived(users.filter((u) => u.is_admin && u.is_active).length);

  // ── Member management (LIF-214) ─────────────────────────
  // The instance-admin axis: who administers this instance, and whose account
  // still works. Every action here is admin-only and guard-railed on the
  // server (the last admin can't be demoted or deactivated, and none of it
  // can be pointed at a bot), so a refusal comes back as a message worth
  // showing on the row rather than something to pre-empt in the client.

  // Destructive actions confirm in place, same shape as ProjectMembers'
  // remove: the button swaps for a red confirm plus Cancel.
  type PendingAction = { id: number; kind: "demote" | "deactivate" };
  let pending = $state<PendingAction | null>(null);
  let rowBusy = $state<number | null>(null);
  let rowError = $state<{ id: number; message: string } | null>(null);

  function applyUser(updated: UserSummary) {
    users = users.map((u) => (u.id === updated.id ? updated : u));
  }

  // ── Re-authentication for the two granting actions ───────
  //
  // Creating an account and promoting one to admin both leave lasting access
  // behind, so the server requires a sign-in from the last 15 minutes. Demote,
  // deactivate and reactivate only ever take access away, are not gated, and
  // deliberately do not go through any of this: they are what an admin reaches
  // for while containing a compromise.
  //
  // One prompt serves both, and it remembers which operation was interrupted
  // so the retry resumes exactly that one with its inputs intact.
  // Every pending operation carries its own arguments. Nothing here reads a
  // reactive field at retry time: the form may have been typed into, the
  // roster may have reloaded, and the retry must run what was actually
  // refused, not what happens to be on screen a minute later.
  type ReauthTarget =
    | { kind: "create"; username: string; password: string }
    | { kind: "promote"; userId: number; username: string }
    | { kind: "reactivate"; userId: number; username: string }
    | { kind: "settings"; patch: InstanceSettingsPatch };
  let reauthFor = $state<ReauthTarget | null>(null);
  let reauthPassword = $state("");
  let reauthBusy = $state(false);
  let reauthError = $state("");

  /** Why the automatic sign-in did not work, when one was attempted. */
  let autoReauthNote = $state("");
  /** True for the whole confirmation drain, including between its sends. The
   *  drain clears the parked slot before each send, so "is something parked"
   *  cannot answer "is a replay in flight". */
  let settingsReplayInFlight = $state(false);
  /** Effective passwordless sign-in, from the public instance payload. */
  let webAutoLogin = $state(false);

  /** The automatic way out of a staleness refusal, where one exists: a
   *  passwordless instance can mint a fresh session without asking anything.
   *  Anywhere else, and on any failure (auto-login off, refused, or a session
   *  belonging to a different admin), this declines as *recoverable* and the
   *  caller shows the password prompt instead of ending the action. */
  async function autoReauth() {
    // The *effective* mode, from the public `/api/instance` payload. The
    // stored `settings.web_auto_login` flag is not the whole answer: an
    // instance running `[auth] required = false` also signs in without a
    // password, and reading the raw flag there would put an impossible
    // password prompt in front of an operator who has no password to type.
    if (!webAutoLogin || !user) {
      return { ok: false as const, error: RECENT_AUTH_ERROR, recoverable: true };
    }
    const auto = await reauthenticateWithoutPassword(user.id);
    if (!auto.ok) autoReauthNote = auto.error;
    return auto;
  }

  /** Promote, handling a staleness refusal. Returns true when it landed.
   *  `recover` is false once the password prompt has already refreshed the
   *  session, so the retry is genuinely the last attempt. */
  async function attemptPromote(
    userId: number,
    username: string,
    recover = true,
  ): Promise<boolean> {
    const res = await retryOnceAfterReauth(() => promoteUser(userId), () =>
      recover
        ? autoReauth()
        : Promise.resolve({ ok: false as const, error: RECENT_AUTH_ERROR, recoverable: false }),
    );
    if (res.ok) {
      applyUser(res.data);
      return true;
    }
    if (needsReauth(res)) {
      reauthFor = { kind: "promote", userId, username };
      return false;
    }
    rowError = { id: userId, message: res.error };
    return false;
  }

  async function runAction(
    u: UserSummary,
    action: (id: number) => ReturnType<typeof promoteUser>,
  ) {
    if (rowBusy !== null) return;
    rowBusy = u.id;
    rowError = null;
    const res = await action(u.id);
    rowBusy = null;
    pending = null;
    if (res.ok) applyUser(res.data);
    else rowError = { id: u.id, message: res.error };
  }

  /** Restore a deactivated account. An expansion, so same rule as promote. */
  async function attemptReactivate(
    userId: number,
    username: string,
    recover = true,
  ): Promise<boolean> {
    const res = await retryOnceAfterReauth(() => reactivateUser(userId), () =>
      recover
        ? autoReauth()
        : Promise.resolve({ ok: false as const, error: RECENT_AUTH_ERROR, recoverable: false }),
    );
    if (res.ok) {
      applyUser(res.data);
      return true;
    }
    if (needsReauth(res)) {
      reauthFor = { kind: "reactivate", userId, username };
      return false;
    }
    rowError = { id: userId, message: res.error };
    return false;
  }

  /** The two granting row buttons. Same shape as `runAction`, plus recovery. */
  async function runGrant(
    u: UserSummary,
    attempt: (userId: number, username: string) => Promise<boolean>,
  ) {
    if (
      rowBusy !== null ||
      creating ||
      saving ||
      settingsReplayInFlight ||
      reauthFor !== null ||
      reauthBusy
    ) return;
    rowBusy = u.id;
    rowError = null;
    reauthError = "";
    await attempt(u.id, u.username);
    rowBusy = null;
    pending = null;
  }

  /** Verify the admin's own password, then resume whatever was interrupted,
   *  exactly once. The pending operation's inputs are untouched throughout,
   *  so a wrong password costs a retype of the password and nothing else. */
  async function submitReauth() {
    const target = reauthFor;
    if (!target || !user || reauthBusy || !reauthPassword) return;
    reauthBusy = true;
    reauthError = "";
    const refreshed = await reauthenticateWithPassword(reauthPassword, user.id);
    if (!refreshed.ok) {
      reauthError = refreshed.error;
      reauthBusy = false;
      return;
    }
    reauthPassword = "";
    autoReauthNote = "";
    // The session is fresh now, so these are the one retry; no further
    // recovery is offered, and a refusal here is a real failure.
    // Take the settings snapshot *now*: `target` was captured before the
    // password round-trip, and more may have merged into the slot since.
    const settingsPatch = parkedSettingsPatch();
    const landed = await (target.kind === "create"
      ? attemptCreate(target.username, target.password, false)
      : target.kind === "promote"
        ? attemptPromote(target.userId, target.username, false)
        : target.kind === "reactivate"
          ? attemptReactivate(target.userId, target.username, false)
          : settingsPatch
            ? drainParkedSettings(settingsPatch)
            : Promise.resolve(true));
    // `drainParkedSettings` owns the slot while it runs; the others clear it
    // here on success.
    if (landed && target.kind !== "settings") reauthFor = null;
    // Only worth saying if the prompt is still on screen. An ordinary failure
    // clears the prompt and reports itself through `saveError`/`rowError`.
    if (!landed && !reauthError && reauthFor) {
      reauthError = "That still was not accepted. Sign out and sign back in, then try again.";
    }
    reauthBusy = false;
  }

  function cancelReauth() {
    // An abandoned settings change must not leave its control asserting a
    // value the server never stored.
    if (reauthFor?.kind === "settings" && settings) hydrateFields(settings, reauthFor.patch);
    reauthFor = null;
    reauthPassword = "";
    reauthError = "";
    autoReauthNote = "";
  }

  // ── Create a member ─────────────────────────────────────
  // Deliberately minimal: a username and a password is everything the server
  // needs (it fills in a {username}@local address), and this is an admin
  // handing someone their first credential, not a profile editor.
  let newUsername = $state("");
  let newPassword = $state("");
  let creating = $state(false);
  let createError = $state("");
  let createdName = $state("");

  /** Create the account, handling a staleness refusal. On refusal the form
   *  keeps its values, so the retry after re-authenticating creates exactly
   *  the account that was typed. */
  async function attemptCreate(
    username: string,
    password: string,
    recover = true,
  ): Promise<boolean> {
    const res = await retryOnceAfterReauth(
      () => createUser({ username, password }),
      () =>
        recover
          ? autoReauth()
          : Promise.resolve({ ok: false as const, error: RECENT_AUTH_ERROR, recoverable: false }),
    );
    if (res.ok) {
      users = [...users, res.data];
      createdName = res.data.username;
      newUsername = "";
      newPassword = "";
      return true;
    }
    if (needsReauth(res)) {
      // The password is held here, not read back off the form: the field is
      // cleared as soon as the account lands, and an admin may well retype
      // something else while the prompt is up.
      reauthFor = { kind: "create", username, password };
      return false;
    }
    createError = res.error;
    return false;
  }

  async function submitCreate() {
    const username = newUsername.trim();
    if (
      !username ||
      !newPassword ||
      creating ||
      rowBusy !== null ||
      saving ||
      settingsReplayInFlight ||
      reauthFor !== null ||
      reauthBusy
    ) return;
    creating = true;
    createError = "";
    createdName = "";
    reauthError = "";
    await attemptCreate(username, newPassword);
    creating = false;
  }
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <span class="text-body-sm font-medium text-[var(--text)]">Settings</span>
  </div>
{/snippet}

<div class="flex-1 overflow-y-auto">
  <div class="w-full max-w-[1000px] mx-auto px-6 py-10 md:py-12">
    {#if loading}
      <!-- LIF-281: structural skeleton replacing the bare centered spinner.
           The tab bar (SettingsTabs: border-b + mb-8) always renders in both
           the admin and non-admin loaded branches, so it's mirrored first to
           pin the frame. Below it we mirror the admin path's default-visible
           layout — the "Instance" heading block and the settings form column
           (single max-w-[560px] column of labeled fields) — which is the
           expected case for this admin-only route. A non-admin instead sees
           the "Admins only" gate on load; that branch is intentionally not
           mirrored (unpredictable until `me()` resolves), but the tab bar
           keeps the top of the frame from shifting. -->
      <!-- Tab bar stand-in -->
      <div class="flex items-center gap-6 border-b border-[var(--border)] mb-8">
        <Skeleton variant="bar" class="h-4 w-16 mb-2.5 mt-1" />
        <Skeleton variant="bar" class="h-4 w-16 mb-2.5 mt-1" />
      </div>

      <!-- Heading block -->
      <section class="mb-8">
        <Skeleton variant="bar" class="h-7 w-40 mb-2" />
        <Skeleton variant="bar" class="h-4 w-full max-w-[52ch]" />
      </section>

      <!-- Settings form card -->
      <section class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] p-5">
        <div class="flex items-center gap-2 mb-5">
          <Skeleton variant="circle" class="size-[15px] rounded" />
          <Skeleton variant="bar" class="h-4 w-24" />
        </div>
        <div class="flex flex-col gap-6 max-w-[560px]">
          {#each [0, 1, 2] as field (field)}
            <div class="flex flex-col gap-1.5">
              <Skeleton variant="bar" class="h-3 w-32" />
              <Skeleton variant="block" class="h-10 w-full rounded-md" />
              <Skeleton variant="bar" class="h-3 w-56" />
            </div>
          {/each}
        </div>
      </section>
    {:else}
      <SettingsTabs active="instance" isAdmin={user?.is_admin ?? false} {navigate} />

      {#if !user?.is_admin}
        <div class="flex flex-col items-center text-center py-20 animate-reveal">
          <div class="size-12 rounded-full bg-[var(--bg-subtle)] grid place-items-center mb-4">
            <Lock size={20} class="text-[var(--text-faint)]" />
          </div>
          <h2 class="text-[1rem] font-semibold text-[var(--text)]">Admins only</h2>
          <p class="text-body text-[var(--text-muted)] mt-1 max-w-[36ch]">
            Instance settings are visible to administrators of this instance.
          </p>
          <button
            class="mt-5 text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                   px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
            onclick={() => navigate("/settings")}
          >
            Back to account
          </button>
        </div>
      {:else}
        <section class="mb-8 animate-reveal delay-100">
          <h1 class="font-display text-title tracking-tight text-[var(--text)] leading-none">Instance</h1>
          <p class="text-body text-[var(--text-muted)] mt-2">
            Settings for the Lific instance at <span class="font-mono text-[var(--text)]">{host}</span>.
            Changes apply immediately.
          </p>
        </section>

        <!-- ── SETTINGS FORM ──────────────────────────────── -->
        <fieldset
          disabled={
            creating ||
            rowBusy !== null ||
            (reauthFor !== null && reauthFor.kind !== "settings") ||
            reauthBusy
          }
          class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] p-5 animate-reveal delay-250"
        >
          <div class="flex items-center gap-2 mb-5">
            <SlidersHorizontal size={15} class="text-[var(--text-muted)]" />
            <h2 class="text-body-lg font-semibold text-[var(--text)]">Settings</h2>
            <span class="font-mono text-micro text-[var(--text-faint)] px-1.5 py-0.5 rounded bg-[var(--bg-subtle)]">v{__APP_VERSION__}</span>
          </div>

          <div class="flex flex-col gap-6 max-w-[560px]">
            <!-- Name -->
            <label class="block">
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Instance name</span>
              <input
                bind:value={fName}
                onblur={commitName}
                placeholder={host}
                maxlength="60"
                class="w-full px-3 py-2 text-body rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                       outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              />
              <span class="block text-caption text-[var(--text)] mt-1.5">Shown on the sign-in screen. Leave blank to use the host.</span>
            </label>

            <!-- Signups: a real status, so each state carries its own color
                 (green = open/permissive, amber = gated) + an icon. -->
            <div>
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Sign-ups</span>
              <div class="inline-flex gap-1 p-1 rounded-xl bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]">
                <button
                  type="button"
                  aria-pressed={fSignups}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {fSignups
                    ? 'bg-[var(--success-bg)] text-[var(--success)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[color-mix(in_oklab,var(--success)_38%,transparent)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setSignups(true)}
                >
                  <DoorOpen size={16} class="shrink-0" />
                  Open
                </button>
                <button
                  type="button"
                  aria-pressed={!fSignups}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {!fSignups
                    ? 'bg-[color-mix(in_oklab,var(--warn)_15%,var(--bg))] text-[var(--warn-text)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[color-mix(in_oklab,var(--warn)_38%,transparent)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setSignups(false)}
                >
                  <DoorClosed size={16} class="shrink-0" />
                  Closed
                </button>
              </div>
              <span class="block text-caption text-[var(--text)] mt-2 leading-relaxed">
                {#if fSignups}
                  Anyone can create their own account{parseDomains(fDomains).length ? " from an allowed domain" : ""}.
                {:else}
                  New accounts are created by an admin only. The sign-in screen shows a closed notice.
                {/if}
              </span>
            </div>

            <!-- Email domain allowlist -->
            <label class="block">
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Allowed signup domains</span>
              <input
                bind:value={fDomains}
                onblur={commitDomains}
                placeholder="snake.com, sub.snake.com"
                class="w-full px-3 py-2 text-body font-mono rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                       outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              />
              <span class="block text-caption text-[var(--text)] mt-1.5">Comma-separated. Leave blank to allow any email domain.</span>
            </label>

            <!-- Session lifetime -->
            <label class="block">
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Session lifetime</span>
              <div class="flex items-center gap-2">
                <input
                  type="number"
                  bind:value={fSession}
                  onblur={commitSession}
                  min="1"
                  max="365"
                  class="w-24 px-3 py-2 text-body rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                         outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
                />
                <span class="text-body text-[var(--text)]">days</span>
              </div>
              <span class="block text-caption text-[var(--text)] mt-1.5">How long a sign-in stays valid before re-authenticating (1 to 365).</span>
            </label>

            <!-- Login message -->
            <label class="block">
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Login message</span>
              <textarea
                bind:value={fMessage}
                onblur={commitMessage}
                rows="2"
                maxlength="280"
                placeholder="Lific Issue tracker. Ask I.T. for access"
                class="w-full px-3 py-2 text-body rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                       outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)] resize-none"
              ></textarea>
              <span class="block text-caption text-[var(--text)] mt-1.5">A short note shown on the sign-in screen. Leave blank for none.</span>
            </label>

            <!-- Single-user mode (LIF-215): auto-sign-in the web UI as the
                 admin. A real auth bypass, so it's set apart with a divider and
                 a stronger (--text) section label, carries a loud warning when
                 on, and is scoped to the browser (REST/MCP still need tokens).
                 The danger is signalled by the divider + amber toggle/warning
                 box, NOT by tinting this 11px label amber — orange-600 is only
                 ~3.4:1 on the light surface and would fail AA. -->
            <div class="pt-6 mt-1 border-t border-[var(--border)]">
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Single-user mode</span>
              <div class="inline-flex gap-1 p-1 rounded-xl bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]">
                <button
                  type="button"
                  aria-pressed={!fAutoLogin}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {!fAutoLogin
                    ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[var(--border)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setAutoLogin(false)}
                >
                  <DoorClosed size={16} class="shrink-0" />
                  Require sign-in
                </button>
                <button
                  type="button"
                  aria-pressed={fAutoLogin}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {fAutoLogin
                    ? 'bg-[color-mix(in_oklab,var(--warn)_15%,var(--bg))] text-[var(--warn-text)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[color-mix(in_oklab,var(--warn)_38%,transparent)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setAutoLogin(true)}
                >
                  <DoorOpen size={16} class="shrink-0" />
                  Skip web sign-in
                </button>
              </div>
              <span class="block text-caption text-[var(--text)] mt-2 leading-relaxed">
                {#if fAutoLogin}
                  The web UI signs in as the admin automatically — no login screen.
                {:else}
                  Everyone signs in with their account as normal.
                {/if}
              </span>
              {#if fAutoLogin}
                <div class="flex items-start gap-2 text-caption text-[var(--warn-text)] bg-[color-mix(in_oklab,var(--warn)_12%,var(--bg))] px-3 py-2 rounded-lg mt-2 max-w-[42ch]">
                  <AlertTriangle size={13} class="shrink-0 mt-0.5" />
                  <span>Anyone who can reach this site becomes admin without a password. Only enable on a private or local instance. REST and MCP are unaffected.</span>
                </div>
              {/if}
            </div>

            <!-- Project-scoped permissions (LIF-194/LIF-197): default-deny
                 membership enforcement. Same divider + amber-when-on
                 treatment as single-user mode above, since flipping this ON
                 can just as suddenly lock people out of a project. -->
            <div class="pt-6 mt-1 border-t border-[var(--border)]">
              <span class="flex items-center gap-1.5 text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">
                <Users size={12} />
                Project permissions
              </span>
              <div class="inline-flex gap-1 p-1 rounded-xl bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]">
                <button
                  type="button"
                  aria-pressed={!fAuthzEnforced}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {!fAuthzEnforced
                    ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[var(--border)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setAuthzEnforced(false)}
                >
                  <DoorOpen size={16} class="shrink-0" />
                  Off
                </button>
                <button
                  type="button"
                  aria-pressed={fAuthzEnforced}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {fAuthzEnforced
                    ? 'bg-[color-mix(in_oklab,var(--warn)_15%,var(--bg))] text-[var(--warn-text)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[color-mix(in_oklab,var(--warn)_38%,transparent)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setAuthzEnforced(true)}
                >
                  <DoorClosed size={16} class="shrink-0" />
                  Enforced
                </button>
              </div>
              <span class="block text-caption text-[var(--text)] mt-2 leading-relaxed max-w-[42ch]">
                When on, only project members can see or edit a project. Add yourself as lead to your projects before enabling.
              </span>
              {#if fAuthzEnforced}
                <div class="flex items-start gap-2 text-caption text-[var(--warn-text)] bg-[color-mix(in_oklab,var(--warn)_12%,var(--bg))] px-3 py-2 rounded-lg mt-2 max-w-[42ch]">
                  <AlertTriangle size={13} class="shrink-0 mt-0.5" />
                  <span>Anyone not added as a project member (via that project's Settings → Members) loses access to it immediately, including you if you aren't a lead yet.</span>
                </div>
              {/if}
            </div>
          </div>

          {#if reauthFor?.kind === "settings"}
            <p class="text-caption text-[var(--text-muted)] mt-4 flex items-center gap-1" role="status">
              <Lock size={12} /> That change needs a recent sign-in. Confirm your password under
              <strong>Members</strong> below and it will be saved.
            </p>
          {/if}
          {#if saveError}
            <p class="text-caption text-[var(--error)] mt-4 flex items-center gap-1" role="alert"><AlertTriangle size={12} /> {saveError}</p>
          {/if}

          <!-- Autosave status (no Save button — each field commits on change). -->
          <div class="flex items-center gap-2 mt-5 h-5 text-body-sm" aria-live="polite">
            {#if saving}
              <span class="inline-flex items-center gap-1.5 text-[var(--text-muted)]">
                <span class="size-3 rounded-full border-2 border-[var(--border)] border-t-[var(--accent)] animate-spin"></span>
                Saving…
              </span>
            {:else if savedAt}
              <span class="inline-flex items-center gap-1 text-[var(--success)]"><Check size={13} /> Saved</span>
            {:else if !saveError}
              <span class="text-[var(--text-muted)]">Changes save automatically.</span>
            {/if}
          </div>
        </fieldset>

        <!-- ── MEMBERS ────────────────────────────────────── -->
        <section class="mt-10 animate-reveal delay-250">
          <div class="flex items-center gap-2 mb-1">
            <ShieldCheck size={16} class="text-[var(--text-muted)]" />
            <h2 class="text-[1rem] font-semibold text-[var(--text)]">Members</h2>
          </div>
          <p class="text-body text-[var(--text-muted)] mb-5 leading-relaxed">
            {users.length} {users.length === 1 ? "person" : "people"} on this instance · {adminCount} admin.
          </p>

          <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] overflow-hidden">
            <!-- Create a member: username + password is all the server needs. -->
            <div class="flex items-center gap-2 px-4 py-3 border-b border-[var(--border)] flex-wrap">
              <input
                bind:value={newUsername}
                disabled={reauthFor !== null || reauthBusy}
                placeholder="username"
                autocomplete="off"
                class="flex-1 min-w-[150px] px-3 py-1.5 text-body-sm font-mono rounded-md border border-[var(--border)]
                       bg-[var(--bg)] text-[var(--text)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              />
              <input
                bind:value={newPassword}
                disabled={reauthFor !== null || reauthBusy}
                type="password"
                placeholder="password"
                autocomplete="new-password"
                class="flex-1 min-w-[150px] px-3 py-1.5 text-body-sm rounded-md border border-[var(--border)]
                       bg-[var(--bg)] text-[var(--text)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              />
              <button
                class="flex items-center gap-1.5 text-body-sm font-medium text-[var(--btn-success-text)]
                       bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)]
                       transition-colors disabled:opacity-40 disabled:cursor-not-allowed shrink-0"
                disabled={creating || rowBusy !== null || saving || settingsReplayInFlight || !newUsername.trim() || !newPassword || reauthFor !== null || reauthBusy}
                onclick={submitCreate}
              >
                <UserPlus size={14} />
                {creating ? "Creating…" : "Create"}
              </button>
            </div>
            {#if createError}
              <div class="px-4 py-2 text-caption text-[var(--error)] bg-[var(--error-bg)]" role="alert">{createError}</div>
            {:else if createdName}
              <div class="px-4 py-2 text-caption text-[var(--success)]" role="status">
                Created <span class="font-mono">@{createdName}</span>. Share the password with them; they can change it in their settings.
              </div>
            {/if}

            <!-- One prompt for both granting actions. It appears only when the
                 server refused for staleness, names which action is waiting,
                 and leaves that action's inputs alone so confirming resumes it
                 unchanged. -->
            {#if reauthFor}
              <div class="px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-subtle)] flex flex-col gap-2">
                <div class="flex items-start gap-2 text-caption text-[var(--text)]">
                  <Lock size={13} class="shrink-0 mt-0.5 text-[var(--text-muted)]" />
                  <p class="leading-relaxed">
                    Verify it's you to
                    {#if reauthFor.kind === "create"}
                      create <span class="font-mono">@{reauthFor.username}</span>.
                    {:else if reauthFor.kind === "promote"}
                      make <span class="font-mono">@{reauthFor.username}</span> an admin.
                    {:else if reauthFor.kind === "reactivate"}
                      restore <span class="font-mono">@{reauthFor.username}</span>.
                    {:else}
                      save that instance setting.
                    {/if}
                    Expanding access needs a recent sign-in, and you have been signed in for a while.
                  </p>
                </div>
                {#if autoReauthNote}
                  <p class="text-caption text-[var(--text-muted)]" role="status">
                    Signing you in automatically did not work ({autoReauthNote}).
                  </p>
                {/if}
                {#if reauthError}
                  <p class="text-caption text-[var(--error)] flex items-center gap-1" role="alert">
                    <AlertTriangle size={12} /> {reauthError}
                  </p>
                {/if}
                <div class="flex items-center gap-2 flex-wrap">
                  <label class="flex-1 min-w-[180px]">
                    <span class="sr-only">Your current password</span>
                    <input
                      bind:value={reauthPassword}
                      type="password"
                      placeholder="your current password"
                      autocomplete="current-password"
                      disabled={reauthBusy}
                      class="w-full px-3 py-1.5 text-body-sm rounded-md border border-[var(--border)]
                             bg-[var(--bg)] text-[var(--text)] outline-none focus-visible:ring-2
                             focus-visible:ring-[var(--accent)] disabled:opacity-50"
                      onkeydown={(e) => { if (e.key === 'Enter') submitReauth(); }}
                    />
                  </label>
                  <button
                    class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                           px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors
                           disabled:opacity-40 disabled:cursor-not-allowed shrink-0"
                    disabled={reauthBusy || !reauthPassword}
                    onclick={submitReauth}
                  >
                    {reauthBusy ? "Verifying…" : "Confirm and continue"}
                  </button>
                  <button
                    class="text-body-sm text-[var(--text-muted)] px-3 py-1.5 rounded-md hover:bg-[var(--surface)] transition-colors disabled:opacity-50 shrink-0"
                    disabled={reauthBusy}
                    onclick={cancelReauth}
                  >
                    Cancel
                  </button>
                </div>
              </div>
            {/if}

            {#each users as u, i (u.id)}
              <div class="flex items-center gap-3 px-4 py-3 {i > 0 ? 'border-t border-[var(--border)]' : ''} {u.is_active ? '' : 'opacity-60'}">
                <div class="size-8 shrink-0 rounded-full bg-[var(--accent)] text-[var(--accent-text)] grid place-items-center text-micro font-semibold tracking-wide">
                  {initials(u.display_name || u.username)}
                </div>
                <div class="flex-1 min-w-0">
                  <div class="text-body text-[var(--text)] truncate leading-tight">
                    {u.display_name || u.username}
                    {#if u.id === user?.id}
                      <span class="text-caption text-[var(--text-faint)]">(you)</span>
                    {/if}
                  </div>
                  <div class="text-caption font-mono text-[var(--text-faint)] truncate leading-tight mt-0.5">@{u.username}</div>
                </div>
                {#if !u.is_active}
                  <span class="text-micro font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded-full shrink-0 text-[var(--warn-text)] bg-[color-mix(in_oklab,var(--warn)_15%,var(--bg))]">
                    Deactivated
                  </span>
                {/if}
                <span
                  class="text-micro font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded-full shrink-0
                         {u.is_admin
                    ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                    : 'text-[var(--text-muted)] bg-[var(--bg-subtle)]'}"
                >
                  {u.is_admin ? "Admin" : "Member"}
                </span>
                <span class="hidden sm:block text-caption text-[var(--text-faint)] tabular-nums shrink-0 w-[5.5rem] text-right">
                  <TimeAgo date={u.created_at} />
                </span>

                <!-- Actions. Never on your own row: demoting or deactivating
                     yourself from the page you are standing on is a footgun,
                     and another admin (or the CLI) can always do it. -->
                {#if u.id !== user?.id}
                  {#if pending?.id === u.id}
                    <div class="flex items-center gap-1.5 shrink-0">
                      <button
                        class="text-caption font-medium text-[var(--error-text)] bg-[var(--error)] px-2 py-1 rounded-md
                               hover:opacity-90 transition-opacity disabled:opacity-40"
                        disabled={rowBusy === u.id}
                        onclick={() =>
                          runAction(u, pending?.kind === "demote" ? demoteUser : deactivateUser)}
                      >
                        {#if rowBusy === u.id}
                          …
                        {:else}
                          {pending.kind === "demote" ? "Demote" : "Deactivate"}
                        {/if}
                      </button>
                      <button
                        class="text-caption text-[var(--text-muted)] px-2 py-1 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                        onclick={() => { pending = null; }}
                      >
                        Cancel
                      </button>
                    </div>
                  {:else}
                    <div class="flex items-center gap-1 shrink-0">
                      {#if u.is_admin}
                        <button
                          class="size-7 grid place-items-center rounded-md text-[var(--text-muted)]
                                 hover:text-[var(--warn-text)] hover:bg-[var(--bg-subtle)] transition-colors"
                          onclick={() => { pending = { id: u.id, kind: "demote" }; rowError = null; }}
                          title="Remove instance admin"
                          aria-label="Remove instance admin from {u.display_name || u.username}"
                        >
                          <ShieldMinus size={14} />
                        </button>
                      {:else}
                        <button
                          class="size-7 grid place-items-center rounded-md text-[var(--text-muted)]
                                 hover:text-[var(--accent)] hover:bg-[var(--accent-subtle)] transition-colors
                                 disabled:opacity-40 disabled:cursor-not-allowed"
                          disabled={rowBusy === u.id || creating || saving || settingsReplayInFlight || reauthFor !== null || reauthBusy}
                          onclick={() => runGrant(u, attemptPromote)}
                          title="Make instance admin"
                          aria-label="Make {u.display_name || u.username} an instance admin"
                        >
                          <ShieldPlus size={14} />
                        </button>
                      {/if}
                      {#if u.is_active}
                        <button
                          class="size-7 grid place-items-center rounded-md text-[var(--text-muted)]
                                 hover:text-[var(--error)] hover:bg-[var(--error-bg)] transition-colors"
                          onclick={() => { pending = { id: u.id, kind: "deactivate" }; rowError = null; }}
                          title="Deactivate account"
                          aria-label="Deactivate {u.display_name || u.username}"
                        >
                          <UserMinus size={14} />
                        </button>
                      {:else}
                        <button
                          class="size-7 grid place-items-center rounded-md text-[var(--text-muted)]
                                 hover:text-[var(--success)] hover:bg-[var(--success-bg)] transition-colors
                                 disabled:opacity-40 disabled:cursor-not-allowed"
                          disabled={rowBusy === u.id || creating || saving || settingsReplayInFlight || reauthFor !== null || reauthBusy}
                          onclick={() => runGrant(u, attemptReactivate)}
                          title="Restore account"
                          aria-label="Restore {u.display_name || u.username}"
                        >
                          <RotateCcw size={14} />
                        </button>
                      {/if}
                    </div>
                  {/if}
                {/if}
              </div>
              {#if rowError?.id === u.id}
                <div class="px-4 pb-2.5 -mt-1 text-caption text-[var(--error)]">{rowError.message}</div>
              {/if}
            {/each}
          </div>
          <p class="text-caption text-[var(--text-faint)] mt-2 max-w-[60ch] leading-relaxed">
            Deactivating an account ends its sessions and revokes its API keys and tokens. Nothing it wrote is
            removed. The last admin who can still sign in cannot be demoted or deactivated.
          </p>
        </section>
      {/if}
    {/if}
  </div>
</div>
