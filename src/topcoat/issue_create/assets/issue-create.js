(() => {
  'use strict';
  const STATUSES = ['backlog', 'todo', 'active', 'done', 'cancelled'];
  const PRIORITIES = ['urgent', 'high', 'medium', 'low', 'none'];

  function querySearch(search) {
    if (search.startsWith('#/')) return search.slice(search.indexOf('?') + 1);
    if (search.startsWith('#')) return search.slice(search.indexOf('?') + 1);
    return search.startsWith('?') ? search.slice(1) : search;
  }

  function model({modules = [], search = ''} = {}) {
    const params = new URLSearchParams(querySearch(search));
    const rawModule = params.get('module');
    const parsedModule = rawModule && /^\d+$/.test(rawModule) ? Number(rawModule) : NaN;
    return {
      status: STATUSES.includes(params.get('status')) ? params.get('status') : 'backlog',
      priority: 'none',
      moduleId: Number.isSafeInteger(parsedModule) && parsedModule > 0 && modules.some(item => item.id === parsedModule) ? parsedModule : null,
      title: '', description: '', labels: [], error: '',
    };
  }

  function controller(env, identifier = '') {
    const state = {phase: 'loading', project: null, role: null, modules: [], labelOptions: [], ...model({search: env.search || ''}), error: ''};
    const transfers = new Set();
    let disposed = false;
    let pendingUploads = 0;
    const publish = patch => {Object.assign(state, patch); env.onChange?.(state);};
    const editable = role => !!role && (role.is_admin || !role.enforced || ['maintainer', 'lead'].includes(role.role));

    async function load() {
      const projects = await env.api.request('/projects');
      if (disposed) return false;
      if (!projects.ok) {publish({phase: 'error', error: projects.error}); return false;}
      const project = projects.data.find(item => item.identifier === identifier);
      if (!project) {publish({phase: 'error', error: `Project ${identifier} not found`}); return false;}
      const [role, modules, labels] = await Promise.all([
        env.api.request(`/projects/${project.id}/my-role`),
        env.api.request(`/modules?project_id=${project.id}`),
        env.api.request(`/labels?project_id=${project.id}`),
      ]);
      if (disposed) return false;
      if (!role.ok) {publish({phase: 'error', error: role.error}); return false;}
      if (!editable(role.data)) {publish({phase: 'denied', project, role: role.data}); return false;}
      if (!modules.ok || !labels.ok) {
        publish({phase: 'error', project, role: role.data, error: !modules.ok ? modules.error : labels.error});
        return false;
      }
      publish({...model({modules: modules.data, search: env.search || ''}), phase: 'ready', project, role: role.data, modules: modules.data, labelOptions: labels.data, error: ''});
      return true;
    }

    function canCreate() {return state.phase === 'ready' && state.title.trim().length > 0 && (env.pendingUploads?.() ?? pendingUploads) === 0 && !state.saving;}
    async function create() {
      if (!canCreate()) return false;
      publish({saving: true, error: ''});
      const body = {project_id: state.project.id, title: state.title.trim(), description: state.description, status: state.status, priority: state.priority, labels: [...state.labels]};
      if (state.moduleId !== null) body.module_id = state.moduleId;
      const result = await env.api.request('/issues', {method: 'POST', body: JSON.stringify(body)});
      if (disposed) return false;
      if (result.ok) {
        env.navigate(`/${encodeURIComponent(identifier)}/issues/${encodeURIComponent(result.data.identifier)}`);
        return true;
      }
      publish({saving: false, error: result.error});
      return false;
    }

    function discard() {
      disposed = true;
      for (const transfer of transfers) transfer.abort();
      transfers.clear();
      env.navigate(`/${encodeURIComponent(identifier)}/issues`);
    }

    async function upload(files, initialSelection = null) {
      let selection = initialSelection || env.selection?.() || {start: state.description.length, end: state.description.length};
      for (const file of files || []) {
        if (disposed) break;
        pendingUploads++;
        env.onPending?.(pendingUploads);
        const transfer = env.attachments.upload(file);
        transfers.add(transfer);
        const result = await transfer.result;
        transfers.delete(transfer);
        pendingUploads--;
        env.onPending?.(pendingUploads);
        if (disposed) continue;
        if (result.ok) {
          const snippet = env.attachments.markdown(result.data);
          const text = state.description;
          const start = Math.max(0, Math.min(text.length, selection.start));
          const end = Math.max(start, Math.min(text.length, selection.end));
          const before = text.slice(0, start);
          const after = text.slice(end);
          const prefix = before && !before.endsWith('\n') ? '\n' : '';
          const suffix = after && !after.startsWith('\n') ? '\n' : '';
          const caret = before.length + prefix.length + snippet.length;
          publish({description: `${before}${prefix}${snippet}${suffix}${after}`, caret});
          selection = {start: caret, end: caret};
        } else if (!result.canceled) publish({error: result.error});
      }
    }

    function toggleLabel(name) {
      publish({labels: state.labels.includes(name) ? state.labels.filter(value => value !== name) : [...state.labels, name]});
    }

    async function createLabel(name, color) {
      const trimmed = name.trim();
      if (state.phase !== 'ready' || !state.project || !trimmed || !/^#[\da-f]{6}$/i.test(color) || state.creatingLabel) return false;
      const existing = state.labelOptions.find(label => label.name.toLowerCase() === trimmed.toLowerCase());
      if (existing) {
        if (!state.labels.includes(existing.name)) publish({labels: [...state.labels, existing.name], error: ''});
        return true;
      }
      publish({creatingLabel: true, error: ''});
      const result = await env.api.request('/labels', {method: 'POST', body: JSON.stringify({project_id: state.project.id, name: trimmed, color})});
      if (disposed) return false;
      if (!result.ok) {
        publish({creatingLabel: false, error: result.error});
        return false;
      }
      const labelOptions = [...state.labelOptions.filter(label => label.id !== result.data.id), result.data].sort((a, b) => a.name.localeCompare(b.name));
      const labels = state.labels.includes(result.data.name) ? state.labels : [...state.labels, result.data.name];
      publish({labelOptions, labels, creatingLabel: false, error: ''});
      return true;
    }

    return {env, state, transfers, load, create, discard, upload, toggleLabel, createLabel, canCreate, dispose() {disposed = true; for (const transfer of transfers) transfer.abort(); transfers.clear();}};
  }

  function mount(root, env = {}) {
    const identifier = root.dataset.projectIdentifier;
    const session = env.session || globalThis.lificSession;
    const api = env.api || session;
    const navigate = env.navigate || (path => {globalThis.location.hash = `#${path}`;});
    const attachments = env.attachments || attachmentClient(globalThis.LificTopcoatAttachments,session);
    const search = env.search ?? (globalThis.location.hash.startsWith('#/') ? globalThis.location.hash : globalThis.location.search);
    const status = root.querySelector('[data-issue-create-load-error]');
    const loadMessage = root.querySelector('[data-issue-create-load-message]');
    const loading = root.querySelector('[data-issue-create-loading]');
    const denied = root.querySelector('[data-issue-create-denied]');
    const form = root.querySelector('[data-issue-create-form]');
    const title = root.querySelector('[data-issue-create-title]');
    const description = root.querySelector('[data-issue-create-description]');
    const error = root.querySelector('[data-issue-create-error]');
    const submit = root.querySelector('[data-issue-create-submit]');
    const module = root.querySelector('[data-issue-create-module]');
    const labelList = root.querySelector('[data-issue-create-labels]');
    const labelName = root.querySelector('[data-issue-create-label-name]');
    const labelColor = root.querySelector('[data-issue-create-label-color]');
    const labelSubmit = root.querySelector('[data-issue-create-label-submit]');
    const uploadStatus = root.querySelector('[data-issue-create-upload-status]');
    let rememberedSelection = null;
    const descriptionSelection = () => description.ownerDocument.activeElement === description
      ? {start: description.selectionStart, end: description.selectionEnd}
      : {start: description.value.length, end: description.value.length};
    const createController = controller({...env, api, attachments, navigate, search, onChange: state => {
      root.setAttribute('aria-busy', String(state.phase === 'loading'));
      loading.hidden = state.phase !== 'loading';
      denied.hidden = state.phase !== 'denied';
      form.hidden = state.phase !== 'ready';
      status.hidden = state.phase !== 'error';
      loadMessage.textContent = state.phase === 'error' ? state.error : '';
      error.textContent = state.error;
      if (state.phase === 'ready') {
        title.value = state.title;
        description.value = state.description;
        if (Number.isInteger(state.caret)) {
          description.focus();
          description.setSelectionRange(state.caret, state.caret);
          delete state.caret;
        }
        root.querySelector('[data-issue-create-status]').value = state.status;
        root.querySelector('[data-issue-create-priority]').value = state.priority;
        module.replaceChildren(new Option('None', ''), ...state.modules.map(item => new Option(item.name, String(item.id))));
        module.value = state.moduleId === null ? '' : String(state.moduleId);
        labelList.replaceChildren(...state.labelOptions.map(label => {
          const wrapper = document.createElement('label');
          const checkbox = document.createElement('input');
          checkbox.type = 'checkbox'; checkbox.value = label.name; checkbox.checked = state.labels.includes(label.name);
          checkbox.dataset.issueCreateLabel = '';
          wrapper.append(checkbox, document.createTextNode(label.name));
          return wrapper;
        }));
      }
      submit.disabled = !createController.canCreate();
      submit.textContent = state.saving ? 'Creating…' : 'Create issue';
      labelSubmit.disabled = !!state.creatingLabel;
      labelSubmit.textContent = state.creatingLabel ? 'Creating…' : 'Create label';
    }, onPending: count => {
      uploadStatus.textContent = count ? `Uploading ${count} file${count === 1 ? '' : 's'}…` : 'Markdown · drag, paste or attach files';
      submit.disabled = !createController.canCreate();
    }} , identifier);

    const refreshSubmit = () => {submit.disabled = !createController.canCreate();};
    form.addEventListener('submit', event => {event.preventDefault(); void createController.create();});
    title.addEventListener('input', () => {createController.state.title = title.value; refreshSubmit();});
    description.addEventListener('input', () => {createController.state.description = description.value;});
    root.querySelector('[data-issue-create-status]').addEventListener('change', event => {createController.state.status = event.target.value;});
    root.querySelector('[data-issue-create-priority]').addEventListener('change', event => {createController.state.priority = event.target.value;});
    module.addEventListener('change', () => {createController.state.moduleId = module.value ? Number(module.value) : null;});
    labelList.addEventListener('change', event => {
      const checkbox = event.target.closest('[data-issue-create-label]');
      if (checkbox) createController.toggleLabel(checkbox.value);
    });
    const submitLabel = async () => {
      if (await createController.createLabel(labelName.value, labelColor.value)) labelName.value = '';
    };
    labelSubmit.addEventListener('click', () => {void submitLabel();});
    labelName.addEventListener('keydown', event => {
      if (event.key === 'Enter') {event.preventDefault(); void submitLabel();}
    });
    root.querySelectorAll('[data-issue-create-back], [data-issue-create-discard]').forEach(button => button.addEventListener('click', event => {event.preventDefault(); createController.discard();}));
    root.querySelector('[data-issue-create-retry]').addEventListener('click', () => {void createController.load();});
    const files = root.querySelector('[data-issue-create-files]');
    files.addEventListener('click', () => {rememberedSelection = descriptionSelection();});
    files.addEventListener('change', () => {void createController.upload(files.files, rememberedSelection || descriptionSelection()); rememberedSelection = null; files.value = '';});
    description.addEventListener('paste', event => {
      const pasted = [...(event.clipboardData?.files || [])];
      if (pasted.length) {const selection = descriptionSelection(); event.preventDefault(); void createController.upload(pasted, selection);}
    });
    description.addEventListener('dragover', event => {if (event.dataTransfer?.types?.includes('Files')) event.preventDefault();});
    description.addEventListener('drop', event => {
      const dropped = [...(event.dataTransfer?.files || [])];
      if (dropped.length) {const selection = descriptionSelection(); event.preventDefault(); void createController.upload(dropped, selection);}
    });
    void createController.load();
    return {controller: createController, dispose() {createController.dispose();}};
  }

  function attachmentClient(helper, session) {
    if (!helper?.createClient || typeof helper.markdown !== 'function') return null;
    return {...helper.createClient({session}),markdown:helper.markdown};
  }

  const api = {STATUSES, PRIORITIES, model, controller, mount, attachmentClient};
  globalThis.LificTopcoatIssueCreate = api;
  if (typeof document !== 'undefined') {
    const start = () => document.querySelectorAll('[data-topcoat-issue-create]').forEach(root => {
      if (!root.__topcoatIssueCreate) root.__topcoatIssueCreate = mount(root);
    });
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, {once: true});
    else start();
  }
})();
