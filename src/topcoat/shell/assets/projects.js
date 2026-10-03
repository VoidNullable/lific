/* The server owns catalog state. This controller renders snapshots and sends
 * typed commands through the shell adapter; it never edits browser history. */
(() => {
  const GROUPS_KEY = 'lific:project-tree:disclosures';
  const byId = (rows, id) => rows.find(row => row.id === id);

  function normalizeCatalog(snapshot, scope = 'private') {
    if (scope === 'public') return {generation: snapshot.generation, groups: [], projects: []};
    const projects = Array.isArray(snapshot?.projects) ? snapshot.projects : [];
    const projectIds = new Set(projects.map(project => project.id));
    const groups = Array.isArray(snapshot?.groups) ? snapshot.groups.map(group => ({
      ...group,
      project_ids: (group.project_ids ?? []).filter(id => projectIds.has(id)),
    })) : [];
    return {generation: snapshot?.generation ?? 0, groups, projects};
  }

  function visibleProjects(snapshot) {
    const grouped = new Set();
    const rows = [];
    for (const group of snapshot.groups) {
      for (const project of snapshot.projects) {
        if (group.project_ids.includes(project.id) && !grouped.has(project.id)) {
          grouped.add(project.id);
          rows.push({groupId: group.id, projectId: project.id});
        }
      }
    }
    for (const project of snapshot.projects) {
      if (!grouped.has(project.id)) rows.push({groupId: null, projectId: project.id});
    }
    return rows;
  }

  function moveProject(snapshot, projectId, groupId, delta) {
    const siblings = groupId === null
      ? snapshot.projects.filter(project =>
        !snapshot.groups.some(group => group.project_ids.includes(project.id)))
      : snapshot.projects.filter(project =>
        byId(snapshot.groups, groupId)?.project_ids.includes(project.id));
    const index = siblings.findIndex(project => project.id === projectId);
    const destination = siblings[index + delta];
    if (index < 0 || !destination) return null;
    return {groupId, beforeProjectId: delta < 0 ? destination.id : siblings[index + 2]?.id ?? null};
  }

  function moveBy(ids, id, delta) {
    const result = [...ids];
    const index = result.indexOf(id);
    const target = index + delta;
    if (index < 0 || target < 0 || target >= result.length) return result;
    result.splice(target, 0, result.splice(index, 1)[0]);
    return result;
  }

  function moveBefore(ids, id, beforeId) {
    const result = [...ids];
    const index = result.indexOf(id);
    if (index < 0 || !result.includes(beforeId) || id === beforeId) return result;
    result.splice(index, 1);
    result.splice(result.indexOf(beforeId), 0, id);
    return result;
  }

  function mergeProjectOrder(snapshot, groupId, reordered) {
    const all = snapshot.projects.map(project => project.id);
    const groupIds = new Set(groupId === null
      ? all.filter(id => !snapshot.groups.some(group => group.project_ids.includes(id)))
      : (byId(snapshot.groups, groupId)?.project_ids ?? []));
    let next = 0;
    return all.map(id => groupIds.has(id) ? reordered[next++] : id);
  }

  class CatalogController {
    constructor(adapter = {}) {
      this.adapter = adapter;
      this.scope = adapter.scope ?? 'private';
      this.snapshot = this.normalize(adapter.current?.() ?? {generation: 0, groups: [], projects: []});
      this.error = null;
      this.pending = false;
      this.request = 0;
      this.mutation = 0;
      this.disconnected = false;
      this.listeners = new Set();
      this.unsubscribe = null;
    }

    normalize(snapshot) { return normalizeCatalog(snapshot, this.scope); }

    subscribe(listener) {
      this.listeners.add(listener);
      return () => this.listeners.delete(listener);
    }

    changed() {
      if (this.disconnected) return;
      for (const listener of this.listeners) listener(this.snapshot, this.error);
      const target = this.adapter.eventTarget;
      const EventType = target?.CustomEvent ?? globalThis.CustomEvent;
      if (target?.dispatchEvent && EventType) {
        target.dispatchEvent(new EventType('lific:project-catalog', {detail: this.snapshot}));
      }
    }

    accept(snapshot) {
      if (this.disconnected) return false;
      const next = this.normalize(snapshot);
      if (next.generation <= this.snapshot.generation) return false;
      this.snapshot = next;
      this.error = null;
      this.changed();
      return true;
    }

    async refresh() {
      if (this.disconnected || this.scope === 'public' || !this.adapter.fetch) return false;
      const request = ++this.request;
      try {
        const snapshot = await this.adapter.fetch();
        if (request !== this.request) return false;
        return this.accept(snapshot);
      } catch (error) {
        if (request === this.request) {
          this.error = error?.message ?? 'Could not refresh projects.';
          this.changed();
        }
        return false;
      }
    }

    connect() {
      this.disconnected = false;
      if (this.scope === 'public' || !this.adapter.subscribe || this.unsubscribe) return;
      this.unsubscribe = this.adapter.subscribe(snapshot => this.accept(snapshot));
    }

    disconnect() {
      this.disconnected = true;
      ++this.mutation;
      ++this.request;
      this.pending = false;
      this.unsubscribe?.();
      this.unsubscribe = null;
      this.listeners.clear();
    }

    async command(command, optimistic) {
      if (this.scope === 'public') throw new Error('Project changes are unavailable.');
      if (!this.adapter.command) throw new Error('Project changes are unavailable.');
      if (this.pending) throw new Error('Another project change is still saving.');
      const previous = this.snapshot;
      const operation = ++this.mutation;
      ++this.request;
      this.snapshot = this.normalize(optimistic(previous));
      this.error = null;
      this.pending = true;
      this.changed();
      try {
        const outcome = await this.adapter.command(command);
        if (this.disconnected || operation !== this.mutation) return outcome;
        if (outcome?.snapshot) this.accept(outcome.snapshot);
        else if (this.adapter.fetch) await this.refresh();
        if (this.disconnected || operation !== this.mutation) return outcome;
        this.pending = false;
        this.changed();
        return outcome;
      } catch (error) {
        if (this.disconnected || operation !== this.mutation) return;
        if (this.snapshot.generation === previous.generation) this.snapshot = previous;
        this.error = error?.message ?? 'Could not save project changes.';
        this.pending = false;
        this.changed();
        throw error;
      }
    }

    reorderGroups(ids) {
      return this.command({type: 'reorder_groups', ids}, snapshot => ({...snapshot,
        groups: ids.map(id => byId(snapshot.groups, id)).filter(Boolean)}));
    }

    reorderProjects(ids) {
      return this.command({type: 'reorder_projects', ids}, snapshot => {
        const byProjectId = new Map(snapshot.projects.map(project => [project.id, project]));
        const reordered = ids.map(id => byProjectId.get(id)).filter(Boolean);
        let next = 0;
        const projects = snapshot.projects.map(project =>
          ids.includes(project.id) ? reordered[next++] : project);
        const groups = snapshot.groups.map(group => ({...group,
          project_ids: projects.filter(project => group.project_ids.includes(project.id))
            .map(project => project.id)}));
        return {...snapshot, projects, groups};
      });
    }

    assignProject(projectId, groupId) {
      return this.command({type: 'assign_project', project_id: projectId, group_id: groupId}, snapshot => {
        const groups = snapshot.groups.map(group => ({...group,
          project_ids: group.project_ids.filter(id => id !== projectId)}));
        if (groupId !== null) {
          const destination = groups.find(group => group.id === groupId);
          if (destination) destination.project_ids.push(projectId);
        }
        return {...snapshot, groups};
      });
    }

    deleteGroup(groupId) {
      return this.command({type: 'delete_group', id: groupId}, snapshot => ({...snapshot,
        // Deleting a group only unassigns its projects. Projects remain in the catalog.
        groups: snapshot.groups.filter(group => group.id !== groupId)}));
    }

    createGroup(name) {
      return this.command({type: 'create_group', name}, snapshot => snapshot);
    }

    renameGroup(id, name) {
      return this.command({type: 'rename_group', id, name}, snapshot => ({...snapshot,
        groups: snapshot.groups.map(group => group.id === id ? {...group, name} : group)}));
    }
  }

  function element(document, tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function storageFor(document, options) {
    try { return options.storage ?? document.defaultView?.localStorage; }
    catch { return undefined; }
  }

  function focusedControl(root) {
    const active = root.ownerDocument.activeElement;
    if (!active || !root.contains(active)) return null;
    const group = active.closest?.('[data-group-id]');
    return {
      action: active.dataset?.action,
      delta: active.dataset?.delta,
      groupId: group?.dataset.groupId,
      projectId: active.dataset?.projectId,
      name: active.name,
      tagName: active.tagName,
    };
  }

  function restoreFocus(root, descriptor) {
    if (!descriptor) return;
    const target = [...root.querySelectorAll('button, input, a')].find(node => {
      const group = node.closest?.('[data-group-id]');
      return node.tagName === descriptor.tagName && node.dataset?.action === descriptor.action
        && node.dataset?.delta === descriptor.delta && node.dataset?.projectId === descriptor.projectId
        && node.name === descriptor.name && group?.dataset.groupId === descriptor.groupId;
    });
    target?.focus({preventScroll: true});
  }

  function draftKey(name, groupId) { return `${groupId ?? ''}\u0000${name}`; }

  function captureDrafts(root, drafts, renderedValues, selections, skipped = new Set()) {
    for (const input of root.querySelectorAll('input')) {
      const key = draftKey(input.name, input.closest?.('[data-group-id]')?.dataset.groupId);
      if (skipped.has(key)) {
        drafts.delete(key);
        selections.delete(key);
        continue;
      }
      if (input.selectionStart !== null && input.selectionStart !== undefined) {
        selections.set(key, {start: input.selectionStart, end: input.selectionEnd,
          direction: input.selectionDirection});
      }
      const dirty = input.value !== renderedValues.get(key);
      if (dirty) drafts.set(key, {value: input.value, dirty: true});
      else drafts.delete(key);
    }
  }

  function restoreDrafts(root, drafts, selections) {
    for (const input of root.querySelectorAll('input')) {
      const key = draftKey(input.name, input.closest?.('[data-group-id]')?.dataset.groupId);
      const draft = drafts.get(key);
      if (draft?.dirty) input.value = draft.value;
      const selection = selections.get(key);
      if (selection && selection.start !== null && selection.end !== null) {
        try { input.setSelectionRange(selection.start, selection.end, selection.direction ?? 'none'); } catch { /* Unsupported input type. */ }
      }
    }
  }

  function rememberRenderedValues(root, renderedValues, drafts) {
    const retained = new Set();
    for (const input of root.querySelectorAll('input')) {
      const key = draftKey(input.name, input.closest?.('[data-group-id]')?.dataset.groupId);
      retained.add(key);
      if (!drafts.has(key)) renderedValues.set(key, input.value);
    }
    for (const key of renderedValues.keys()) if (!retained.has(key)) renderedValues.delete(key);
    for (const key of drafts.keys()) if (!retained.has(key)) drafts.delete(key);
  }

  function render(root, controller, options = {}) {
    const focus = focusedControl(root);
    if (controller.scope === 'public') {
      root.replaceChildren();
      return;
    }
    const document = root.ownerDocument;
    const snapshot = controller.snapshot;
    const expanded = options.expanded ?? readExpanded(storageFor(document, options),
      snapshot.groups.map(group => group.id));
    const tree = element(document, 'section', 'tc-projects');
    tree.setAttribute('aria-label', 'Projects');
    tree.setAttribute('aria-busy', String(controller.pending));
    const title = element(document, 'h2', 'tc-projects__heading', 'Projects');
    tree.append(title);
    const create = element(document, 'form', 'tc-projects__create');
    const createName = element(document, 'input', 'tc-projects__input');
    createName.name = 'group-name';
    createName.type = 'text';
    createName.maxLength = 80;
    createName.required = true;
    createName.setAttribute('aria-label', 'New group name');
    const createButton = element(document, 'button', 'tc-projects__action', 'Create group');
    createButton.type = 'submit';
    create.append(createName, createButton);
    tree.append(create);
    const error = element(document, 'p', 'tc-projects__error', controller.error ?? '');
    error.setAttribute('role', 'status');
    error.hidden = !controller.error;
    tree.append(error);
    const groupList = element(document, 'div', 'tc-projects__groups');
    for (const group of snapshot.groups) {
      const item = element(document, 'section', 'tc-projects__group');
      item.dataset.groupId = String(group.id);
      item.draggable = true;
      const header = element(document, 'div', 'tc-projects__group-heading');
      const disclosure = element(document, 'button', 'tc-projects__disclosure',
        expanded.has(String(group.id)) ? '▾' : '▸');
      disclosure.type = 'button';
      disclosure.setAttribute('aria-label', `${expanded.has(String(group.id)) ? 'Collapse' : 'Expand'} ${group.name}`);
      disclosure.setAttribute('aria-expanded', String(expanded.has(String(group.id))));
      disclosure.dataset.action = 'disclose';
      const name = element(document, 'span', 'tc-projects__group-name', group.name);
      const actions = element(document, 'div', 'tc-projects__group-actions');
      for (const [delta, label] of [[-1, `Move ${group.name} up`], [1, `Move ${group.name} down`]]) {
        const move = element(document, 'button', 'tc-projects__action', delta < 0 ? '↑' : '↓');
        move.type = 'button';
        move.dataset.action = 'move-group';
        move.dataset.delta = String(delta);
        move.setAttribute('aria-label', label);
        actions.append(move);
      }
      const renameForm = element(document, 'form', 'tc-projects__rename');
      renameForm.dataset.action = 'rename-group';
      const renameName = element(document, 'input', 'tc-projects__input');
      renameName.name = 'name';
      renameName.type = 'text';
      renameName.maxLength = 80;
      renameName.required = true;
      renameName.value = group.name;
      renameName.setAttribute('aria-label', `Rename ${group.name}`);
      const rename = element(document, 'button', 'tc-projects__action', 'Rename');
      rename.type = 'submit';
      const remove = element(document, 'button', 'tc-projects__action', 'Delete group');
      remove.type = 'button';
      remove.dataset.action = 'delete-group';
      renameForm.append(renameName, rename, remove);
      actions.append(renameForm);
      header.append(disclosure, name, actions);
      item.append(header);
      const list = element(document, 'div', 'tc-projects__project-list');
      list.hidden = !expanded.has(String(group.id));
      for (const project of snapshot.projects) {
        if (group.project_ids.includes(project.id)) {
          list.append(projectLink(document, project, options.activeIdentifier));
        }
      }
      item.append(list);
      groupList.append(item);
    }
    const ungrouped = element(document, 'section', 'tc-projects__group tc-projects__group--ungrouped');
    ungrouped.dataset.groupId = '';
    ungrouped.append(element(document, 'h3', 'tc-projects__group-name', 'Ungrouped'));
    const list = element(document, 'div', 'tc-projects__project-list');
    for (const row of visibleProjects(snapshot).filter(row => row.groupId === null)) {
      const project = byId(snapshot.projects, row.projectId);
      if (project) list.append(projectLink(document, project, options.activeIdentifier));
    }
    ungrouped.append(list);
    groupList.append(ungrouped);
    tree.append(groupList);
    restoreDrafts(tree, options.drafts ?? new Map(), options.selections ?? new Map());
    root.replaceChildren(tree);
    if (controller.pending) {
      for (const control of tree.querySelectorAll('button, input')) control.setAttribute('aria-disabled', 'true');
    }
    restoreFocus(root, focus);
  }

  function projectLink(document, project, activeIdentifier) {
    const link = element(document, 'a', 'tc-projects__project');
    link.href = `/${encodeURIComponent(project.identifier)}/overview`;
    link.dataset.projectId = String(project.id);
    link.draggable = true;
    if (project.identifier.toUpperCase() === activeIdentifier?.toUpperCase()) {
      link.setAttribute('aria-current', 'page');
    }
    const label = project.emoji ? `${project.emoji} ${project.name}` : project.name;
    link.textContent = label;
    link.setAttribute('aria-label', `Open ${project.name}`);
    return link;
  }

  function readExpanded(storage, defaults = []) {
    try {
      const saved = storage?.getItem(GROUPS_KEY);
      if (saved === null || saved === undefined) return new Set(defaults.map(String));
      const ids = JSON.parse(saved);
      return new Set(Array.isArray(ids) ? ids.map(String) : []);
    } catch { return new Set(); }
  }

  function attach(root, adapter, options = {}) {
    const controller = new CatalogController({...adapter,
      eventTarget: adapter.eventTarget ?? root.ownerDocument.defaultView});
    const storage = storageFor(root.ownerDocument, options);
    const expanded = readExpanded(storage, controller.snapshot.groups.map(group => group.id));
    const drafts = new Map();
    const selections = new Map();
    const renderedValues = new Map();
    const knownGroups = new Set(controller.snapshot.groups.map(group => String(group.id)));
    const draw = ({skipDraftKeys = []} = {}) => {
      captureDrafts(root, drafts, renderedValues, selections, new Set(skipDraftKeys));
      const currentGroups = new Set(controller.snapshot.groups.map(group => String(group.id)));
      for (const id of knownGroups) if (!currentGroups.has(id)) expanded.delete(id);
      for (const id of currentGroups) if (!knownGroups.has(id)) expanded.add(id);
      knownGroups.clear();
      for (const id of currentGroups) knownGroups.add(id);
      render(root, controller, {...options, expanded, drafts, selections});
      restoreDrafts(root, drafts, selections);
      rememberRenderedValues(root, renderedValues, drafts);
    };
    const unsubscribe = controller.subscribe(draw);
    controller.connect();
    draw();
    controller.refresh();
    root.addEventListener('click', event => {
      if (controller.pending) return;
      const button = event.target.closest?.('[data-action="disclose"]');
      if (button) {
        const group = button.closest('[data-group-id]');
        const id = group.dataset.groupId;
        if (expanded.has(id)) expanded.delete(id); else expanded.add(id);
        try { storage?.setItem(GROUPS_KEY, JSON.stringify([...expanded])); } catch { /* Optional preference. */ }
        draw();
        return;
      }
      const remove = event.target.closest?.('[data-action="delete-group"]');
      if (remove) controller.deleteGroup(Number(remove.closest('[data-group-id]').dataset.groupId)).catch(() => {});
      const move = event.target.closest?.('[data-action="move-group"]');
      if (move) {
        const id = Number(move.closest('[data-group-id]').dataset.groupId);
        const index = controller.snapshot.groups.findIndex(group => group.id === id);
        const target = index + Number(move.dataset.delta);
        if (target < 0 || target >= controller.snapshot.groups.length) return;
        const ids = moveBy(controller.snapshot.groups.map(group => group.id), id,
          Number(move.dataset.delta));
        controller.reorderGroups(ids).catch(() => {});
      }
    });
    root.addEventListener('submit', event => {
      event.preventDefault();
      if (controller.pending) return;
      const form = event.target;
      const data = new FormData(form);
      if (form.matches?.('[data-action="rename-group"]')) {
        const id = Number(form.closest('[data-group-id]').dataset.groupId);
        controller.renameGroup(id, String(data.get('name') ?? '').trim())
          .then(() => draw({skipDraftKeys: [draftKey('name', String(id))]})).catch(() => {});
      } else if (form.matches?.('.tc-projects__create')) {
        controller.createGroup(String(data.get('group-name') ?? '').trim())
          .then(() => draw({skipDraftKeys: [draftKey('group-name')]})).catch(() => {});
      }
    });
    root.addEventListener('keydown', event => {
      if (controller.pending) return;
      if (!event.altKey || !['ArrowUp', 'ArrowDown'].includes(event.key)) return;
      const link = event.target.closest?.('[data-project-id]');
      if (!link) return;
      const row = visibleProjects(controller.snapshot).find(item => item.projectId === Number(link.dataset.projectId));
      if (!row) return;
      const ids = row.groupId === null
        ? controller.snapshot.projects
          .filter(project => !controller.snapshot.groups.some(group => group.project_ids.includes(project.id)))
          .map(project => project.id)
        : controller.snapshot.projects
          .filter(project => byId(controller.snapshot.groups, row.groupId)?.project_ids.includes(project.id))
          .map(project => project.id);
      const index = ids.indexOf(row.projectId);
      const target = index + (event.key === 'ArrowUp' ? -1 : 1);
      if (target < 0 || target >= ids.length) return;
      event.preventDefault();
      const reordered = moveBy(ids, row.projectId, target - index);
      controller.reorderProjects(mergeProjectOrder(controller.snapshot, row.groupId, reordered)).catch(() => {});
    });
    let draggedProject = null;
    let draggedGroup = null;
    root.addEventListener('dragstart', event => {
      if (controller.pending) return;
      const link = event.target.closest?.('[data-project-id]');
      const group = event.target.closest?.('[data-group-id]');
      if (link) draggedProject = Number(link.dataset.projectId);
      else if (group && !group.classList.contains('tc-projects__group--ungrouped')) {
        draggedGroup = Number(group.dataset.groupId);
      }
      if (event.dataTransfer) event.dataTransfer.setData('text/plain', String(draggedProject ?? draggedGroup ?? ''));
    });
    root.addEventListener('dragover', event => {
      const target = event.target.closest?.('[data-group-id]');
      if (target && (draggedProject !== null || draggedGroup !== null)) event.preventDefault();
    });
    root.addEventListener('drop', event => {
      if (controller.pending) return;
      const target = event.target.closest?.('[data-group-id]');
      if (!target) return;
      event.preventDefault();
      const groupId = target.dataset.groupId === '' ? null : Number(target.dataset.groupId);
      if (draggedGroup !== null && groupId !== null && draggedGroup !== groupId) {
        const ids = moveBefore(controller.snapshot.groups.map(group => group.id), draggedGroup, groupId);
        controller.reorderGroups(ids).catch(() => {});
      } else if (draggedProject !== null) {
        const targetProject = event.target.closest?.('[data-project-id]');
        const source = visibleProjects(controller.snapshot).find(row => row.projectId === draggedProject);
        if (targetProject && source?.groupId === groupId) {
          const ids = groupId === null
            ? controller.snapshot.projects
              .filter(project => !controller.snapshot.groups.some(group => group.project_ids.includes(project.id)))
              .map(project => project.id)
            : controller.snapshot.projects
              .filter(project => byId(controller.snapshot.groups, groupId)?.project_ids.includes(project.id))
              .map(project => project.id);
          const reordered = moveBefore(ids, draggedProject, Number(targetProject.dataset.projectId));
          controller.reorderProjects(mergeProjectOrder(controller.snapshot, groupId, reordered)).catch(() => {});
        } else {
          controller.assignProject(draggedProject, groupId).catch(() => {});
        }
      }
      draggedProject = draggedGroup = null;
    });
    root.addEventListener('dragend', () => { draggedProject = draggedGroup = null; });
    return {controller, destroy() { unsubscribe(); controller.disconnect(); }};
  }

  const api = {CatalogController, normalizeCatalog, visibleProjects, moveProject, moveBy, moveBefore,
    mergeProjectOrder, attach};
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') window.LificTopcoatProjects = api;
})();
