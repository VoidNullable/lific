/* Issue description editor. The route supplies dispatch and owns the write queue. */
(() => {
  'use strict';

  function createSaveQueue({text = '', savedDescription = '', expectedSeq = 0, debounceMs = 650, save, onChange = () => {}}) {
    let current = String(text), saved = String(savedDescription), seq = Number(expectedSeq);
    let dirty = current !== saved, conflict = false, error = '', timer = null, running = null;
    let requested = false, revision = 0, disposed = false;

    function state() {return {text: current, savedDescription: saved, dirty, expectedSeq: seq, conflict, error};}
    function changed() {onChange(state());}
    function schedule() {
      if (debounceMs < 0 || disposed || !dirty || conflict || requested) return;
      clearTimeout(timer);
      timer = setTimeout(() => {timer = null; flush();}, debounceMs);
    }
    function edit(next) {
      current = String(next); revision++; dirty = current !== saved; error = '';
      schedule();
      changed();
      return state();
    }
    async function drain() {
      while (!disposed && requested && dirty && !conflict) {
        requested = false;
        const sent = current, sentRevision = revision, sentSeq = seq;
        error = '';
        try {
          const result = await save(sent, sentSeq, sentRevision);
          if (disposed) break;
          if (result?.status === 'conflict') {
            saved = String(result.currentDescription ?? result.current_description ?? saved);
            seq = Math.max(seq, Number(result.expectedSeq ?? result.expected_seq ?? seq));
            dirty = current !== saved; conflict = true; requested = false;
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
      if (disposed || !dirty) return Promise.resolve(state());
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
      }
      changed();
      return state();
    }
    function dispose() {disposed = true; requested = false; clearTimeout(timer); timer = null;}
    return {edit, flush, state, setCanonical, dispose};
  }

  function appendInline(parent, source) {
    const pattern = /(!?)\[([^\]]*)\]\(([^)\s]+)(?:\s+"([^"]*)")?\)|\*\*([^*]+)\*\*|__([^_]+)__|`([^`]+)`|\*([^*]+)\*|_([^_]+)_/g;
    let offset = 0, match;
    while ((match = pattern.exec(source))) {
      if (match.index > offset) parent.append(document.createTextNode(source.slice(offset, match.index)));
      const [, image, label, rawUrl, title, bold, boldAlt, code, italic, italicAlt] = match;
      if (rawUrl) {
        let url;
        try {url = new URL(rawUrl, document.baseURI);} catch {_text(parent, match[0]); offset = pattern.lastIndex; continue;}
        if (!['http:', 'https:', 'mailto:'].includes(url.protocol) && !rawUrl.startsWith('/') && !rawUrl.startsWith('#')) {
          _text(parent, match[0]); offset = pattern.lastIndex; continue;
        }
        if (image) {
          if (!['http:', 'https:'].includes(url.protocol)) {
            _text(parent, match[0]); offset = pattern.lastIndex; continue;
          }
          const attachment = rawUrl.match(/^\/api\/attachments\/(\d+)$/);
          if (attachment && typeof globalThis.lificSession?.resolve === 'function') {
            const resolved = globalThis.lificSession.resolve(`/attachments/${attachment[1]}`);
            if (!['private', 'public'].includes(resolved?.kind) || typeof resolved.url !== 'string') {
              _text(parent, match[0]); offset = pattern.lastIndex; continue;
            }
            url = new URL(resolved.url, document.baseURI);
          }
          if (!['http:', 'https:'].includes(url.protocol)) {
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
      const heading = line.match(/^(#{1,6})\s+(.+)$/);
      if (heading) {
        const node = document.createElement(`h${heading[1].length}`); appendInline(node, heading[2]); container.append(node); i++; continue;
      }
      if (/^>\s?/.test(line)) {
        const quote = document.createElement('blockquote'); appendInline(quote, line.replace(/^>\s?/, '')); container.append(quote); i++; continue;
      }
      if (/^\s*[-*+]\s+/.test(line) || /^\s*\d+[.)]\s+/.test(line)) {
        const ordered = /^\s*\d+[.)]\s+/.test(line), list = document.createElement(ordered ? 'ol' : 'ul');
        while (i < lines.length && (ordered ? /^\s*\d+[.)]\s+/ : /^\s*[-*+]\s+/).test(lines[i])) {
          const item = document.createElement('li'); appendInline(item, lines[i].replace(ordered ? /^\s*\d+[.)]\s+/ : /^\s*[-*+]\s+/, '')); list.append(item); i++;
        }
        container.append(list); continue;
      }
      const paragraph = document.createElement('p');
      while (i < lines.length && lines[i].trim() && !/^(#{1,6}\s|>\s?|```|\s*[-*+]\s+|\s*\d+[.)]\s+)/.test(lines[i])) {
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
    if (!input || !preview) return null;
    const pendingRequests = new Set();
    let showingPreview = true, destroyed = false;
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
      debounceMs: props.debounce_ms ?? 650, save: saveThroughRoute, onChange: renderState});
    function renderState() {
      if (destroyed) return;
      root.hidden = false;
      const state = queue.state();
      if (input.value !== state.text) input.value = state.text;
      input.disabled = !capabilities.edit;
      saveButton.hidden = showingPreview;
      saveButton.disabled = !capabilities.edit || !state.dirty;
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
    input.addEventListener('input', () => {
      if (destroyed || !capabilities.edit) return;
      const text = input.value;
      window.dispatchEvent(new CustomEvent('lific:issue-detail-intent', {detail: {
        route, action: {type: 'edit_description', description: text},
      }}));
      queue.edit(text); renderState();
    });
    const flush = async () => {if (!capabilities.edit) return; await queue.flush(); renderState();};
    saveButton.addEventListener('click', flush);
    previewButton.addEventListener('click', async () => {
      await flush();
      if (!queue.state().error && !queue.state().conflict) showingPreview = true;
      renderState();
    });
    editButton.addEventListener('click', () => {showingPreview = false; renderState(); input.focus();});
    input.addEventListener('keydown', event => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {event.preventDefault(); flush();}
    });
    function update(next = {}) {
      if (destroyed) return;
      if (next.route && (next.route.issue_id !== route.issue_id || next.route.generation !== route.generation)) return;
      if (next.capabilities) capabilities = {...next.capabilities};
      queue.setCanonical({text: next.text, savedDescription: next.saved_description, expectedSeq: next.expected_seq});
      renderState();
    }
    function dispose() {
      if (destroyed) return;
      destroyed = true; queue.dispose();
      for (const cancel of [...pendingRequests]) cancel();
    }
    input.value = props.text ?? ''; renderState();
    return {flush, update, dispose, queue, preview() {showingPreview = true; renderState();}, edit() {showingPreview = false; renderState(); input.focus();}};
  }

  const api = {createSaveQueue, renderMarkdown, mount};
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') window.lificIssueEditor = api;
})();
