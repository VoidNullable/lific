const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  identifierError,
  safeColor,
  normalizeColor,
  archiveError,
  ProjectSettingsController,
} = require("./project-settings.js");
const success = (data) => ({ ok: true, data });
function setup(handler = async () => success({})) {
  const calls = [];
  let identity = "private:1";
  const states = [];
  const controller = new ProjectSettingsController({
    identity: () => identity,
    request: async (path, options) => {
      calls.push({ path, ...options });
      return handler(path, options);
    },
    onChange: (state) => states.push(structuredClone(state)),
  });
  controller.state = {
    ...controller.state,
    project: { id: 4, identifier: "LIF", name: "Lific", lead_user_id: 1 },
    role: { role: "lead", enforced: true, is_admin: false },
    members: [],
    labels: [],
    groups: [],
    projects: [],
    users: [],
  };
  return {
    controller,
    calls,
    states,
    changeAccount() {
      identity = "private:2";
      controller.accountChanged();
    },
  };
}
test("identifier validation matches reserved word and server grammar", () => {
  for (const value of ["", "DOC", "1ABC", "TOOLONG", "A-B"])
    assert.ok(identifierError(value));
  assert.equal(identifierError("PRO2"), "");
});
test("custom label colors normalize while legacy unsafe colors never reach CSS", () => {
  assert.equal(normalizeColor("aBc"), "#aabbcc");
  assert.equal(safeColor("red; background:url(x)"), "#6B7280");
  assert.equal(safeColor("#EF4444"), "#EF4444");
});
test("archive selection rejects nonarchives empty and oversized files", () => {
  assert.ok(archiveError({ name: "a.zip", size: 1 }, 20));
  assert.ok(archiveError({ name: "a.tar.gz", size: 0 }, 20));
  assert.ok(archiveError({ name: "a.tar.gz", size: 21 }, 20));
  assert.equal(archiveError({ name: "a.tar.gz", size: 20 }, 20), "");
});
test("project created before failed group assignment remains usable with warning", async () => {
  const { controller, calls } = setup(async (path) =>
    path === "/projects"
      ? success({ id: 8, identifier: "ABC" })
      : { ok: false, error: "group removed" },
  );
  await controller.create({
    name: " A project ",
    identifier: "abc",
    group_id: 2,
  });
  assert.equal(controller.state.project.id, 8);
  assert.match(controller.state.warning, /created.*group removed/);
  assert.equal(calls[0].body.identifier, "ABC");
  assert.deepEqual(calls[1].body, { project_id: 8, group_id: 2 });
});
test("viewer cannot mutate project or member roles; label edits remain maintainer gated", async () => {
  const { controller, calls } = setup();
  controller.state.role.role = "viewer";
  assert.equal(await controller.save({ name: "bad" }), false);
  assert.equal(await controller.member("change", 3, "lead"), false);
  assert.equal(
    await controller.label("create", { name: "bug", color: "#123456" }),
    false,
  );
  assert.equal(calls.length, 0);
  controller.state.role.role = "maintainer";
  await controller.label("create", { name: "bug", color: "#123456" });
  assert.equal(calls[0].path, "/labels");
});
test("membership uses PATCH role endpoint and surfaces forbidden failures", async () => {
  const { controller, calls } = setup(async () => ({
    ok: false,
    status: 403,
    error: "Only a lead can change roles",
  }));
  await controller.member("change", 3, "maintainer");
  assert.equal(calls[0].method, "PATCH");
  assert.deepEqual(calls[0].body, { role: "maintainer" });
  assert.match(controller.state.error, /Only a lead/);
  assert.deepEqual(controller.state.members, []);
});
test("label merge retains target color and removes source only after server success", async () => {
  const { controller, calls } = setup(async () =>
    success({ id: 2, name: "target", color: "#123456" }),
  );
  controller.state.labels = [
    { id: 1, name: "source", color: "#ffffff" },
    { id: 2, name: "target", color: "#123456" },
  ];
  await controller.label("merge", { id: 1, into: 2 });
  assert.equal(calls[0].path, "/labels/1/merge");
  assert.deepEqual(controller.state.labels, [
    { id: 2, name: "target", color: "#123456" },
  ]);
});
test("failed project reorder keeps canonical order and permits retry", async () => {
  const { controller } = setup(async () => ({ ok: false, error: "offline" }));
  controller.state.projects = [{ id: 1 }, { id: 2 }];
  await controller.group("reorder-projects", { ids: [2, 1] });
  assert.deepEqual(controller.state.projects, [{ id: 1 }, { id: 2 }]);
  assert.equal(controller.state.busy, false);
  assert.equal(controller.state.error, "offline");
});
test("late mutation cannot update a different signed-in account", async () => {
  let finish;
  const { controller, changeAccount } = setup(
    () => new Promise((resolve) => (finish = resolve)),
  );
  const pending = controller.save({ name: "new" });
  changeAccount();
  finish(success({ id: 4, name: "new" }));
  await pending;
  assert.equal(controller.state.project, null);
  assert.equal(controller.state.status, "idle");
});
test("archive upload distinguishes progress processing and uncertain connection loss", async () => {
  const { controller } = setup();
  controller.state.capabilities = { can_import: true, max_upload_bytes: 100 };
  controller.env.upload = async (file, progress, processing) => {
    progress(57);
    assert.equal(controller.state.archive.progress, 57);
    processing();
    assert.equal(controller.state.archive.phase, "processing");
    return { ok: false, status: null, error: "connection lost" };
  };
  await controller.importArchive({ name: "p.tar.gz", size: 10 }, true);
  assert.equal(controller.state.archive.phase, "unknown");
  assert.match(controller.state.archive.error, /Check the project list/);
  assert.equal(
    await controller.importArchive({ name: "p.tar.gz", size: 10 }, true),
    false,
  );
  controller.acknowledgeArchive();
  assert.equal(controller.state.archive.phase, "idle");
});
test("invalid archive can be retried and imported report preserves external references", async () => {
  const { controller } = setup();
  controller.state.capabilities = { can_import: true, max_upload_bytes: 100 };
  controller.env.upload = async () => ({
    ok: false,
    status: 422,
    error: "checksum invalid",
  });
  await controller.importArchive({ name: "p.tar.gz", size: 10 }, true);
  assert.equal(controller.state.archive.phase, "error");
  const result = {
    project: { id: 9, identifier: "NEW", is_public: false },
    report: {
      rows: { issues: 3 },
      blobs: 2,
      external_reference_count: 1,
      external_references: ["OLD-1"],
    },
  };
  controller.env.upload = async () => success(result);
  await controller.importArchive({ name: "p.tar.gz", size: 10 }, true);
  assert.deepEqual(controller.state.archive.result, result);
  assert.equal(controller.state.archive.phase, "success");
});
test("archive download is revoked after an account switch while preparing", async () => {
  let finish;
  const { controller, changeAccount } = setup();
  controller.env.download = () => new Promise((resolve) => (finish = resolve));
  controller.env.saveDownload = () => assert.fail("stale archive saved");
  const pending = controller.exportArchive(true);
  changeAccount();
  finish(success({ blob: "secret" }));
  await pending;
  assert.equal(controller.state.project, null);
});
test("project group assignment failure rolls back selection and successful order uses server response", async () => {
  let fail = true;
  const { controller, calls } = setup(async (path) =>
    fail
      ? { ok: false, error: "no group" }
      : success(
          path === "/project-groups/reorder" ? [{ id: 2 }, { id: 1 }] : [],
        ),
  );
  controller.state.groups = [
    { id: 1, project_ids: [4] },
    { id: 2, project_ids: [] },
  ];
  await controller.group("assign", { group_id: 2 });
  assert.deepEqual(controller.state.groups[0].project_ids, [4]);
  fail = false;
  await controller.group("reorder", { ids: [2, 1] });
  assert.deepEqual(controller.state.groups, [{ id: 2 }, { id: 1 }]);
  assert.deepEqual(calls[1].body, { ids: [2, 1] });
});
test("GitHub preview freezes the repository token and mapping for confirmed import", async () => {
  const { controller, calls } = setup(async (_path, options) =>
    success({ dry_run: options.body.dry_run, issues_created: 2 }),
  );
  await controller.importGithub({
    repo: "owner/repo",
    token: "secret",
    state: "all",
    map_open: "backlog",
    map_closed: "done",
    dry_run: true,
  });
  await controller.confirmGithub();
  assert.equal(calls[1].body.token, "secret");
  assert.equal(calls[1].body.dry_run, false);
  assert.equal(controller.state.summary.issues_created, 2);
});
test("recent-auth refusal preserves action and retries once after verified refresh", async () => {
  let requests = 0;
  const { controller } = setup(async () =>
    ++requests === 1
      ? { ok: false, status: 403, error: "recent authentication required" }
      : success({ id: 4, name: "new" }),
  );
  controller.env.reauthenticate = async (password) => {
    assert.equal(password, "correct");
    return { ok: true };
  };
  await controller.save({ name: "new" });
  assert.equal(controller.state.needsAuth, true);
  await controller.resume("correct");
  assert.equal(controller.state.project.name, "new");
  assert.equal(requests, 2);
  assert.equal(controller.state.needsAuth, false);
});
test("unknown archive import survives page reload until project list is checked", async () => {
  const saved = new Map();
  const { controller } = setup(async (path) =>
    success(
      path === "/projects"
        ? []
        : path === "/project-archives"
          ? { can_import: true, max_upload_bytes: 100 }
          : [],
    ),
  );
  controller.env.storage = {
    setItem: (key, value) => saved.set(key, value),
    getItem: (key) => saved.get(key),
    removeItem: (key) => saved.delete(key),
  };
  controller.state.capabilities = { can_import: true, max_upload_bytes: 100 };
  controller.env.upload = async () => ({
    ok: false,
    status: null,
    error: "network lost",
  });
  await controller.importArchive({ name: "p.tar.gz", size: 1 }, true);
  await controller.load();
  assert.equal(controller.state.archive.phase, "unknown");
  controller.acknowledgeArchive();
  assert.equal(saved.size, 0);
});
test("archive export requires confirmation and verified account before saving", async () => {
  const { controller } = setup(async () => success({ id: 1 }));
  let saved = 0;
  controller.env.download = async () => ({
    ok: true,
    data: { blob: "archive", filename: "LIF.lific.tar.gz" },
  });
  controller.env.saveDownload = () => saved++;
  assert.equal(await controller.exportArchive(false), false);
  await controller.exportArchive(true);
  assert.equal(saved, 1);
});

test("project and membership events refresh authoritative identity and role", async () => {
  let role = "lead";
  const project = {
    id: 4,
    identifier: "LIF",
    name: "Current project",
    lead_user_id: null,
  };
  const { controller, calls } = setup(async (path) =>
    success(
      path === "/projects"
        ? [project]
        : path.endsWith("/my-role")
          ? { role, enforced: true, is_admin: false }
          : [],
    ),
  );
  controller.identifier = "LIF";
  role = "viewer";
  await controller.handleEvent({ type: "project.updated", project_id: 4 });
  assert.equal(controller.state.project.name, "Current project");
  assert.equal(controller.state.role.role, "viewer");
  const requests = calls.length;
  await controller.handleEvent({ type: "project.updated", project_id: 99 });
  assert.equal(calls.length, requests);
  role = "maintainer";
  await controller.handleEvent({ type: "membership.changed", project_id: 4 });
  assert.equal(controller.state.role.role, "maintainer");
});
test("reconnect and catalog refresh update permissions without losing archive outcome", async () => {
  const project = {
    id: 4,
    identifier: "LIF",
    name: "Project",
    lead_user_id: null,
  };
  const { controller, calls } = setup(async (path) =>
    success(
      path === "/projects"
        ? [project]
        : path.endsWith("/my-role")
          ? { role: "viewer", enforced: true, is_admin: false }
          : [],
    ),
  );
  controller.identifier = "LIF";
  controller.state.archive = {
    phase: "success",
    progress: null,
    result: { project: { identifier: "IMPT" } },
    error: "",
  };
  await controller.connectionChanged(true);
  assert.equal(controller.state.role.role, "viewer");
  const count = calls.length;
  await controller.connectionChanged(true);
  assert.equal(calls.length, count);
  await controller.connectionChanged(false);
  await controller.connectionChanged(true);
  assert.ok(calls.length > count);
  await controller.handleEvent({ type: "catalog.changed" });
  assert.equal(controller.state.archive.phase, "success");
});
test("successful identifier rename adopts and navigates to the returned identifier", async () => {
  const { controller } = setup(async () =>
    success({ id: 4, identifier: "NEW", name: "Lific" }),
  );
  controller.identifier = "LIF";
  let destination;
  controller.env.navigate = (path) => (destination = path);
  await controller.save({ identifier: "new" });
  assert.equal(controller.identifier, "NEW");
  assert.equal(destination, "/NEW/overview");
});
test("membership role and add writes retain display metadata absent from server write DTO", async () => {
  const { controller } = setup(async (_path, options) =>
    success({
      project_id: 4,
      user_id: 3,
      role: options.body.role,
      created_at: "now",
    }),
  );
  controller.state.members = [
    {
      project_id: 4,
      user_id: 3,
      role: "viewer",
      username: "ada",
      display_name: "Ada Lovelace",
    },
  ];
  controller.state.users = [
    { id: 3, username: "ada", display_name: "Ada Lovelace" },
  ];
  await controller.member("change", 3, "maintainer");
  assert.equal(controller.state.members[0].display_name, "Ada Lovelace");
  controller.state.members = [];
  await controller.member("add", 3, "viewer");
  assert.equal(controller.state.members[0].username, "ada");
});
test("archive download rejects viewer and maintainer even when general authorization is off", async () => {
  const { controller } = setup();
  let downloads = 0;
  controller.env.download = async () => {
    downloads++;
    return { ok: false, error: "unexpected" };
  };
  for (const role of ["viewer", "maintainer"]) {
    controller.state.role = { role, enforced: false, is_admin: false };
    assert.equal(await controller.exportArchive(true), false);
  }
  assert.equal(downloads, 0);
});

test("same-account refresh cancels stale reauthentication with an actionable retry notice", async () => {
  let authenticated = false;
  const project = { id: 4, identifier: "LIF", name: "Project" };
  const { controller } = setup(async (path, options) =>
    options.method === "PUT"
      ? authenticated
        ? success({ ...project, ...options.body })
        : { ok: false, status: 403, error: "recent authentication required" }
      : success(
          path === "/projects"
            ? [project]
            : path.endsWith("/my-role")
              ? { role: "lead", enforced: true, is_admin: false }
              : [],
        ),
  );
  controller.identifier = "LIF";
  controller.state.status = "ready";
  controller.env.reauthenticate = async () => {
    authenticated = true;
    return { ok: true };
  };
  await controller.save({ name: "Changed" });
  assert.equal(controller.state.needsAuth, true);
  await controller.handleEvent({ type: "catalog.changed" });
  assert.equal(controller.state.needsAuth, false);
  assert.equal(controller.pending, null);
  assert.match(controller.state.error, /Submit the action again/);
  await controller.handleEvent({ type: "project.updated", project_id: 4 });
  assert.match(controller.state.error, /Submit the action again/);
  await controller.save({ name: "Changed" });
  assert.equal(controller.state.needsAuth, true);
  assert.equal(await controller.resume("password"), true);
  assert.equal(controller.state.project.name, "Changed");
});
test("refresh queued during a refused mutation does not leave an unusable password confirmation", async () => {
  let reply;
  const project = { id: 4, identifier: "LIF", name: "Project" };
  const { controller } = setup(async (path, options) =>
    options.method === "PUT"
      ? new Promise((resolve) => (reply = resolve))
      : success(
          path === "/projects"
            ? [project]
            : path.endsWith("/my-role")
              ? { role: "lead", enforced: true, is_admin: false }
              : [],
        ),
  );
  controller.identifier = "LIF";
  controller.state.status = "ready";
  const attempt = controller.save({ name: "Changed" });
  await controller.handleEvent({ type: "project.updated", project_id: 4 });
  reply({ ok: false, status: 403, error: "recent authentication required" });
  await attempt;
  await new Promise(setImmediate);
  assert.equal(controller.state.needsAuth, false);
  assert.equal(controller.pending, null);
  assert.match(controller.state.error, /Submit the action again/);
});
