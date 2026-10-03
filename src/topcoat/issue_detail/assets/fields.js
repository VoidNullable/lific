(() => {
  'use strict';

  const STATUSES = ['backlog', 'todo', 'active', 'done', 'cancelled'];
  const PRIORITIES = ['urgent', 'high', 'medium', 'low', 'none'];

  function labelNames(value) {
    return [...new Set((Array.isArray(value) ? value : String(value ?? '').split(','))
      .map(name => String(name).trim()).filter(Boolean))];
  }

  function fillModules(select, modules, selected) {
    const values = [['', 'No module'], ...modules.map(module => [String(module.id), module.name])];
    select.replaceChildren(...values.map(([value, name]) => {
      const option = document.createElement('option'); option.value = value; option.textContent = name; return option;
    }));
    select.value = selected == null ? '' : String(selected);
  }

  function mount(root, props = {}) {
    if (!root) return null;
    const fields = Object.fromEntries([...root.querySelectorAll('[data-field]')].map(node => [node.dataset.field, node]));
    if (!fields.title || !fields.status || !fields.priority || !fields.module_id || !fields.labels) return null;
    let issue = {...(props.issue || {})}, capabilities = {...props.capabilities}, busy = false, alive = true;
    const modules = Array.isArray(props.modules) ? props.modules : [];
    const labels = Array.isArray(props.labels) ? props.labels : [];
    const statusNode = root.querySelector('[data-fields-status]');
    const metadata = root.querySelector('[data-field-metadata]');
    const created = root.querySelector('[data-field-created]'), updated = root.querySelector('[data-field-updated]');

    function values() {
      return {title: fields.title.value.trim(), status: fields.status.value, priority: fields.priority.value,
        module_id: fields.module_id.value === '' ? null : Number(fields.module_id.value), labels: labelNames(fields.labels.value)};
    }
    function render() {
      if (!alive) return;
      fields.title.value = issue.title || '';
      fields.status.value = STATUSES.includes(issue.status) ? issue.status : 'backlog';
      fields.priority.value = PRIORITIES.includes(issue.priority) ? issue.priority : 'none';
      fillModules(fields.module_id, modules, issue.module_id);
      fields.labels.value = labelNames(issue.labels).join(', ');
      for (const input of Object.values(fields)) input.disabled = busy || !capabilities.edit;
      if (statusNode && !statusNode.textContent) statusNode.textContent = capabilities.edit ? '' : 'You can view this issue but cannot edit its fields.';
      if (metadata) {
        metadata.hidden = !issue.id;
        if (created) created.textContent = issue.created_at ? `Created ${issue.created_at}` : '';
        if (updated) updated.textContent = issue.updated_at ? `Updated ${issue.updated_at}` : '';
      }
    }
    async function change(key, value) {
      if (!alive || busy || !capabilities.edit) return;
      const old = issue[key];
      if (JSON.stringify(old) === JSON.stringify(value)) return;
      busy = true; if (statusNode) statusNode.textContent = 'Saving…'; render();
      try {
        const result = await props.onIntent?.({type: 'set_scalar', field: key, value});
        if (result?.status === 'applied' && result.issue) issue = {...result.issue};
        else if (result?.status === 'conflict' && result.issue) issue = {...result.issue};
        else if (result?.status && result.status !== 'applied') throw new Error(result.error || 'Could not save this field.');
        if (statusNode) statusNode.textContent = result?.status === 'conflict' ? 'This issue changed elsewhere. The latest value is shown.' : '';
      } catch (error) {
        issue[key] = old;
        if (statusNode) statusNode.textContent = error?.message || 'Could not save this field.';
      } finally {busy = false; render();}
    }
    fields.title.addEventListener('keydown', event => {
      if (event.key === 'Escape') {fields.title.value = issue.title || ''; fields.title.blur();}
      else if (event.key === 'Enter' || ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's')) {
        event.preventDefault(); void change('title', fields.title.value.trim()); fields.title.blur();
      }
    });
    fields.title.addEventListener('blur', () => void change('title', fields.title.value.trim()));
    for (const key of ['status', 'priority']) fields[key].addEventListener('change', () => void change(key, fields[key].value));
    fields.module_id.addEventListener('change', () => void change('module_id', fields.module_id.value === '' ? null : Number(fields.module_id.value)));
    fields.labels.addEventListener('keydown', event => {
      if (event.key === 'Enter') {event.preventDefault(); void change('labels', labelNames(fields.labels.value)); fields.labels.blur();}
      else if (event.key === 'Escape') {fields.labels.value = labelNames(issue.labels).join(', '); fields.labels.blur();}
    });
    fields.labels.addEventListener('blur', () => void change('labels', labelNames(fields.labels.value)));
    render();
    function setCapabilities(next) {
      capabilities = {...next};
      for (const input of Object.values(fields)) input.disabled = busy || !capabilities.edit;
      if (statusNode) statusNode.textContent = capabilities.edit ? '' : 'You can view this issue but cannot edit its fields.';
    }
    return {values, setCapabilities, update(next, nextCapabilities) {issue = {...next}; if (nextCapabilities) capabilities = {...nextCapabilities}; render();}, dispose() {alive = false;}};
  }

  const api = {STATUSES, PRIORITIES, labelNames, mount};
  if (typeof module !== 'undefined') module.exports = api;
  globalThis.LificTopcoatIssueFields = api;
})();
