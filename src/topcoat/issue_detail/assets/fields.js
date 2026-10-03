(() => {
  'use strict';

  const STATUSES = ['backlog', 'todo', 'active', 'done', 'cancelled'];
  const PRIORITIES = ['urgent', 'high', 'medium', 'low', 'none'];

  function labelNames(value) {
    return [...new Set((Array.isArray(value) ? value : []).map(name => String(name)).filter(Boolean))];
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
    const labelField = root.querySelector('[data-label-field]');
    const labelOptions = root.querySelector('[data-label-options]');
    const newLabelName = root.querySelector('[data-new-label-name]');
    const newLabelColor = root.querySelector('[data-new-label-color]');
    const createLabelButton = root.querySelector('[data-create-label]');
    if (!fields.title || !fields.status || !fields.priority || !fields.module_id || !fields.target_date ||
      !labelField || !labelOptions || !newLabelName || !newLabelColor || !createLabelButton) return null;
    let issue = {...(props.issue || {})}, capabilities = {...props.capabilities}, busy = false, alive = true;
    const modules = Array.isArray(props.modules) ? props.modules : [];
    let labels = Array.isArray(props.labels) ? [...props.labels] : [];
    const statusNode = root.querySelector('[data-fields-status]');
    const metadata = root.querySelector('[data-field-metadata]');
    const created = root.querySelector('[data-field-created]'), updated = root.querySelector('[data-field-updated]');

    function values() {
      return {title: fields.title.value.trim(), status: fields.status.value, priority: fields.priority.value,
        module_id: fields.module_id.value === '' ? null : Number(fields.module_id.value), target_date: fields.target_date.value || null,
        labels: labelNames([...labelOptions.querySelectorAll('input:checked')].map(input => input.value))};
    }
    function renderLabels() {
      const selected = new Set(labelNames(issue.labels));
      const names = new Map(labels.map(label => [String(label.name), label]));
      for (const name of selected) if (!names.has(name)) names.set(name, {name, color: null});
      const activeName = labelOptions.contains(document.activeElement) ? document.activeElement.value : null;
      labelOptions.replaceChildren(...[...names.values()].sort((a, b) => a.name.localeCompare(b.name)).map(label => {
        const labelName = String(label.name);
        const row = document.createElement('label');
        row.className = 'tc-issue-fields__label';
        const input = document.createElement('input');
        input.type = 'checkbox'; input.value = labelName; input.checked = selected.has(labelName);
        input.disabled = busy || !capabilities.edit;
        const text = document.createElement('span'); text.textContent = labelName;
        if (label.color) text.style.setProperty('--tc-label-color', label.color);
        row.append(input, text);
        return row;
      }));
      if (activeName !== null) [...labelOptions.querySelectorAll('input')].find(input => input.value === activeName)?.focus({preventScroll:true});
    }
    function render() {
      if (!alive) return;
      fields.title.value = issue.title || '';
      fields.status.value = STATUSES.includes(issue.status) ? issue.status : 'backlog';
      fields.priority.value = PRIORITIES.includes(issue.priority) ? issue.priority : 'none';
      fillModules(fields.module_id, modules, issue.module_id);
      fields.target_date.value = issue.target_date || '';
      for (const input of Object.values(fields)) input.disabled = busy || !capabilities.edit;
      renderLabels();
      newLabelName.disabled = newLabelColor.disabled = createLabelButton.disabled = busy || !capabilities.edit;
      if (statusNode && !statusNode.textContent) statusNode.textContent = capabilities.edit ? '' : 'You can view this issue but cannot edit its fields.';
      if (metadata) {
        metadata.hidden = !issue.id;
        if (created) created.textContent = issue.created_at ? `Created ${issue.created_at}` : '';
        if (updated) updated.textContent = issue.updated_at ? `Updated ${issue.updated_at}` : '';
      }
    }
    async function change(key, value, {recordUndo = true} = {}) {
      if (!alive || busy || !capabilities.edit) return;
      const old = issue[key];
      if (JSON.stringify(old) === JSON.stringify(value)) return;
      busy = true; if (statusNode) statusNode.textContent = 'Saving…'; render();
      try {
        const result = await props.onIntent?.({type: 'set_scalar', field: key, value});
        if (result?.status === 'applied' && result.issue) issue = {...result.issue};
        else if (result?.status === 'conflict' && result.issue) issue = {...result.issue};
        else if (result?.status && result.status !== 'applied') throw new Error(result.error || 'Could not save this field.');
        if (statusNode) {
          statusNode.replaceChildren(document.createTextNode(result?.status === 'conflict' ? 'This issue changed elsewhere. The latest value is shown.' : 'Saved.'));
          if (recordUndo && result?.status === 'applied') {
            const undo = document.createElement('button'); undo.type = 'button'; undo.textContent = 'Undo';
            undo.addEventListener('click', () => void change(key, old, {recordUndo:false}), {once:true});
            statusNode.append(' ', undo);
          }
        }
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
    fields.target_date.addEventListener('change', () => void change('target_date', fields.target_date.value || null));
    labelOptions.addEventListener('change', () => void change('labels', values().labels));
    createLabelButton.addEventListener('click', async () => {
      const name = newLabelName.value.trim();
      if (!name || busy || !capabilities.edit || typeof props.onCreateLabel !== 'function') return;
      busy = true; if (statusNode) statusNode.textContent = 'Creating label…'; render();
      try {
        const created = await props.onCreateLabel(name, newLabelColor.value);
        const label = created?.label || created;
        if (!label || typeof label.name !== 'string') throw new Error('Could not create this label.');
        if (!labels.some(item => String(item.name).toLocaleLowerCase() === label.name.toLocaleLowerCase())) labels.push(label);
        newLabelName.value = '';
        if (statusNode) statusNode.textContent = 'Label created. Saving…';
        const nextLabels = labelNames([...(issue.labels || []), label.name]);
        busy = false; render(); await change('labels', nextLabels);
      } catch (error) {
        if (statusNode) statusNode.textContent = error?.message || 'Could not create this label.';
        busy = false; render();
      }
    });
    render();
    function setCapabilities(next) {
      capabilities = {...next};
      render();
    }
    return {values, setCapabilities, update(next, nextCapabilities) {issue = {...next}; if (nextCapabilities) capabilities = {...nextCapabilities}; render();}, dispose() {alive = false;}};
  }

  const api = {STATUSES, PRIORITIES, labelNames, mount};
  if (typeof module !== 'undefined') module.exports = api;
  globalThis.LificTopcoatIssueFields = api;
})();
