/* Issue description editor. The route supplies dispatch and owns the write queue. */
(() => {
  'use strict';

  function createSaveQueue({text = '', savedDescription = '', expectedSeq = 0, debounceMs = -1, save, onChange = () => {}}) {
    let current = String(text), saved = String(savedDescription), seq = Number(expectedSeq);
    let dirty = current !== saved, conflict = false, error = '', timer = null, running = null;
    let requested = false, revision = 0, disposed = false, blocked = false;

    function state() {return {text: current, savedDescription: saved, dirty, expectedSeq: seq, conflict, error};}
    function changed() {onChange(state());}
    function schedule() {
      if (debounceMs < 0 || disposed || blocked || !dirty || conflict || requested) return;
      clearTimeout(timer);
      timer = setTimeout(() => {timer = null; flush();}, debounceMs);
    }
    function edit(next) {
      current = String(next); revision++; dirty = current !== saved; error = '';
      conflict = conflict && dirty;
      schedule();
      changed();
      return state();
    }
    async function drain() {
      while (!disposed && !blocked && requested && dirty && !conflict) {
        requested = false;
        const sent = current, sentRevision = revision, sentSeq = seq;
        error = '';
        try {
          const result = await save(sent, sentSeq, sentRevision);
          if (disposed) break;
          if (result?.status === 'conflict') {
            saved = String(result.currentDescription ?? result.current_description ?? saved);
            seq = Math.max(seq, Number(result.expectedSeq ?? result.expected_seq ?? seq));
            dirty = current !== saved; conflict = dirty; requested = false;
            changed();
            break;
          }
          if (result?.status && result.status !== 'applied') throw new Error(result.error || 'Description save failed.');
          saved = String(result?.description ?? sent);
          seq = Math.max(seq, Number(result?.expectedSeq ?? result?.expected_seq ?? seq));
          dirty = current !== saved;
          if (revision === sentRevision && !dirty) conflict = false;
          // An edit can return to the old baseline while this request is in
          // flight. Once the acknowledgement moves the baseline, queue that
          // now-dirty draft too.
          schedule();
          changed();
        } catch (failure) {
          if (!disposed) error = String(failure?.message || failure || 'Description save failed.');
          requested = false;
          changed();
          break;
        }
      }
      changed();
      return state();
    }
    function flush() {
      clearTimeout(timer); timer = null;
      if (disposed || blocked || !dirty) return Promise.resolve(state());
      conflict = false;
      requested = true;
      changed();
      if (!running) {
        running = drain().finally(() => {running = null;});
      }
      return running;
    }
    function setCanonical({text: nextText, savedDescription: nextSaved, expectedSeq: nextSeq}) {
      if (disposed) return state();
      const incomingSeq = Number(nextSeq ?? seq);
      if (incomingSeq >= seq) {
        seq = incomingSeq;
        if (nextSaved !== undefined) saved = String(nextSaved);
        if (nextText !== undefined && (!dirty || String(nextText) === current)) current = String(nextText);
        dirty = current !== saved;
        conflict = conflict && dirty;
      }
      changed();
      return state();
    }
    function setBlocked(value) {
      if (disposed) return state();
      blocked = !!value;
      if (blocked) {clearTimeout(timer); timer = null;}
      else schedule();
      changed();
      return state();
    }
    function setConflict({currentDescription, current_description, expectedSeq, expected_seq} = {}) {
      if (disposed) return state();
      saved = String(currentDescription ?? current_description ?? saved);
      seq = Math.max(seq, Number(expectedSeq ?? expected_seq ?? seq));
      dirty = current !== saved; conflict = dirty; requested = false;
      clearTimeout(timer); timer = null;
      changed();
      return state();
    }
    function discard() {
      clearTimeout(timer); timer = null; requested = false; revision++;
      current = saved; dirty = false; conflict = false; error = '';
      changed();
      return state();
    }
    function dispose() {disposed = true; requested = false; clearTimeout(timer); timer = null;}
    return {edit, flush, state, setCanonical, setBlocked, setConflict, discard, dispose};
  }

  function appendInline(parent, source) {
    const pattern = /(!?)\[([^\]]*)\]\(([^)\s]+)(?:\s+"([^"]*)")?\)|\*\*([^*]+)\*\*|__([^_]+)__|`([^`]+)`|\*([^*]+)\*|_([^_]+)_/g;
    let offset = 0, match;
    while ((match = pattern.exec(source))) {
      if (match.index > offset) parent.append(document.createTextNode(source.slice(offset, match.index)));
      const [, image, label, rawUrl, title, bold, boldAlt, code, italic, italicAlt] = match;
      if (rawUrl) {
        const attachment = rawUrl.match(/^\/api\/attachments\/(\d+)$/);
        let url = null;
        if (attachment) {
          if (typeof globalThis.lificSession?.resolve === 'function') {
            const resolved = globalThis.lificSession.resolve(`/attachments/${attachment[1]}`);
            if (['private', 'public'].includes(resolved?.kind) && typeof resolved.url === 'string') {
              try {url = new URL(resolved.url, document.baseURI);} catch {url = null;}
            }
          }
        } else {
          try {url = new URL(rawUrl, document.baseURI);} catch {url = null;}
        }
        const validLink = url && ['http:', 'https:', 'mailto:'].includes(url.protocol);
        const validMedia = url && ['http:', 'https:'].includes(url.protocol);
        if ((!validMedia && image) || (!validLink && !image)) {
          _text(parent, match[0]); offset = pattern.lastIndex; continue;
        }
        if (image) {
          const session = globalThis.lificSession;
          if (session?.state?.publicProject != null && !attachment) {
            _text(parent, match[0]); offset = pattern.lastIndex; continue;
          }
          const imageNode = document.createElement('img');
          imageNode.className = 'tc-issue-editor__image'; imageNode.src = url.href;
          imageNode.alt = label; imageNode.loading = 'lazy'; imageNode.decoding = 'async';
          imageNode.referrerPolicy = 'no-referrer';
          if (title) imageNode.title = title;
          parent.append(imageNode);
        } else {
          const link = document.createElement('a'); link.href = url.href; link.rel = 'noopener noreferrer'; link.textContent = label;
          if (title) link.title = title;
          parent.append(link);
        }
      } else if (bold || boldAlt) {
        const node = document.createElement('strong'); node.textContent = bold || boldAlt; parent.append(node);
      } else if (italic || italicAlt) {
        const node = document.createElement('em'); node.textContent = italic || italicAlt; parent.append(node);
      } else {
        const node = document.createElement('code'); node.textContent = code; parent.append(node);
      }
      offset = pattern.lastIndex;
    }
    if (offset < source.length) parent.append(document.createTextNode(source.slice(offset)));
  }
  function _text(parent, text) {parent.append(document.createTextNode(text));}

  function listMarker(line) {
    const match = line.match(/^(\s*)([-*+]|\d+[.)])\s+(.*)$/);
    if (!match) return null;
    const indent = match[1].replace(/\t/g, '    ').length;
    return {indent, ordered: /^\d/.test(match[2]), content: match[3]};
  }

  function parseList(container, lines, start) {
    const first = listMarker(lines[start]);
    const list = document.createElement(first.ordered ? 'ol' : 'ul');
    let index = start;
    while (index < lines.length) {
      const marker = listMarker(lines[index]);
      if (!marker || marker.indent !== first.indent || marker.ordered !== first.ordered) break;
      const item = document.createElement('li');
      const task = marker.content.match(/^\[([ xX])\]\s+(.*)$/);
      if (task) {
        const checkbox = document.createElement('input');
        checkbox.type = 'checkbox'; checkbox.disabled = true;
        checkbox.checked = task[1].toLowerCase() === 'x';
        checkbox.setAttribute('aria-label', checkbox.checked ? 'Completed task' : 'Incomplete task');
        item.append(checkbox);
        appendInline(item, task[2]);
      } else appendInline(item, marker.content);
      index++;
      while (index < lines.length) {
        const nested = listMarker(lines[index]);
        if (nested && nested.indent > first.indent) {
          index = parseList(item, lines, index);
          continue;
        }
        if (!lines[index].trim()) {
          const following = listMarker(lines[index + 1] || '');
          if (following && following.indent > first.indent) {index++; continue;}
          break;
        }
        const indent = lines[index].match(/^\s*/)[0].replace(/\t/g, '    ').length;
        if (indent <= first.indent) break;
        item.append(document.createElement('br'));
        appendInline(item, lines[index].trim());
        index++;
      }
      list.append(item);
    }
    container.append(list);
    return index;
  }

  function tableCells(line) {
    let value = line.trim();
    if (value.startsWith('|')) value = value.slice(1);
    if (value.endsWith('|') && !value.endsWith('\\|')) value = value.slice(0, -1);
    const cells = [];
    let current = '', escaped = false;
    for (const character of value) {
      if (character === '|' && !escaped) {cells.push(current.trim()); current = '';}
      else if (character === '\\' && !escaped) {current += character; escaped = true; continue;}
      else {current += character; escaped = false;}
      escaped = false;
    }
    cells.push(current.trim());
    return cells;
  }

  function tableSeparator(line) {
    const cells = tableCells(line);
    return cells.length > 0 && cells.every(cell => /^:?-{3,}:?$/.test(cell));
  }

  function parseTable(container, lines, start) {
    const headers = tableCells(lines[start]);
    const separators = tableCells(lines[start + 1]);
    if (headers.length !== separators.length || !tableSeparator(lines[start + 1])) return null;
    const table = document.createElement('table');
    table.className = 'tc-issue-editor__table';
    const alignments = separators.map(cell => cell.startsWith(':') && cell.endsWith(':') ? 'center'
      : cell.endsWith(':') ? 'right' : cell.startsWith(':') ? 'left' : '');
    const head = document.createElement('thead'), headerRow = document.createElement('tr');
    headers.forEach((text, index) => {
      const cell = document.createElement('th');
      cell.scope = 'col';
      if (alignments[index]) cell.style.textAlign = alignments[index];
      appendInline(cell, text); headerRow.append(cell);
    });
    head.append(headerRow); table.append(head);
    const body = document.createElement('tbody');
    let index = start + 2;
    while (index < lines.length && lines[index].includes('|') && lines[index].trim()) {
      const values = tableCells(lines[index]);
      const row = document.createElement('tr');
      headers.forEach((_, column) => {
        const cell = document.createElement('td');
        if (alignments[column]) cell.style.textAlign = alignments[column];
        appendInline(cell, values[column] || ''); row.append(cell);
      });
      body.append(row); index++;
    }
    table.append(body); container.append(table);
    return index;
  }

  function renderMarkdown(container, source) {
    container.replaceChildren();
    const lines = String(source).replace(/\r\n?/g, '\n').split('\n');
    let i = 0;
    while (i < lines.length) {
      const line = lines[i];
      if (!line.trim()) {i++; continue;}
      if (/^```/.test(line)) {
        const code = []; i++;
        while (i < lines.length && !/^```/.test(lines[i])) code.push(lines[i++]);
        if (i < lines.length) i++;
        const pre = document.createElement('pre'), codeNode = document.createElement('code');
        codeNode.textContent = code.join('\n'); pre.append(codeNode); container.append(pre); continue;
      }
      const heading = line.match(/^(#{1,6})\s+(.*)$/);
      if (heading) {
        const node = document.createElement(`h${heading[1].length}`); appendInline(node, heading[2]); container.append(node); i++; continue;
      }
      if (/^>\s?/.test(line)) {
        const quote = document.createElement('blockquote'); appendInline(quote, line.replace(/^>\s?/, '')); container.append(quote); i++; continue;
      }
      if (lines[i + 1] && line.includes('|') && tableSeparator(lines[i + 1])) {
        const next = parseTable(container, lines, i);
        if (next > i) {i = next; continue;}
        const malformed = document.createElement('p');
        appendInline(malformed, line);
        malformed.append(document.createElement('br'));
        appendInline(malformed, lines[i + 1]);
        container.append(malformed);
        i += 2;
        continue;
      }
      if (listMarker(line)) {
        i = parseList(container, lines, i);
        continue;
      }
      const paragraph = document.createElement('p');
      while (i < lines.length && lines[i].trim() && !/^(#{1,6}\s|>\s?|```)/.test(lines[i])
        && !listMarker(lines[i]) && !(lines[i + 1] && lines[i].includes('|') && tableSeparator(lines[i + 1]))) {
        if (paragraph.childNodes.length) paragraph.append(document.createElement('br'));
        appendInline(paragraph, lines[i++]);
      }
      container.append(paragraph);
    }
    return container;
  }

  function mount(root, props = {}) {
    if (!root) return null;
    const route = {...props.route};
    let capabilities = {...props.capabilities};
    const input = root.querySelector('[data-editor-input]'), preview = root.querySelector('[data-editor-preview]');
    const errorNode = root.querySelector('[data-editor-error]'), conflictNode = root.querySelector('[data-editor-conflict]');
    const conflictMessage = root.querySelector('[data-editor-conflict-message]');
    const serverNode = root.querySelector('[data-editor-server-value]'), statusNode = root.querySelector('[data-editor-status]');
    const saveButton = root.querySelector('[data-editor-save]'), editButton = root.querySelector('[data-editor-edit]');
    const previewButton = root.querySelector('[data-editor-preview-toggle]');
    const cancelButton = root.querySelector('[data-editor-cancel]');
    const attachmentRoot = root.querySelector('[data-editor-attachments]');
    const uploadForm = attachmentRoot?.querySelector('[data-attachment-upload]');
    const uploadInput = uploadForm?.querySelector('[data-attachment-files]');
    const uploadStatus = uploadForm?.querySelector('[data-attachment-status]');
    if (!input || !preview) return null;
    const pendingRequests = new Set();
    let showingPreview = true, destroyed = false, uploadBusy = false, uploadPoll = null;
    let lastSelection = {start: input.value.length, end: input.value.length}, attachmentMount = null;
    let saving = 0, attachmentGeneration = 0;
    function sameRoute(candidate) {
      return candidate?.issue_id === route.issue_id && candidate?.generation === route.generation;
    }
    function saveThroughRoute(description, expectedSeq, editRevision) {
      return new Promise((resolve, reject) => {
        let settled = false;
        const cancel = () => {cleanup(); if (!settled) {settled = true; reject(new Error('Editor was unmounted.'));}};
        const done = event => {
          const detail = event.detail || {};
          if (!sameRoute(detail.route)) return;
          if (event.type === 'lific:issue-detail-applied' && detail.kind === 'editor' && detail.edit_revision === editRevision) {
            cleanup(); settled = true; resolve({status: 'applied', description: detail.description, expected_seq: detail.expected_seq});
          } else if (event.type === 'lific:issue-detail-conflict' && detail.edit_revision === editRevision) {
            cleanup(); settled = true; resolve({status: 'conflict', current_description: detail.current_description, expected_seq: detail.expected_seq});
          } else if (event.type === 'lific:issue-detail-error' && detail.edit_revision === editRevision) {
            cleanup(); settled = true; reject(new Error(detail.error || 'Description save failed.'));
          }
        };
        const cleanup = () => {
          pendingRequests.delete(cancel);
          window.removeEventListener('lific:issue-detail-applied', done);
          window.removeEventListener('lific:issue-detail-conflict', done);
          window.removeEventListener('lific:issue-detail-error', done);
        };
        window.addEventListener('lific:issue-detail-applied', done);
        window.addEventListener('lific:issue-detail-conflict', done);
        window.addEventListener('lific:issue-detail-error', done);
        pendingRequests.add(cancel);
        window.dispatchEvent(new CustomEvent('lific:issue-detail-intent', {detail: {
          route, action: {type: 'save_description', description, expected_seq: expectedSeq, edit_revision: editRevision},
        }}));
      });
    }
    const queue = createSaveQueue({text: props.text ?? '', savedDescription: props.saved_description ?? '', expectedSeq: props.expected_seq ?? 0,
      debounceMs: -1, save: saveThroughRoute, onChange: renderState});
    function renderState() {
      if (destroyed) return;
      root.hidden = false;
      const state = queue.state();
      if (input.value !== state.text) input.value = state.text;
      input.disabled = !capabilities.edit;
      saveButton.hidden = showingPreview;
      saveButton.disabled = !capabilities.edit || !state.dirty || uploadBusy;
      if (cancelButton) {cancelButton.hidden = showingPreview; cancelButton.disabled = saving > 0;}
      if (attachmentRoot) attachmentRoot.hidden = !capabilities.edit || showingPreview;
      if (errorNode) {errorNode.hidden = !state.error; errorNode.textContent = state.error;}
      if (conflictNode) conflictNode.hidden = !state.conflict;
      if (conflictMessage) conflictMessage.textContent = state.conflict ? 'This description changed on the server. Review the current version before saving your draft again.' : '';
      if (serverNode) serverNode.textContent = state.conflict ? state.savedDescription : '';
      if (statusNode) statusNode.textContent = state.dirty ? 'Unsaved changes' : 'Saved';
      renderMarkdown(preview, state.text);
      input.hidden = showingPreview;
      preview.hidden = !showingPreview;
      previewButton.hidden = showingPreview;
      previewButton.textContent = showingPreview ? 'Edit' : 'Preview';
      previewButton.setAttribute('aria-pressed', String(showingPreview));
      editButton.hidden = !capabilities.edit || !showingPreview;
    }
    function rememberSelection() {
      lastSelection = {start: input.selectionStart ?? input.value.length, end: input.selectionEnd ?? input.value.length};
    }
    function applyInputTransform(text, selectionStart, selectionEnd) {
      input.value = text;
      input.focus(); input.setSelectionRange(selectionStart, selectionEnd);
      input.dispatchEvent(new Event('input', {bubbles: true}));
    }
    function toggleInline(marker) {
      const text = input.value, start = input.selectionStart, end = input.selectionEnd, width = marker.length;
      if (start === end) {
        const next = text.slice(0, start) + marker + marker + text.slice(end);
        applyInputTransform(next, start + width, start + width);
        return;
      }
      const selected = text.slice(start, end);
      const nestedStrong = marker === '*' && text[start - 2] === '*' && text[end + 1] === '*';
      if (!nestedStrong && text.slice(start - width, start) === marker && text.slice(end, end + width) === marker) {
        applyInputTransform(text.slice(0, start - width) + selected + text.slice(end + width), start - width, end - width);
      } else if (selected.length >= width * 2 && selected.startsWith(marker) && selected.endsWith(marker)) {
        const inner = selected.slice(width, selected.length - width);
        applyInputTransform(text.slice(0, start) + inner + text.slice(end), start, start + inner.length);
      } else {
        applyInputTransform(text.slice(0, start) + marker + selected + marker + text.slice(end), start + width, end + width);
      }
    }
    function insertLink() {
      const text = input.value, start = input.selectionStart, end = input.selectionEnd;
      const selected = text.slice(start, end);
      if (start === end) {
        const insertion = '[link text](URL)';
        const next = text.slice(0, start) + insertion + text.slice(end);
        applyInputTransform(next, start + 1, start + 10);
      } else {
        const insertion = `[${selected}](URL)`;
        const next = text.slice(0, start) + insertion + text.slice(end);
        const urlStart = start + selected.length + 3;
        applyInputTransform(next, urlStart, urlStart + 3);
      }
    }
    function insertUploadedMarkdown(markdown) {
      const text = input.value;
      const start = Math.max(0, Math.min(text.length, lastSelection.start));
      const end = Math.max(start, Math.min(text.length, lastSelection.end));
      const before = text.slice(0, start), after = text.slice(end);
      const prefix = before && !before.endsWith('\n') ? '\n' : '';
      const suffix = after && !after.startsWith('\n') ? '\n' : '';
      const caret = before.length + prefix.length + markdown.length;
      input.value = `${before}${prefix}${markdown}${suffix}${after}`;
      lastSelection = {start: caret, end: caret};
      input.focus(); input.setSelectionRange(caret, caret);
      input.dispatchEvent(new Event('input', {bubbles: true}));
    }
    function stopUploadPolling() {
      if (uploadPoll !== null) clearInterval(uploadPoll);
      uploadPoll = null;
    }
    function finishUploadIfIdle() {
      if (!uploadBusy || uploadInput?.disabled) return false;
      stopUploadPolling(); uploadBusy = false; queue.setBlocked(false); renderState();
      return true;
    }
    function pollUpload() {
      if (uploadPoll !== null) return;
      uploadPoll = setInterval(finishUploadIfIdle, 40);
      finishUploadIfIdle();
    }
    function beforeUpload(event) {
      if (!capabilities.edit) {event.preventDefault(); event.stopImmediatePropagation(); return;}
      const state = queue.state();
      if (state.conflict) {
        event.preventDefault(); event.stopImmediatePropagation();
        if (uploadStatus) uploadStatus.textContent = 'Resolve the description conflict before uploading.';
        return;
      }
    }
    function afterUploadSubmit() {
      if (!uploadInput?.disabled || !uploadInput.files?.length) return;
      uploadBusy = true; queue.setBlocked(true); renderState(); pollUpload();
    }
    function mountAttachments() {
      const helper = globalThis.LificTopcoatAttachments;
      if (!uploadForm || !uploadInput || !attachmentRoot || !helper?.createClient || !helper?.attach) return;
      const generation = ++attachmentGeneration;
      try {
        const client = helper.createClient({session: globalThis.lificSession});
        attachmentMount = helper.attach(attachmentRoot, {
          client, target: {entity_type: 'issue', entity_id: Number(route.issue_id)},
          text: {read: () => queue.state().text, write: text => {
            input.value = text; input.dispatchEvent(new Event('input', {bubbles: true}));
          }},
          onUploaded(_attachment, markdown) {
            if (!destroyed && generation === attachmentGeneration) insertUploadedMarkdown(markdown);
          },
        });
      } catch (error) {
        if (uploadStatus) uploadStatus.textContent = `Attachments unavailable: ${error.message}`;
      }
    }
    mountAttachments();
    if (attachmentMount) {
      uploadForm.addEventListener('submit', beforeUpload, true);
      attachmentRoot.addEventListener('pointerdown', rememberSelection, true);
      input.addEventListener('select', rememberSelection);
      input.addEventListener('keyup', rememberSelection);
      input.addEventListener('mouseup', rememberSelection);
      input.addEventListener('blur', rememberSelection);
      uploadForm.addEventListener('submit', afterUploadSubmit);
    }
    input.addEventListener('input', () => {
      if (destroyed || !capabilities.edit) return;
      const text = input.value;
      queue.edit(text);
      rememberSelection();
      window.dispatchEvent(new CustomEvent('lific:issue-detail-intent', {detail: {
        route, action: {type: 'edit_description', description: text},
      }}));
      renderState();
    });
    const flush = async () => {if (!capabilities.edit || uploadBusy) return; await queue.flush(); renderState();};
    async function commit() {
      if (!capabilities.edit || uploadBusy) return;
      saving++; renderState();
      try {
        await flush();
        const state = queue.state();
        if (!destroyed && !state.dirty && !state.error && !state.conflict) showingPreview = true;
      } finally {saving--; renderState();}
    }
    function cancel() {
      if (!capabilities.edit || saving > 0) return;
      if (attachmentMount?.cancel) attachmentMount.cancel();
      else {attachmentGeneration++; attachmentMount?.dispose(); attachmentMount = null; mountAttachments();}
      stopUploadPolling(); uploadBusy = false; queue.setBlocked(false);
      queue.discard(); showingPreview = true; renderState();
    }
    function attachmentBusy(event) {
      stopUploadPolling();
      uploadBusy = !!event.detail?.busy; queue.setBlocked(uploadBusy); renderState();
    }
    attachmentRoot?.addEventListener('lific:attachment-busy', attachmentBusy);
    saveButton.addEventListener('click', commit);
    cancelButton?.addEventListener('click', cancel);
    previewButton.addEventListener('click', commit);
    editButton.addEventListener('click', () => {showingPreview = false; renderState(); input.focus();});
    input.addEventListener('keydown', event => {
      if ((event.metaKey || event.ctrlKey) && !event.altKey) {
        const key = event.key.toLowerCase();
        if (event.shiftKey && key === 'k') {
          event.preventDefault(); event.stopPropagation();
          insertLink();
          return;
        }
        if (!event.shiftKey && (key === 'b' || key === 'i')) {
          event.preventDefault();
          const marker = key === 'b' ? '**' : '*';
          toggleInline(marker);
          return;
        }
      }
      if (event.key === 'Escape') {event.preventDefault(); event.stopPropagation(); cancel();}
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {event.preventDefault(); commit();}
    });
    function update(next = {}) {
      if (destroyed) return;
      if (next.route && (next.route.issue_id !== route.issue_id || next.route.generation !== route.generation)) return;
      if (next.capabilities) capabilities = {...next.capabilities};
      if (next.conflict) queue.setConflict(next.conflict);
      else queue.setCanonical({text: next.text, savedDescription: next.saved_description, expectedSeq: next.expected_seq});
      renderState();
    }
    function dispose() {
      if (destroyed) return;
      destroyed = true; attachmentGeneration++; queue.dispose();
      stopUploadPolling(); attachmentMount?.dispose();
      attachmentRoot?.removeEventListener('lific:attachment-busy', attachmentBusy);
      uploadForm?.removeEventListener('submit', beforeUpload, true);
      uploadForm?.removeEventListener('submit', afterUploadSubmit);
      attachmentRoot?.removeEventListener('pointerdown', rememberSelection, true);
      input.removeEventListener('select', rememberSelection); input.removeEventListener('keyup', rememberSelection);
      input.removeEventListener('mouseup', rememberSelection); input.removeEventListener('blur', rememberSelection);
      for (const cancel of [...pendingRequests]) cancel();
    }
    input.value = props.text ?? ''; renderState();
    return {flush, update, dispose, queue, cancel, preview() {showingPreview = true; renderState();}, edit() {showingPreview = false; renderState(); input.focus();}};
  }

  const api = {createSaveQueue, renderMarkdown, mount};
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') window.lificIssueEditor = api;
})();
