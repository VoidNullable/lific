import { beforeEach, expect, test } from "bun:test";
import { get } from "svelte/store";
import { currentUser, getUserRevision, publishUser } from "../src/lib/userState";

const original = { id: 1, username: "test", display_name: "Before", email: "test@example.com", is_admin: false };
beforeEach(() => currentUser.set(null));

test("successful profile publication reaches all subscribers immediately", () => {
  const shell: string[] = [];
  const settings: string[] = [];
  const stopShell = currentUser.subscribe(user => shell.push(user?.display_name ?? ""));
  const stopSettings = currentUser.subscribe(user => settings.push(user?.display_name ?? ""));
  publishUser(original);
  publishUser({ ...original, display_name: "After" });
  expect(shell).toEqual(["", "Before", "After"]);
  expect(settings).toEqual(shell);
  stopShell(); stopSettings();
});

test("late me response cannot replace a newer profile", () => {
  const revision = getUserRevision();
  publishUser({ ...original, display_name: "Saved" });
  expect(publishUser(original, revision)).toBe(false);
  expect(get(currentUser)?.display_name).toBe("Saved");
});

test("unchanged revision accepts me, sign-out invalidates pending loads", () => {
  expect(publishUser(original, getUserRevision())).toBe(true);
  const revision = getUserRevision();
  currentUser.set(null);
  expect(publishUser(original, revision)).toBe(false);
  expect(get(currentUser)).toBeNull();
});

test("direct store updates also invalidate stale loads", () => {
  currentUser.set(original);
  const revision = getUserRevision();
  currentUser.update(user => user && { ...user, display_name: "Updated" });
  expect(publishUser(original, revision)).toBe(false);
  expect(get(currentUser)?.display_name).toBe("Updated");
});
