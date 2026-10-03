(() => {
  "use strict";
  const PALETTE = [
    ["Red", "#EF4444"],
    ["Orange", "#F97316"],
    ["Amber", "#D97706"],
    ["Green", "#16A34A"],
    ["Emerald", "#059669"],
    ["Teal", "#0D9488"],
    ["Cyan", "#0891B2"],
    ["Blue", "#2563EB"],
    ["Indigo", "#4F46E5"],
    ["Violet", "#7C3AED"],
    ["Pink", "#DB2777"],
    ["Gray", "#6B7280"],
  ];
  function identifierError(value) {
    return value === "DOC"
      ? "DOC is reserved for page identifiers."
      : /^[A-Z][A-Z0-9]{0,4}$/.test(value)
        ? ""
        : "Use 1–5 uppercase letters or digits, starting with a letter.";
  }
  function safeColor(value) {
    return /^#[a-f\d]{6}$/i.test(value) ? value : "#6B7280";
  }
  function normalizeColor(value) {
    let color = value.trim().replace(/^#/, "");
    if (!/^(?:[a-f\d]{3}|[a-f\d]{6})$/i.test(color))
      throw new Error("Enter a 3 or 6 digit hex color.");
    if (color.length === 3) color = [...color].map((c) => c + c).join("");
    return "#" + color.toLowerCase();
  }
  function archiveError(file, limit) {
    return !file || !file.name.toLowerCase().endsWith(".tar.gz")
      ? "Choose a Lific project archive ending in .tar.gz."
      : !file.size
        ? "This archive is empty."
        : file.size > limit
          ? "This archive exceeds the web upload limit. Use the CLI for larger archives."
          : "";
  }
  function permissions(role) {
    return {
      edit:
        !!role &&
        (!role.enforced ||
          role.is_admin ||
          ["maintainer", "lead"].includes(role.role)),
      manage:
        !!role && (!role.enforced || role.is_admin || role.role === "lead"),
    };
  }
  function emptyState() {
    return {
      status: "idle",
      project: null,
      role: null,
      projects: [],
      groups: [],
      labels: [],
      members: [],
      users: [],
      capabilities: null,
      error: "",
      warning: "",
      busy: false,
      archive: { phase: "idle", progress: null, result: null, error: "" },
      summary: null,
      needsAuth: false,
      refreshing: false,
      authNotice: "",
    };
  }
  class ProjectSettingsController {
    constructor(env, identifier = null) {
      this.env = env;
      this.identifier = identifier;
      this.state = emptyState();
      this.generation = 0;
      this.disposed = false;
      this.pending = null;
      this.githubPreview = null;
      this.reauthenticating = false;
      this.refreshQueued = false;
      this.connected = false;
    }
    publish(patch) {
      this.state = { ...this.state, ...patch };
      this.env.onChange?.(this.state);
    }
    accountChanged() {
      this.refreshQueued = false;
      this.pending = null;
      this.githubPreview = null;
      this.generation++;
      this.publish(emptyState());
    }
    dispose() {
      this.githubPreview = null;
      this.disposed = true;
      this.generation++;
    }
    archiveKey() {
      return "lific:topcoat:archive-pending:" + this.env.identity();
    }
    pendingArchive(value) {
      try {
        value
          ? this.env.storage?.setItem(this.archiveKey(), "1")
          : this.env.storage?.removeItem(this.archiveKey());
      } catch {}
    }
    acknowledgeArchive() {
      if (!this.state.busy) {
        this.pendingArchive(false);
        this.publish({
          archive: { phase: "idle", progress: null, result: null, error: "" },
        });
      }
    }
    async importArchive(file, confirmed) {
      const caps = this.state.capabilities;
      if (
        !caps?.can_import ||
        !confirmed ||
        ["unknown", "success"].includes(this.state.archive.phase)
      )
        return false;
      return this.mutate(null, async (live) => {
        const error = archiveError(file, caps.max_upload_bytes);
        if (error) throw new Error(error);
        this.pendingArchive(true);
        this.publish({
          archive: { phase: "uploading", progress: 0, result: null, error: "" },
        });
        let result;
        try {
          result = await this.env.upload(
            file,
            (progress) => {
              if (live())
                this.publish({ archive: { ...this.state.archive, progress } });
            },
            () => {
              if (live())
                this.publish({
                  archive: { ...this.state.archive, phase: "processing" },
                });
            },
          );
        } catch (error) {
          result = { ok: false, status: null, error: error.message };
        }
        if (!live()) return;
        if (result.ok) {
          this.pendingArchive(false);
          this.publish({
            archive: {
              phase: "success",
              progress: null,
              result: result.data,
              error: "",
            },
          });
        } else {
          const unknown = result.status == null;
          if (!unknown) this.pendingArchive(false);
          this.publish({
            archive: {
              phase: unknown ? "unknown" : "error",
              progress: null,
              result: null,
              error: unknown
                ? `${result.error} Check the project list before importing again.`
                : result.error,
            },
          });
        }
      });
    }
    exportData() {
      return this.mutate("manage", async (live) => {
        const result = await this.env.download(
          `/export/projects/${encodeURIComponent(this.state.project.identifier)}`,
          `${this.state.project.identifier}.json`,
        );
        if (!live()) return;
        if (!result.ok) throw new Error(result.error);
        this.env.saveDownload(result.data);
      });
    }
    canExportArchive() {
      return (
        !!this.state.role &&
        (this.state.role.is_admin ||
          this.state.role.role === "lead" ||
          (this.env.userId?.() != null &&
            this.state.project?.lead_user_id === this.env.userId()))
      );
    }
    exportArchive(confirmed) {
      if (!confirmed || !this.canExportArchive()) return Promise.resolve(false);
      return this.mutate(null, async (live) => {
        const result = await this.env.download(
          `/project-archives/${encodeURIComponent(this.state.project.identifier)}`,
        );
        if (!live()) return;
        if (!result.ok) throw new Error(result.error);
        const user = await this.request("/auth/me");
        if (
          live() &&
          user.ok &&
          `private:${user.data.id}` === this.env.identity()
        )
          this.env.saveDownload(result.data);
        else if (live())
          throw new Error(
            "Your account changed. Download the archive again after signing in.",
          );
      });
    }
    async request(path, options = {}) {
      try {
        return await this.env.request(path, options);
      } catch (error) {
        return { ok: false, status: null, error: error.message };
      }
    }
    async refresh() {
      if (this.disposed || this.env.identity() === null) return;
      if (
        this.state.busy ||
        this.reauthenticating ||
        this.state.refreshing ||
        this.state.status === "loading"
      ) {
        this.refreshQueued = true;
        return;
      }
      this.refreshQueued = false;
      await this.load(true);
      if (this.refreshQueued) return this.refresh();
    }
    handleEvent(event) {
      const projectId = this.state.project?.id;
      const relevant =
        event.type === "resync.required" ||
        event.type === "sync_required" ||
        event.type === "catalog.changed" ||
        event.type === "projects.reordered" ||
        event.type === "project_groups.changed" ||
        /^(project|membership|member|label)\./.test(event.type);
      const matches =
        event.project_id == null ||
        projectId == null ||
        event.project_id === projectId;
      return relevant && matches ? this.refresh() : Promise.resolve();
    }
    connectionChanged(connected) {
      const reconnected = connected && !this.connected;
      this.connected = connected;
      return reconnected ? this.refresh() : Promise.resolve();
    }
    async load(refresh = false) {
      const previous = this.state;
      const identity = this.env.identity(),
        generation = ++this.generation;
      const notice =
        refresh && this.pending
          ? "Project access was refreshed. Submit the action again to confirm your password."
          : refresh
            ? previous.authNotice || ""
            : "";
      if (notice) this.pending = null;
      this.publish(
        refresh && previous.status === "ready"
          ? {
              refreshing: true,
              error: notice,
              authNotice: notice,
              ...(notice ? { needsAuth: false } : {}),
            }
          : {
              ...emptyState(),
              status: "loading",
              error: notice,
              authNotice: notice,
            },
      );
      if (identity === null) {
        this.publish({ status: "idle", refreshing: false });
        return;
      }
      const current = () =>
        !this.disposed &&
        generation === this.generation &&
        identity === this.env.identity();
      const projects = await this.request("/projects");
      if (!current()) return;
      if (!projects.ok) {
        this.publish({
          status: "error",
          role: null,
          refreshing: false,
          error: projects.error,
        });
        return;
      }
      const project = this.identifier
        ? projects.data.find(
            (p) => p.identifier === this.identifier.toUpperCase(),
          )
        : null;
      if (this.identifier && !project) {
        this.publish({
          status: "error",
          role: null,
          refreshing: false,
          error: "Project not found or access was removed.",
        });
        return;
      }
      const paths = ["/project-groups", "/users", "/project-archives"];
      if (project)
        paths.push(
          `/projects/${project.id}/my-role`,
          `/labels?project_id=${project.id}`,
          `/projects/${project.id}/members`,
        );
      const results = await Promise.all(
        paths.map((path) => this.request(path)),
      );
      if (!current()) return;
      const [groups, users, capabilities, role, labels, members] = results;
      const failures = results.filter((result) => !result.ok);
      if (role && !role.ok) {
        this.publish({
          status: "error",
          role: null,
          refreshing: false,
          error: role.error,
        });
        return;
      }
      let restoredArchive = refresh ? previous.archive : this.state.archive;
      try {
        if (this.env.storage?.getItem(this.archiveKey()))
          restoredArchive = {
            phase: "unknown",
            progress: null,
            result: null,
            error:
              "The previous import outcome is unknown. Check the project list before importing again.",
          };
      } catch {}
      this.publish({
        archive: restoredArchive,
        status: "ready",
        refreshing: false,
        summary: refresh ? previous.summary : null,
        project,
        projects: projects.data,
        groups: groups.ok ? groups.data : [],
        users: users.ok ? users.data : [],
        capabilities: capabilities.ok ? capabilities.data : null,
        role: role?.data || null,
        labels: labels?.data || [],
        members: members?.data || [],
        warning: failures.map((result) => result.error).join(" "),
      });
      if (!refresh && this.refreshQueued) void this.refresh();
    }
    async mutate(permission, operation) {
      const identity = this.env.identity(),
        generation = this.generation;
      if (
        this.disposed ||
        identity === null ||
        this.state.busy ||
        this.state.refreshing ||
        (permission && !permissions(this.state.role)[permission])
      )
        return false;
      this.publish({ busy: true, error: "", warning: "", authNotice: "" });
      const live = () =>
        !this.disposed &&
        generation === this.generation &&
        identity === this.env.identity();
      try {
        await operation(live);
      } catch (error) {
        if (live()) {
          if (
            error.status === 403 &&
            error.message === "recent authentication required"
          ) {
            this.pending = { permission, operation, identity, generation };
            this.publish({
              needsAuth: true,
              error: "Confirm your password to continue.",
            });
          } else this.publish({ error: error.message });
        }
      } finally {
        if (live()) this.publish({ busy: false });
      }
      const success = live() && !this.state.error;
      if (live() && this.refreshQueued) void this.refresh();
      return success;
    }
    async resume(password) {
      const pending = this.pending;
      if (
        !pending ||
        pending.identity !== this.env.identity() ||
        pending.generation !== this.generation
      )
        return false;
      this.reauthenticating = true;
      const result = await this.env.reauthenticate(password);
      this.reauthenticating = false;
      if (
        pending.identity !== this.env.identity() ||
        pending.generation !== this.generation
      ) {
        this.accountChanged();
        return false;
      }
      if (!result.ok) {
        this.publish({ error: result.error });
        if (this.refreshQueued) void this.refresh();
        return false;
      }
      this.pending = null;
      this.publish({ needsAuth: false, error: "" });
      const success = await this.mutate(pending.permission, pending.operation);
      if (this.state.needsAuth) {
        this.pending = null;
        this.publish({
          needsAuth: false,
          error:
            "That still was not accepted. Sign out and back in, then try again.",
        });
      }
      return success;
    }
    cancelReauth() {
      this.pending = null;
      this.publish({ needsAuth: false, error: "" });
    }
    async required(path, method, body) {
      const result = await this.request(path, { method, body });
      if (!result.ok) {
        const error = new Error(result.error || "Request failed.");
        error.status = result.status;
        throw error;
      }
      return result.data;
    }
    create(draft) {
      return this.mutate(null, async (live) => {
        const identifier = draft.identifier.trim().toUpperCase();
        const error = identifierError(identifier);
        if (error) throw new Error(error);
        if (!draft.name.trim()) throw new Error("Enter a project name.");
        const body = {
          name: draft.name.trim(),
          identifier,
          description: draft.description?.trim() || "",
          emoji: draft.emoji || undefined,
          lead_user_id: draft.lead_user_id || undefined,
        };
        const project = await this.required("/projects", "POST", body);
        if (!live()) return;
        this.publish({ project });
        if (draft.group_id != null) {
          const assigned = await this.request("/project-groups/assign", {
            method: "PUT",
            body: { project_id: project.id, group_id: draft.group_id },
          });
          if (live() && !assigned.ok)
            this.publish({
              warning: `Project created, but it was not added to the group: ${assigned.error}`,
            });
        }
      });
    }
    save(input) {
      return this.mutate("manage", async (live) => {
        if (input.identifier) {
          const error = identifierError(input.identifier.trim().toUpperCase());
          if (error) throw new Error(error);
          input = {
            ...input,
            identifier: input.identifier.trim().toUpperCase(),
          };
        }
        const project = await this.required(
          `/projects/${this.state.project.id}`,
          "PUT",
          input,
        );
        if (live()) {
          const renamed =
            !!input.identifier &&
            project.identifier !== this.state.project.identifier;
          if (renamed) this.identifier = project.identifier;
          this.publish({ project });
          if (renamed)
            this.env.navigate?.(
              `/${encodeURIComponent(project.identifier)}/overview`,
            );
        }
      });
    }
    deleteProject(confirmation) {
      return this.mutate("manage", async (live) => {
        if (confirmation !== this.state.project.identifier)
          throw new Error("Type the project identifier to confirm deletion.");
        await this.required(`/projects/${this.state.project.id}`, "DELETE");
        if (live()) {
          this.publish({ project: null });
          this.env.navigate?.("/");
        }
      });
    }
    member(action, userId, role = "viewer") {
      return this.mutate("manage", async (live) => {
        if (!["viewer", "maintainer", "lead"].includes(role))
          throw new Error("Choose a valid project role.");
        const project = this.state.project.id,
          base = `/projects/${project}/members`;
        const data = await this.required(
          action === "add" ? base : `${base}/${userId}`,
          { add: "POST", change: "PATCH", remove: "DELETE" }[action],
          action === "add"
            ? { user_id: userId, role }
            : action === "change"
              ? { role }
              : undefined,
        );
        if (live())
          this.publish({
            members:
              action === "remove"
                ? this.state.members.filter((m) => m.user_id !== userId)
                : [
                    ...this.state.members.filter((m) => m.user_id !== userId),
                    {
                      username:
                        this.state.members.find(
                          (member) => member.user_id === userId,
                        )?.username ??
                        this.state.users.find((user) => user.id === userId)
                          ?.username ??
                        "",
                      display_name:
                        this.state.members.find(
                          (member) => member.user_id === userId,
                        )?.display_name ??
                        this.state.users.find((user) => user.id === userId)
                          ?.display_name ??
                        "",
                      ...data,
                    },
                  ],
          });
      });
    }
    label(action, input) {
      return this.mutate("edit", async (live) => {
        const base = "/labels",
          path =
            action === "create"
              ? base
              : action === "merge"
                ? `${base}/${input.id}/merge`
                : `${base}/${input.id}`;
        const body =
          action === "create"
            ? {
                project_id: this.state.project.id,
                name: input.name.trim(),
                color: normalizeColor(input.color),
              }
            : action === "update"
              ? { name: input.name.trim(), color: normalizeColor(input.color) }
              : action === "merge"
                ? { into: input.into }
                : undefined;
        if (action === "merge" && input.id === input.into)
          throw new Error("Choose another label to merge into.");
        const data = await this.required(
          path,
          { create: "POST", update: "PUT", merge: "POST", delete: "DELETE" }[
            action
          ],
          body,
        );
        if (live()) {
          const labels = this.state.labels.filter(
            (label) =>
              label.id !== input.id &&
              (action === "delete" || label.id !== data.id),
          );
          this.publish({
            labels: action === "delete" ? labels : [...labels, data],
          });
        }
      });
    }
    group(action, input) {
      return this.mutate(null, async (live) => {
        const contracts = {
          create: ["/project-groups", "POST", { name: input.name }],
          rename: [
            `/project-groups/${input.id}`,
            "PATCH",
            { name: input.name },
          ],
          delete: [`/project-groups/${input.id}`, "DELETE"],
          assign: [
            "/project-groups/assign",
            "PUT",
            {
              project_id: input.project_id ?? this.state.project.id,
              group_id: input.group_id,
            },
          ],
          reorder: ["/project-groups/reorder", "PUT", { ids: input.ids }],
          "reorder-projects": ["/projects/reorder", "PUT", { ids: input.ids }],
        };
        const data = await this.required(...contracts[action]);
        if (!live()) return;
        if (action === "reorder-projects") this.publish({ projects: data });
        else if (action === "reorder") this.publish({ groups: data });
        else {
          const groups = await this.required("/project-groups", "GET");
          if (live()) this.publish({ groups });
        }
      });
    }
    confirmGithub() {
      return this.githubPreview
        ? this.importGithub({ ...this.githubPreview, dry_run: false })
        : Promise.resolve(false);
    }
    resetGithub() {
      this.githubPreview = null;
      this.publish({ summary: null });
    }
    importGithub(input) {
      return this.mutate("manage", async (live) => {
        if (!/^[^/\s]+\/[^/\s]+$/.test(input.repo.trim()))
          throw new Error("Enter a GitHub repository as owner/repository.");
        const summary = await this.required(
          `/projects/${this.state.project.id}/import/github`,
          "POST",
          { ...input, repo: input.repo.trim() },
        );
        if (live()) {
          this.githubPreview = input.dry_run ? { ...input } : null;
          this.publish({ summary });
        }
      });
    }
  }

  function node(doc, tag, text = null, attrs = {}) {
    const value = doc.createElement(tag);
    if (text !== null) value.textContent = text;
    for (const [key, item] of Object.entries(attrs)) {
      if (item != null) value.setAttribute(key, String(item));
    }
    return value;
  }
  function button(doc, text, action, key = text) {
    const value = node(doc, "button", text, {
      type: "button",
      class: "tc-button",
      "data-settings-focus": key,
    });
    value.addEventListener("click", action);
    return value;
  }
  function link(doc, text, href) {
    return node(doc, "a", text, { href, class: "tc-button" });
  }
  function section(doc, title) {
    const value = node(doc, "section");
    value.append(node(doc, "h2", title));
    return value;
  }
  function field(doc, form, name, title, value = "", options = {}) {
    const label = node(doc, "label", title);
    const control = node(
      doc,
      options.select ? "select" : options.textarea ? "textarea" : "input",
      null,
      {
        name,
        "aria-label": title,
        "data-settings-focus": `${form.dataset.form}:${name}`,
        type: options.type || "text",
        required: options.required ? "required" : null,
        autocomplete: options.autocomplete,
        maxlength: options.maxlength,
      },
    );
    if (options.select)
      for (const [id, title] of options.select)
        control.append(node(doc, "option", title, { value: id ?? "" }));
    if (options.type === "checkbox") {
      control.checked = !!value;
      label.className = "tc-project-settings__check";
      label.prepend(control);
    } else {
      control.value = value ?? "";
      label.append(control);
    }
    form.append(label);
    return control;
  }
  function form(doc, key, title, onSubmit) {
    const value = node(doc, "form", null, { "data-form": key });
    if (title) value.append(node(doc, "h3", title));
    value.addEventListener("submit", (event) => {
      event.preventDefault();
      value.dataset.submitted = "true";
      void onSubmit(Object.fromEntries(new FormData(value)), value);
    });
    return value;
  }
  function submit(doc, value, text) {
    value.append(
      node(doc, "button", text, {
        type: "submit",
        class: "tc-button",
        "data-settings-focus": `${value.dataset.form}:submit`,
      }),
    );
  }
  function numeric(value) {
    return value === "" || value == null ? null : Number(value);
  }
  function renderSetup(doc, state, controller) {
    const fragment = doc.createDocumentFragment();
    if (state.project) {
      fragment.append(
        node(doc, "p", `Project ${state.project.identifier} created.`),
        link(
          doc,
          "Open project",
          `/${encodeURIComponent(state.project.identifier)}/overview`,
        ),
        link(
          doc,
          "Project settings",
          `/${encodeURIComponent(state.project.identifier)}/settings`,
        ),
      );
      return fragment;
    }
    const create = form(doc, "create", null, (values) =>
      controller.create({
        ...values,
        lead_user_id: numeric(values.lead_user_id),
        group_id: numeric(values.group_id),
      }),
    );
    const name = field(doc, create, "name", "Name", "", { required: true });
    const identifier = field(doc, create, "identifier", "Identifier", "", {
      required: true,
      maxlength: 5,
    });
    let touched = false;
    identifier.addEventListener("input", () => {
      touched = true;
    });
    name.addEventListener("input", () => {
      if (!touched)
        identifier.value = name.value
          .toUpperCase()
          .replace(/[^A-Z0-9]/g, "")
          .slice(0, 5);
    });
    field(doc, create, "description", "Description", "", { textarea: true });
    field(doc, create, "emoji", "Icon");
    field(doc, create, "lead_user_id", "Lead", "", {
      select: [
        [null, "Me"],
        ...state.users.map((user) => [
          user.id,
          user.display_name || user.username,
        ]),
      ],
    });
    field(doc, create, "group_id", "Project group", "", {
      select: [
        [null, "No group"],
        ...state.groups.map((group) => [group.id, group.name]),
      ],
    });
    submit(doc, create, "Create project");
    fragment.append(create);
    if (state.capabilities?.can_import)
      fragment.append(link(doc, "Import project archive", "/projects/import"));
    return fragment;
  }
  function renderArchive(doc, state, controller) {
    const archive = state.archive,
      fragment = doc.createDocumentFragment();
    if (!state.capabilities?.can_import) {
      fragment.append(
        node(
          doc,
          "p",
          "Only a signed-in instance admin can import a project archive.",
        ),
      );
      return fragment;
    }
    if (archive.phase === "unknown") {
      fragment.append(
        node(doc, "p", archive.error, { role: "alert" }),
        link(doc, "Check project list", "/projects"),
        button(doc, "I've checked the project list", () =>
          controller.acknowledgeArchive(),
        ),
      );
      return fragment;
    }
    if (archive.phase === "success") {
      const result = archive.result;
      fragment.append(
        node(doc, "h2", `${result.project.identifier} imported`),
        node(
          doc,
          "p",
          "The new project is private. You are its lead. The source project is unchanged.",
        ),
      );
      const report = node(doc, "dl");
      for (const [table, count] of Object.entries(result.report.rows)) {
        report.append(
          node(doc, "dt", table.replaceAll("_", " ")),
          node(doc, "dd", String(count)),
        );
      }
      fragment.append(
        report,
        node(doc, "p", `${result.report.blobs} files imported.`),
      );
      fragment.append(
        node(
          doc,
          "h3",
          `${result.report.external_reference_count} unresolved references`,
        ),
      );
      const refs = node(doc, "ul");
      for (const reference of result.report.external_references)
        refs.append(node(doc, "li", reference));
      fragment.append(
        refs,
        link(
          doc,
          "Open imported project",
          `/${encodeURIComponent(result.project.identifier)}/overview`,
        ),
        button(doc, "Import another archive", () =>
          controller.acknowledgeArchive(),
        ),
      );
      return fragment;
    }
    fragment.append(
      node(
        doc,
        "p",
        "Create a private project from a Lific archive. You become its lead. Accounts and permissions do not transfer; imported author names are text only.",
      ),
      node(
        doc,
        "p",
        "The source stays untouched. Import never merges or overwrites a project.",
      ),
    );
    const upload = form(doc, "archive", null, (_values, value) =>
      controller.importArchive(
        value.elements.archive.files[0],
        value.elements.confirmed.checked,
      ),
    );
    const file = field(
      doc,
      upload,
      "archive",
      "Project archive (.tar.gz)",
      "",
      { type: "file", required: true },
    );
    file.accept = ".tar.gz,application/gzip";
    const confirmation = field(
      doc,
      upload,
      "confirmed",
      "I understand this imports linked files, history and deleted content that may contain sensitive information.",
      false,
      { type: "checkbox" },
    );
    confirmation.required = true;
    upload.append(
      node(
        doc,
        "p",
        `Web upload limit: ${Math.round(state.capabilities.max_upload_bytes / 1024 / 1024)} MiB. Once processing starts, import cannot be canceled. If the connection is lost, check the project list before importing again.`,
      ),
    );
    const validation = node(doc, "p", "", { role: "alert" });
    upload.append(validation);
    file.addEventListener("change", () => {
      confirmation.checked = false;
      validation.textContent = archiveError(
        file.files[0],
        state.capabilities.max_upload_bytes,
      );
    });
    if (archive.error)
      upload.append(node(doc, "p", archive.error, { role: "alert" }));
    submit(doc, upload, "Import as private project");
    fragment.append(upload);
    return fragment;
  }
  function renderGroups(doc, state, controller) {
    const area = section(doc, "Project groups");
    const add = form(doc, "group-add", null, (values) =>
      controller.group("create", { name: values.name.trim() }),
    );
    field(doc, add, "name", "Group name", "", { required: true });
    submit(doc, add, "Create group");
    area.append(add);
    const groups = [...state.groups].sort(
      (a, b) => a.sort_order - b.sort_order || a.id - b.id,
    );
    groups.forEach((group, index) => {
      const edit = form(doc, `group-${group.id}`, null, (values) =>
        controller.group("rename", { id: group.id, name: values.name.trim() }),
      );
      field(doc, edit, "name", "Group name", group.name, { required: true });
      submit(doc, edit, "Rename group");
      const reorder = (direction) => {
        const ids = groups.map((group) => group.id);
        [ids[index], ids[index + direction]] = [
          ids[index + direction],
          ids[index],
        ];
        return controller.group("reorder", { ids });
      };
      const up = button(
        doc,
        `Move ${group.name} up`,
        () => reorder(-1),
        `group-up-${group.id}`,
      );
      up.disabled = index === 0;
      const down = button(
        doc,
        `Move ${group.name} down`,
        () => reorder(1),
        `group-down-${group.id}`,
      );
      down.disabled = index === groups.length - 1;
      edit.append(
        up,
        down,
        button(doc, `Delete ${group.name}`, () =>
          controller.group("delete", { id: group.id }),
        ),
      );
      area.append(edit);
    });
    area.append(renderProjectOrder(doc, state, controller));
    return area;
  }
  function renderProjectOrder(doc, state, controller) {
    const fragment = doc.createDocumentFragment(),
      projects = [...state.projects].sort(
        (a, b) => a.sort_order - b.sort_order || a.id - b.id,
      );
    projects.forEach((project, index) => {
      const row = node(doc, "div", null, { class: "tc-project-settings__row" });
      row.append(
        link(
          doc,
          `${project.name} · ${project.identifier}`,
          `/${encodeURIComponent(project.identifier)}/overview`,
        ),
      );
      const move = (offset) => {
        const ids = projects.map((project) => project.id);
        [ids[index], ids[index + offset]] = [ids[index + offset], ids[index]];
        return controller.group("reorder-projects", { ids });
      };
      const up = button(doc, `Move ${project.name} up`, () => move(-1)),
        down = button(doc, `Move ${project.name} down`, () => move(1));
      up.disabled = index === 0;
      down.disabled = index === projects.length - 1;
      row.append(up, down);
      fragment.append(row);
    });
    return fragment;
  }

  function renderIdentity(doc, state, controller) {
    const project = state.project,
      access = permissions(state.role),
      block = node(doc, "div", null, {
        class: "tc-project-settings__identity",
        "data-project-identity": "",
        "data-settings-identity-owner": "",
      });
    block.append(
      node(doc, "p", project.identifier, { class: "tc-dashboard__identifier" }),
    );
    const heading = node(doc, "h1");
    heading.tabIndex = -1;
    heading.dataset.dashboardFocus = "heading";
    const inline = (target, key, title, value, textarea = false) => {
      const show = () => {
        const edit = button(doc, value || `Add ${title.toLowerCase()}…`, () => {
          const entry = form(doc, `inline-${key}`, null, (values) =>
            controller.save({ [key]: values[key].trim() }),
          );
          const control = field(doc, entry, key, title, value, {
            textarea,
            required: key === "name",
          });
          submit(doc, entry, "Save");
          entry.append(button(doc, "Cancel", show));
          target.replaceChildren(entry);
          control.focus();
        });
        edit.className = "tc-project-settings__inline";
        target.replaceChildren(edit);
      };
      show();
    };
    if (access.manage) inline(heading, "name", "Name", project.name);
    else heading.textContent = project.name;
    block.append(heading);
    const description = node(doc, "div");
    if (access.manage)
      inline(
        description,
        "description",
        "Description",
        project.description,
        true,
      );
    else description.textContent = project.description;
    block.append(description);
    const icon = node(doc, "div");
    if (access.manage) inline(icon, "emoji", "Icon", project.emoji || "");
    else if (project.emoji) icon.textContent = project.emoji;
    block.append(icon);
    return block;
  }
  function renderSettings(doc, state, controller) {
    const fragment = doc.createDocumentFragment(),
      project = state.project,
      access = permissions(state.role);
    if (!access.manage)
      fragment.append(
        node(
          doc,
          "p",
          "Only a project lead or instance admin can change project settings.",
        ),
      );
    const grouping = section(doc, "Sidebar group");
    const group = form(doc, "assignment", null, (values) =>
      controller.group("assign", { group_id: numeric(values.group_id) }),
    );
    field(
      doc,
      group,
      "group_id",
      "Group",
      state.groups.find((g) => g.project_ids.includes(project.id))?.id ?? "",
      {
        select: [
          [null, "No group"],
          ...state.groups.map((g) => [g.id, g.name]),
        ],
      },
    );
    submit(doc, group, "Save group");
    grouping.append(group);
    fragment.append(grouping);
    const members = section(doc, "Project members");
    for (const member of state.members) {
      const row = node(doc, "div", null, { class: "tc-project-settings__row" });
      row.append(node(doc, "span", member.display_name || member.username));
      if (access.manage) {
        const change = form(doc, `member-${member.user_id}`, null, (values) =>
          controller.member("change", member.user_id, values.role),
        );
        field(doc, change, "role", "Role", member.role, {
          select: [
            ["viewer", "Viewer"],
            ["maintainer", "Maintainer"],
            ["lead", "Lead"],
          ],
        });
        submit(doc, change, "Save role");
        row.append(
          change,
          button(doc, `Remove ${member.username}`, () =>
            controller.member("remove", member.user_id),
          ),
        );
      } else row.append(node(doc, "span", member.role));
      members.append(row);
    }
    if (access.manage) {
      const add = form(doc, "member-add", null, (values) =>
        controller.member("add", numeric(values.user_id), values.role),
      );
      field(doc, add, "user_id", "Member", "", {
        select: state.users
          .filter(
            (user) =>
              !state.members.some((member) => member.user_id === user.id),
          )
          .map((user) => [user.id, user.display_name || user.username]),
        required: true,
      });
      field(doc, add, "role", "Role", "viewer", {
        select: [
          ["viewer", "Viewer"],
          ["maintainer", "Maintainer"],
          ["lead", "Lead"],
        ],
      });
      submit(doc, add, "Add member");
      members.append(add);
    }
    fragment.append(members);
    const labels = section(doc, "Labels");
    const labelForm = (label) => {
      const edit = form(
        doc,
        label ? `label-${label.id}` : "label-add",
        null,
        (values) =>
          controller.label(label ? "update" : "create", {
            id: label?.id,
            ...values,
          }),
      );
      field(doc, edit, "name", "Label name", label?.name || "", {
        required: true,
      });
      const color = field(
        doc,
        edit,
        "color",
        "Color",
        safeColor(label?.color || "#6B7280"),
        { required: true },
      );
      const swatches = node(doc, "div", null, {
        class: "tc-project-settings__swatches",
      });
      for (const [name, value] of PALETTE) {
        const swatch = button(
          doc,
          name,
          () => {
            color.value = value;
          },
          `${edit.dataset.form}:${name}`,
        );
        swatch.className = "tc-project-settings__swatch";
        swatch.style.backgroundColor = value;
        swatch.textContent = "";
        swatch.setAttribute("aria-label", name);
        swatches.append(swatch);
      }
      edit.append(swatches);
      submit(doc, edit, label ? "Save label" : "Create label");
      return edit;
    };
    for (const label of state.labels) {
      const row = node(doc, "div", null, { class: "tc-project-settings__row" }),
        title = node(doc, "span", label.name, {
          class: "tc-project-settings__label",
        });
      title.style.color = safeColor(label.color);
      row.append(title);
      if (access.edit) {
        row.append(
          labelForm(label),
          button(doc, `Delete ${label.name}`, () =>
            controller.label("delete", { id: label.id }),
          ),
        );
        if (state.labels.length > 1) {
          const merge = form(doc, `merge-${label.id}`, null, (values) =>
            controller.label("merge", {
              id: label.id,
              into: numeric(values.into),
            }),
          );
          field(doc, merge, "into", "Merge into", "", {
            select: state.labels
              .filter((other) => other.id !== label.id)
              .map((other) => [other.id, other.name]),
          });
          submit(doc, merge, "Merge label");
          row.append(merge);
        }
      }
      labels.append(row);
    }
    if (access.edit) labels.append(labelForm(null));
    fragment.append(labels);
    if (access.manage) {
      const github = section(doc, "Import GitHub issues");
      github.append(
        node(
          doc,
          "p",
          "Pull requests are skipped. Re-running does not duplicate issues that were already imported.",
        ),
      );
      const importForm = form(doc, "github", null, (values) =>
        controller.importGithub({
          ...values,
          token: values.token.trim() || undefined,
          dry_run: true,
        }),
      );
      field(doc, importForm, "repo", "Repository (owner/repository)", "", {
        required: true,
      });
      field(doc, importForm, "token", "GitHub token (optional)", "", {
        type: "password",
        autocomplete: "off",
      });
      field(doc, importForm, "state", "Issues", "all", {
        select: [
          ["all", "All"],
          ["open", "Open"],
          ["closed", "Closed"],
        ],
      });
      const statuses = ["backlog", "todo", "active", "done", "cancelled"].map(
        (v) => [v, v],
      );
      field(doc, importForm, "map_open", "Open issue status", "backlog", {
        select: statuses,
      });
      field(doc, importForm, "map_closed", "Closed issue status", "done", {
        select: statuses,
      });
      submit(doc, importForm, "Preview import");
      github.append(importForm);
      if (state.summary) {
        const summary = node(doc, "dl");
        for (const [key, value] of Object.entries(state.summary)) {
          if (key !== "dry_run") {
            summary.append(
              node(doc, "dt", key.replaceAll("_", " ")),
              node(doc, "dd", String(value)),
            );
          }
        }
        github.append(summary);
        if (state.summary.dry_run) {
          for (const control of importForm.elements) control.disabled = true;
          github.append(
            button(doc, "Run import", () => controller.confirmGithub()),
            button(doc, "Edit import configuration", () =>
              controller.resetGithub(),
            ),
          );
        }
      }
      fragment.append(github);
      const danger = section(doc, "Danger zone");
      const rename = form(doc, "identifier", null, (values) =>
        controller.save({ identifier: values.identifier }),
      );
      field(
        doc,
        rename,
        "identifier",
        "Project identifier",
        project.identifier,
        { required: true, maxlength: 5 },
      );
      submit(doc, rename, "Rename identifier");
      danger.append(rename);
      const lead = form(doc, "lead", null, (values) =>
        controller.save({ lead_user_id: numeric(values.lead_user_id) }),
      );
      field(doc, lead, "lead_user_id", "Project lead", project.lead_user_id, {
        select: [
          [null, "No lead"],
          ...state.users.map((user) => [
            user.id,
            user.display_name || user.username,
          ]),
        ],
      });
      submit(doc, lead, "Change lead");
      danger.append(lead);
      const deletion = form(doc, "delete", null, (values) =>
        controller.deleteProject(values.confirmation),
      );
      field(
        doc,
        deletion,
        "confirmation",
        `Type ${project.identifier} to delete this project`,
        "",
        { required: true },
      );
      submit(doc, deletion, "Delete project");
      danger.append(
        deletion,
        button(doc, "Export project data", () => controller.exportData()),
      );
      fragment.append(danger);
    }
    if (
      state.role?.is_admin ||
      state.role?.role === "lead" ||
      project.lead_user_id === controller.env.userId?.()
    ) {
      const publication = section(doc, "Public view");
      publication.append(
        node(
          doc,
          "p",
          project.is_public
            ? "Current issues, pages, comments, authors and files are readable by anyone without an account."
            : "Publishing exposes all current issues, pages, comments, authors and attached files to everyone.",
        ),
      );
      const publish = form(doc, "publication", null, (_values, value) => {
        if (project.is_public || value.elements.confirmed.checked)
          return controller.save({ is_public: !project.is_public });
      });
      if (!project.is_public)
        field(
          doc,
          publish,
          "confirmed",
          "I understand this makes existing project content public.",
          false,
          { type: "checkbox", required: true },
        );
      submit(
        doc,
        publish,
        project.is_public ? "Turn off public access" : "Publish project",
      );
      publication.append(publish);
      fragment.append(publication);
    }
    if (state.capabilities && controller.canExportArchive()) {
      const archive = section(doc, "Project archive");
      archive.append(
        node(
          doc,
          "p",
          "Copy the complete project to another Lific instance. Archives contain history, deleted content and author names. Accounts and permissions do not transfer.",
        ),
      );
      const transfer = form(doc, "download", null, (_values, value) =>
        controller.exportArchive(value.elements.confirmed.checked),
      );
      field(
        doc,
        transfer,
        "confirmed",
        "I understand this archive includes history and deleted content.",
        false,
        { type: "checkbox", required: true },
      );
      submit(doc, transfer, "Download project archive");
      archive.append(transfer);
      fragment.append(archive);
    }
    fragment.append(renderGroups(doc, state, controller));
    return fragment;
  }
  function attach(root, options = {}) {
    const doc = root.ownerDocument,
      win = options.win || doc.defaultView,
      session = options.session || win.lificSession;
    const identity = () =>
      session.state.publicProject === null && session.state.user
        ? `private:${session.state.user.id}`
        : null;
    const storage = (() => {
      try {
        return win.sessionStorage;
      } catch {
        return null;
      }
    })();
    const env = {
      identity,
      userId: () => session.state.user?.id,
      storage,
      request: (path, request = {}) =>
        session.request(path, {
          ...request,
          body:
            request.body === undefined
              ? undefined
              : JSON.stringify(request.body),
        }),
      navigate: (path) => win.location.assign(path),
      reauthenticate: async (password) => {
        const userId = session.state.user?.id;
        const result = await session.request("/auth/me/refresh", {
          method: "POST",
          body: JSON.stringify(password ? { password } : {}),
        });
        if (result.ok && result.data.user.id === userId) {
          session.saveSession(result.data.token);
          await session.bootstrap();
          return { ok: true };
        }
        return {
          ok: false,
          error:
            result.error ||
            "The refreshed session belongs to a different account.",
        };
      },
      download: async (path, filename) => {
        const token = win.localStorage.getItem("lific_token"),
          owner = identity();
        try {
          const response = await win.fetch("/api" + path, {
            headers: token ? { Authorization: `Bearer ${token}` } : {},
            cache: "no-store",
          });
          if (
            owner !== identity() ||
            token !== win.localStorage.getItem("lific_token")
          )
            return { ok: false, error: "Your account changed." };
          if (!response.ok) {
            const body = await response.json();
            return {
              ok: false,
              error: body.error || `HTTP ${response.status}`,
            };
          }
          const blob = await response.blob();
          return owner === identity() &&
            token === win.localStorage.getItem("lific_token")
            ? {
                ok: true,
                data: {
                  blob,
                  filename:
                    filename ||
                    `${controller.state.project.identifier}.lific.tar.gz`,
                },
              }
            : { ok: false, error: "Your account changed." };
        } catch {
          return {
            ok: false,
            error:
              "The archive download stopped. Check your connection and download it again.",
          };
        }
      },
      saveDownload: ({ blob, filename }) => {
        const url = win.URL.createObjectURL(blob),
          anchor = node(doc, "a", null, { href: url, download: filename });
        doc.body.append(anchor);
        anchor.click();
        anchor.remove();
        win.URL.revokeObjectURL(url);
      },
      upload: (file, onProgress, onProcessing) =>
        new Promise((resolve) => {
          const xhr = new win.XMLHttpRequest(),
            token = win.localStorage.getItem("lific_token"),
            owner = identity();
          xhr.open("POST", "/api/project-archives");
          if (token) xhr.setRequestHeader("Authorization", `Bearer ${token}`);
          xhr.upload.onprogress = (event) =>
            onProgress(
              event.lengthComputable
                ? Math.min(100, Math.round((event.loaded / event.total) * 100))
                : null,
            );
          xhr.upload.onload = onProcessing;
          const unknown = () =>
            resolve({
              ok: false,
              status: null,
              error: "The archive import outcome is unknown.",
            });
          xhr.onerror = unknown;
          xhr.onabort = unknown;
          xhr.ontimeout = unknown;
          xhr.onload = () => {
            if (
              owner !== identity() ||
              token !== win.localStorage.getItem("lific_token")
            ) {
              unknown();
              return;
            }
            try {
              const body = JSON.parse(xhr.responseText);
              if (xhr.status === 201 && body.project && body.report)
                resolve({ ok: true, data: body });
              else if (
                xhr.status >= 400 &&
                xhr.status < 500 &&
                typeof body.error === "string"
              )
                resolve({ ok: false, status: xhr.status, error: body.error });
              else unknown();
            } catch {
              unknown();
            }
          };
          const body = new win.FormData();
          body.append("archive", file, file.name);
          try {
            xhr.send(body);
          } catch {
            unknown();
          }
        }),
    };
    const content = root.querySelector("[data-project-settings-content]"),
      status = root.querySelector("[data-project-settings-status]"),
      error = root.querySelector("[data-project-settings-error]"),
      warning = root.querySelector("[data-project-settings-warning]");
    let previous = null,
      audience = identity(),
      disposed = false,
      identityNode = null;
    const installIdentity = () => {
      const target = doc.querySelector("[data-project-identity]");
      if (controller.state.status !== "ready" && identityNode) {
        const blank = node(doc, "div", null, { "data-project-identity": "" });
        identityNode.replaceWith(blank);
        identityNode = null;
      }
      if (
        target &&
        controller.state.status === "ready" &&
        controller.state.project &&
        (target !== identityNode ||
          identityNode?.dataset.settingsProjectVersion !==
            JSON.stringify([controller.state.project, controller.state.role]))
      ) {
        const replacement = renderIdentity(doc, controller.state, controller);
        replacement.dataset.settingsProjectVersion = JSON.stringify([
          controller.state.project,
          controller.state.role,
        ]);
        identityNode = replacement;
        target.replaceWith(replacement);
      }
    };
    const render = (state) => {
      root.setAttribute(
        "aria-busy",
        String(state.busy || state.refreshing || state.status === "loading"),
      );
      status.textContent =
        state.archive.phase === "uploading"
          ? `Uploading… ${state.archive.progress ?? ""}${state.archive.progress === null ? "" : "%"}`
          : state.archive.phase === "processing"
            ? "Importing…"
            : state.refreshing
              ? "Refreshing project access…"
              : state.busy
                ? "Saving…"
                : state.status === "loading"
                  ? "Loading project access…"
                  : "";
      if (state.archive.phase === "uploading") {
        const progress = node(doc, "progress", null, {
          max: 100,
          "aria-label": "Archive upload",
        });
        if (state.archive.progress !== null)
          progress.value = state.archive.progress;
        status.append(progress);
      }
      error.textContent = state.error;
      warning.textContent = state.warning;
      if ((state.busy || state.refreshing) && previous?.status === "ready") {
        const controls = [
          ...content.querySelectorAll("input,select,textarea,button"),
          ...(identityNode?.querySelectorAll("input,textarea,button") || []),
        ];
        for (const control of controls) {
          if (!control.hasAttribute("data-settings-disabled"))
            control.dataset.settingsDisabled = String(control.disabled);
          control.disabled = true;
        }
        previous = state;
        return;
      }
      const active = doc.activeElement,
        focus = root.contains(active) ? active.dataset.settingsFocus : null;
      const drafts = new Map(
        [...content.querySelectorAll("form")]
          .filter(
            (value) =>
              value.dataset.form !== "assignment" &&
              (!value.dataset.submitted || state.error || state.needsAuth),
          )
          .map((value) => [
            value.dataset.form,
            [...value.elements]
              .filter(
                (control) =>
                  control.name &&
                  control.type !== "file" &&
                  control.type !== "password",
              )
              .map((control) => ({
                name: control.name,
                value: control.value,
                checked: control.checked,
              })),
          ]),
      );
      if (state.status === "ready") {
        const view = {
          new: renderSetup,
          archive: renderArchive,
          settings: renderSettings,
        }[root.dataset.topcoatProjectSettings];
        content.replaceChildren(view(doc, state, controller));
        if (state.project) {
          doc.body.dataset.lificProjectId = String(state.project.id);
          win.lificSync?.setActiveProject?.(state.project.id);
        }
        for (const formElement of content.querySelectorAll("form"))
          for (const draft of drafts.get(formElement.dataset.form) || []) {
            const control = formElement.elements[draft.name];
            if (control) {
              control.value = draft.value;
              control.checked = draft.checked;
            }
          }
        if (state.needsAuth) {
          const reauth = form(
            doc,
            "reauth",
            "Confirm your password",
            (values) => controller.resume(values.password),
          );
          field(doc, reauth, "password", "Password", "", {
            type: "password",
            autocomplete: "current-password",
          });
          submit(doc, reauth, "Confirm and retry");
          reauth.append(button(doc, "Cancel", () => controller.cancelReauth()));
          content.prepend(reauth);
        }
      } else if (state.status === "error") {
        content.replaceChildren(
          button(doc, "Try again", () => controller.load()),
        );
      } else content.replaceChildren();
      if (focus)
        [...content.querySelectorAll("[data-settings-focus]")]
          .find((value) => value.dataset.settingsFocus === focus)
          ?.focus();
      for (const control of identityNode?.querySelectorAll(
        "[data-settings-disabled]",
      ) || []) {
        control.disabled = control.dataset.settingsDisabled === "true";
        delete control.dataset.settingsDisabled;
      }
      previous = state;
      installIdentity();
    };
    env.onChange = render;
    const controller = new ProjectSettingsController(
      env,
      root.dataset.projectIdentifier || null,
    );
    const transition = () => {
      const next = identity();
      if (next !== audience && !controller.reauthenticating) {
        audience = next;
        controller.accountChanged();
        if (next !== null) void controller.load();
      }
    };
    const listeners = [
      "lific:session-change",
      "lific:account-change",
      "lific:scope-change",
    ].map((name) => {
      win.addEventListener(name, transition);
      return () => win.removeEventListener(name, transition);
    });
    const listen = (name, callback) => {
      win.addEventListener(name, callback);
      listeners.push(() => win.removeEventListener(name, callback));
    };
    listen("lific:realtime", (event) => {
      void controller.handleEvent(event.detail || {});
    });
    listen("lific:project-catalog", () => {
      void controller.handleEvent({ type: "catalog.changed" });
    });
    listen("lific:sync-change", () => {
      void controller.connectionChanged(
        win.lificSync?.state.connected === true,
      );
    });
    for (const name of ["online", "focus"])
      listen(name, () => {
        void controller.refresh();
      });
    const storageChange = (event) => {
      if (event.key === "lific_token" || event.key === null) transition();
    };
    win.addEventListener("storage", storageChange);
    const observer = new win.MutationObserver(() => {
      if (!disposed) installIdentity();
    });
    observer.observe(doc.body, { childList: true, subtree: true });
    void controller.load();
    return {
      controller,
      refresh: () => controller.refresh(),
      dispose() {
        if (disposed) return;
        disposed = true;
        observer.disconnect();
        controller.dispose();
        listeners.forEach((remove) => remove());
        win.removeEventListener("storage", storageChange);
      },
    };
  }
  const api = {
    PALETTE,
    identifierError,
    safeColor,
    normalizeColor,
    archiveError,
    permissions,
    ProjectSettingsController,
    attach,
    mount: attach,
  };
  if (typeof module !== "undefined") module.exports = api;
  globalThis.LificTopcoatProjectSettings = api;
  if (typeof window !== "undefined") {
    const root = document.querySelector("[data-topcoat-project-settings]");
    if (root && window.lificSession) window.lificProjectSettings = attach(root);
  }
})();
