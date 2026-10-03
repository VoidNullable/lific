(() => {
  'use strict';
  const MAX_INLINE_BYTES = 10 * 1024 * 1024;
  const failure = (error, status = null, canceled = false, code = null) => ({ok: false, error, status, canceled, code});
  const refused = () => failure('Attachments are read-only in the public view.', 403);
  function target(value) {
    if (value === null || value === undefined) return null;
    if (!['issue', 'page', 'comment'].includes(value.entity_type) || !Number.isSafeInteger(value.entity_id) || value.entity_id <= 0) throw new TypeError('Invalid attachment target.');
    return value;
  }
  function filename(disposition, fallback = 'download') {
    const extended = disposition?.match(/filename\*\s*=\s*UTF-8''([^;]+)/i);
    const quoted = disposition?.match(/filename\s*=\s*"([^"]*)"/i);
    const plain = disposition?.match(/filename\s*=\s*([^;]+)/i);
    let value = quoted?.[1] || plain?.[1]?.trim() || fallback;
    if (extended) {try {value = decodeURIComponent(extended[1].trim());} catch { /* Keep ordinary filename. */ }}
    return value.split(/[\\/]/).pop().replace(/[\u0000-\u001f\u007f]/g, '') || 'download';
  }
  function viewerKind(attachment) {
    const mime = (attachment.mime || '').toLowerCase().split(';')[0].trim();
    const extension = attachment.filename?.split(/[\\/]/).pop().split('.').pop().toLowerCase() || '';
    let kind;
    if (mime.startsWith('image/')) kind = 'image';
    else if (mime.startsWith('video/')) kind = 'video';
    else if (mime.startsWith('audio/')) kind = 'audio';
    else if (['application/zip'].includes(mime)) kind = 'zip';
    else if (['application/vnd.sqlite3', 'application/x-sqlite3'].includes(mime)) kind = 'sqlite';
    else if (mime === 'application/json') kind = 'json';
    else if (['png','jpg','jpeg','gif','webp','avif','bmp','ico','svg'].includes(extension)) kind = 'image';
    else if (['patch','diff'].includes(extension)) kind = 'diff';
    else if (['csv','tsv','tab'].includes(extension)) kind = 'csv';
    else if (extension === 'json') kind = 'json';
    else if (extension === 'zip') kind = 'zip';
    else if (['db','sqlite','sqlite3'].includes(extension)) kind = 'sqlite';
    else if (['mp4','webm','m4v'].includes(extension)) kind = 'video';
    else if (['mp3','ogg','oga','opus','weba'].includes(extension)) kind = 'audio';
    else if (mime.startsWith('text/') || /^(txt|text|log|out|err|md|markdown|rst|adoc|rs|ts|tsx|js|jsx|mjs|cjs|svelte|vue|py|rb|go|java|kt|kts|swift|c|h|cc|cpp|hpp|cs|php|pl|lua|r|scala|clj|ex|exs|erl|hs|ml|zig|nim|dart|sql|graphql|gql|proto|sh|bash|zsh|fish|ps1|bat|cmd|yaml|yml|toml|ini|cfg|conf|env|properties|html|htm|xml|css|scss|sass|less|lock|gitignore|dockerfile|makefile|cmake|gradle)$/.test(extension) || /^(makefile|dockerfile|license|readme|changelog)$/i.test(attachment.filename)) kind = 'text';
    else kind = 'file';
    return ['text','diff','csv','json'].includes(kind) && attachment.size_bytes > MAX_INLINE_BYTES ? 'file' : kind;
  }
  function createClient({session, win = globalThis.window, fetch: fetcher = globalThis.fetch, createXHR = () => new XMLHttpRequest()} = {}) {
    const storedToken = () => win.localStorage.getItem('lific_token');
    const audience = () => `${session.state.publicProject || ''}:${session.state.user?.id || ''}:${storedToken() || ''}`;
    const changed = () => failure('The account or view changed. Try again.', null, true, 'audience_changed');
    function url(id, variant = 'original') {
      if (!Number.isSafeInteger(id) || id <= 0 || !['original', 'thumbnail', 'preview'].includes(variant)) throw new TypeError('Invalid attachment request.');
      return session.resolve(`/attachments/${id}${variant === 'original' ? '' : `/${variant}`}`).url;
    }
    function upload(file, {target: requestedTarget = null, onProgress = () => {}} = {}) {
      const link = target(requestedTarget);
      const resolved = session.resolve('/attachments', 'POST');
      if (resolved.kind !== 'private') return {result: Promise.resolve(refused()), abort() {}};
      const form = new FormData();
      form.append('file', file, file.name);
      if (link) {form.append('entity_type', link.entity_type); form.append('entity_id', String(link.entity_id));}
      const started = audience(), xhr = createXHR();
      let settled = false;
      const result = new Promise(resolve => {
        const finish = outcome => {
          if (!settled) {
            settled = true;
            win.removeEventListener('lific:account-change', accountChanged);
            win.removeEventListener('lific:scope-change', accountChanged);
            resolve(outcome);
          }
        };
        const accountChanged = () => {if (audience() !== started) xhr.abort();};
        win.addEventListener('lific:account-change', accountChanged);
        win.addEventListener('lific:scope-change', accountChanged);
        xhr.open('POST', resolved.url);
        const token = storedToken();
        if (token) xhr.setRequestHeader('Authorization', `Bearer ${token}`);
        xhr.upload.onprogress = event => onProgress({loaded: event.loaded, total: event.lengthComputable ? event.total : file.size});
        xhr.upload.onload = () => onProgress({loaded: file.size, total: file.size});
        xhr.onload = () => {
          if (started !== audience()) {finish(changed()); return;}
          let body;
          try {body = JSON.parse(xhr.responseText);} catch {body = null;}
          if (xhr.status === 401) session.clearSession();
          const valid = body && Number.isSafeInteger(body.id) && typeof body.filename === 'string' && typeof body.mime === 'string' && typeof body.size === 'number';
          finish(xhr.status >= 200 && xhr.status < 300 && valid ? {ok: true, status: xhr.status, data: body}
            : failure(body?.error || `Invalid upload response (HTTP ${xhr.status})`, xhr.status));
        };
        xhr.onerror = () => finish(failure("Couldn't reach the server. Check your connection and try again."));
        xhr.ontimeout = () => finish(failure('Upload timed out.'));
        xhr.onabort = () => finish(started === audience() ? failure('Upload canceled.', null, true) : changed());
        xhr.send(form);
      });
      return {result, abort() {if (!settled) xhr.abort();}};
    }
    async function streamDownload(id, {open, variant = 'original', range = null, filename: fallback = 'download', signal = undefined, onProgress = () => {}}) {
      const resolved = session.resolve(`/attachments/${id}${variant === 'original' ? '' : `/${variant}`}`), started = audience();
      if (!['private', 'public'].includes(resolved.kind)) return refused();
      const headers = new Headers();
      if (resolved.kind === 'private' && storedToken()) headers.set('Authorization', `Bearer ${storedToken()}`);
      if (range) headers.set('Range', range);
      const controller = new AbortController();
      let reader, destination, status = null;
      const cancelTransfer = () => {
        void Promise.resolve().then(() => reader?.cancel(controller.signal.reason)).catch(() => {});
        void Promise.resolve().then(() => destination?.abort(controller.signal.reason)).catch(() => {});
      };
      const abort = () => controller.abort(signal?.reason);
      const accountChanged = () => {if (started !== audience()) controller.abort();};
      const requireCurrent = () => {
        if (started !== audience()) throw new Error('audience_changed');
        controller.signal.throwIfAborted();
      };
      controller.signal.addEventListener('abort', cancelTransfer, {once: true});
      signal?.addEventListener('abort', abort, {once: true});
      if (signal?.aborted) abort();
      win.addEventListener('lific:account-change', accountChanged);
      win.addEventListener('lific:scope-change', accountChanged);
      try {
        requireCurrent();
        const response = await fetcher(url(id, variant), {headers, signal: controller.signal, credentials: resolved.kind === 'public' ? 'omit' : 'same-origin'});
        status = response.status;
        requireCurrent();
        if (!response.ok) {
          let body;
          try {body = await response.json();} catch {body = null;}
          requireCurrent();
          if (status === 401 && resolved.kind === 'private') session.clearSession();
          return {...failure(body?.error || `HTTP ${status}`, status), contentRange: response.headers.get('Content-Range')};
        }
        const metadata = {status, filename: filename(response.headers.get('Content-Disposition'), fallback), contentType: response.headers.get('Content-Type'), contentRange: response.headers.get('Content-Range'), acceptRanges: response.headers.get('Accept-Ranges'), contentLength: response.headers.get('Content-Length')};
        reader = response.body.getReader();
        destination = await open(metadata);
        let loaded = 0;
        for (;;) {
          requireCurrent();
          const {done, value} = await reader.read();
          requireCurrent();
          if (done) break;
          await destination.write(value);
          loaded += value.byteLength;
          onProgress({loaded, total: metadata.contentLength === null ? null : Number(metadata.contentLength)});
        }
        await destination.close();
        requireCurrent();
        return {ok: true, ...metadata, loaded};
      } catch (error) {
        await Promise.resolve().then(() => reader?.cancel(error)).catch(() => {});
        await Promise.resolve().then(() => destination?.abort(error)).catch(() => {});
        return audience() !== started ? changed() : failure(error.message || 'Download failed.', status, controller.signal.aborted || error.name === 'AbortError');
      } finally {
        reader?.releaseLock();
        signal?.removeEventListener('abort', abort);
        controller.signal.removeEventListener('abort', cancelTransfer);
        win.removeEventListener('lific:account-change', accountChanged);
        win.removeEventListener('lific:scope-change', accountChanged);
      }
    }
    return {
      upload, url, streamDownload, audience,
      async text(id, {signal} = {}) {
        const decoder = new TextDecoder();
        const parts = [];
        let length = 0;
        const result = await streamDownload(id, {signal, open() {return {
          write(chunk) {length += chunk.byteLength; if (length > MAX_INLINE_BYTES) throw new Error('File is too large to preview inline.'); parts.push(decoder.decode(chunk, {stream: true}));},
          close() {parts.push(decoder.decode());}, abort() {parts.length = 0;},
        };}});
        return result.ok ? {...result, text: parts.join('')} : result;
      },
      async thumbnail(id, {signal} = {}) {
        const chunks = [];
        let length = 0;
        const result = await streamDownload(id, {signal, variant: 'thumbnail', open(metadata) {return {
          write(chunk) {length += chunk.byteLength; if (length > MAX_INLINE_BYTES) throw new Error('Thumbnail is too large.'); chunks.push(chunk);},
          close() {}, abort() {chunks.length = 0;},
        };}});
        return result.ok ? {...result, blob: new Blob(chunks, {type: result.contentType})} : result;
      },
      list(requestedTarget) {const link = target(requestedTarget); if (!link) throw new TypeError('Attachment list requires a target.'); return session.request(`/attachments?${new URLSearchParams(link)}`);},
      preview(id, {signal} = {}) {return session.request(`/attachments/${id}/preview`, {signal});},
      remove(id, {signal} = {}) {return session.resolve(`/attachments/${id}`, 'DELETE').kind === 'private' ? session.request(`/attachments/${id}`, {method: 'DELETE', signal}) : Promise.resolve(refused());},
      altText(id, alt_text) {return session.resolve(`/attachments/${id}`, 'PATCH').kind === 'private' ? session.request(`/attachments/${id}`, {method: 'PATCH', body: JSON.stringify({alt_text})}) : Promise.resolve(refused());},
    };
  }
  function markdown(attachment) {
    const label = (attachment.alt_text || attachment.filename).replace(/[\\[\]]/g, '\\$&');
    return `${attachment.mime.startsWith('image/') ? '!' : ''}[${label}](/api/attachments/${attachment.id})`;
  }
  function attach(root, {client, target: requestedTarget = null, onUploaded = () => {}, onDeleted = () => {}, win = root.ownerDocument.defaultView} = {}) {
    const form = root.querySelector('[data-attachment-upload]');
    const input = form?.querySelector('[data-attachment-files]');
    const submit = form?.querySelector('[type=submit]');
    const cancel = form?.querySelector('[data-attachment-cancel]');
    const progress = form?.querySelector('[data-attachment-progress]');
    const status = form?.querySelector('[data-attachment-status]');
    const objectUrls = new Set(), transfers = new Set(), operations = new Set();
    let disposed = false, running = false, remaining = [], uploadGeneration = 0, scopeGeneration = 0, identity = client.audience();
    const say = message => {if (status) status.textContent = message;};
    function cancelAll() {uploadGeneration++; for (const transfer of transfers) transfer.abort(); say('Upload canceled.');}
    function clearScope() {
      scopeGeneration++; cancelAll();
      for (const controller of operations) controller.abort();
      for (const src of objectUrls) win.URL.revokeObjectURL(src);
      objectUrls.clear();
      for (const media of root.querySelectorAll('video,audio')) {media.pause(); media.removeAttribute('src'); media.load();}
      for (const image of root.querySelectorAll('[data-attachment-image]')) image.removeAttribute('src');
    }
    function accountChanged() {
      const next = client.audience();
      if (next !== identity) {identity = next; root.hidden = true; clearScope(); remaining = []; if (input) input.value = ''; say('');}
    }
    async function operation(run) {
      const controller = new AbortController();
      operations.add(controller);
      try {return await run(controller.signal);}
      finally {operations.delete(controller);}
    }
    function choose() {remaining = Array.from(input.files || []);}
    async function upload(event) {
      event.preventDefault();
      if (running || disposed) return;
      if (!remaining.length) {say('Choose a file to upload.'); return;}
      const current = ++uploadGeneration;
      running = true; input.disabled = true; submit.disabled = true; cancel.hidden = false;
      const queue = remaining.slice(), failed = [], bytes = new Map(queue.map(file => [file, 0]));
      progress.hidden = false; progress.max = queue.reduce((sum, file) => sum + file.size, 0); progress.value = 0;
      const errors = [];
      async function next() {
        for (;;) {
          const file = queue.shift();
          if (!file || current !== uploadGeneration || disposed) break;
          say(`Uploading ${file.name}…`);
          const transfer = client.upload(file, {target: requestedTarget, onProgress(value) {
            if (current === uploadGeneration && !disposed) {bytes.set(file, value.loaded); progress.value = Array.from(bytes.values()).reduce((sum, count) => sum + count, 0);}
          }});
          transfers.add(transfer);
          const result = await transfer.result;
          transfers.delete(transfer);
          if (current !== uploadGeneration || disposed) break;
          if (result.ok) {
            remaining = remaining.filter(pending => pending !== file);
            try {await onUploaded(result.data, markdown(result.data)); if (current === uploadGeneration && !disposed) say(`Uploaded ${result.data.filename}.`);}
            catch (error) {errors.push(`Uploaded ${result.data.filename}, but the view could not update: ${error.message}`);}
          } else {failed.push(file); errors.push(result.error);}
        }
      }
      try {await Promise.all(Array.from({length: Math.min(3, queue.length)}, next));}
      finally {
        running = false;
        if (!disposed) {
          input.disabled = false; submit.disabled = false; cancel.hidden = true; progress.hidden = true;
          if (current === uploadGeneration) {remaining = failed; if (!failed.length) input.value = ''; if (errors.length) say(errors.join(' '));}
        }
      }
    }
    async function click(event) {
      const remove = event.target.closest('[data-attachment-delete]');
      const preview = event.target.closest('[data-attachment-preview]');
      if (remove && root.contains(remove)) {
        remove.disabled = true;
        const id = Number(remove.dataset.attachmentDelete), current = scopeGeneration, card = remove.closest('[data-attachment-id]');
        try {
          const result = await operation(signal => client.remove(id, {signal}));
          if (!disposed && current === scopeGeneration) {
            if (result.ok) {card.remove(); try {await onDeleted(id);} catch (error) {say(`Deleted the file, but the view could not update: ${error.message}`);}}
            else card.querySelector('[data-attachment-message]').textContent = result.error;
          }
        } catch (error) {if (!disposed && current === scopeGeneration) card.querySelector('[data-attachment-message]').textContent = error.message;}
        finally {if (card.isConnected) remove.disabled = false;}
      }
      if (preview && root.contains(preview)) {
        preview.disabled = true;
        const card = preview.closest('[data-attachment-id]'), output = card.querySelector('[data-attachment-content]'), current = scopeGeneration;
        const id = Number(card.dataset.attachmentId);
        try {
          const result = await operation(signal => ['zip','sqlite'].includes(card.dataset.attachmentKind) ? client.preview(id, {signal}) : client.text(id, {signal}));
          if (!disposed && current === scopeGeneration) {
            output.textContent = result.ok ? (result.text ?? JSON.stringify(result.data, null, 2)) : result.error;
            output.hidden = false;
          }
        } catch (error) {if (!disposed && current === scopeGeneration) {output.textContent = error.message; output.hidden = false;}}
        finally {preview.disabled = false;}
      }
    }
    for (const image of root.querySelectorAll('[data-attachment-image]')) {
      const id = Number(image.dataset.attachmentImage), current = scopeGeneration;
      void operation(signal => client.thumbnail(id, {signal})).then(result => {
        if (disposed || current !== scopeGeneration) return;
        if (result.ok) {const src = win.URL.createObjectURL(result.blob); objectUrls.add(src); image.src = src;}
        else if (result.status === 404) image.src = client.url(id);
        else {image.hidden = true; image.closest('[data-attachment-id]').querySelector('[data-attachment-message]').textContent = result.error;}
      }).catch(error => {if (!disposed && current === scopeGeneration) image.closest('[data-attachment-id]').querySelector('[data-attachment-message]').textContent = error.message;});
    }
    form?.addEventListener('submit', upload); input?.addEventListener('change', choose);
    cancel?.addEventListener('click', cancelAll); root.addEventListener('click', click);
    win.addEventListener('lific:account-change', accountChanged); win.addEventListener('lific:scope-change', accountChanged);
    return {dispose() {
      disposed = true; clearScope();
      form?.removeEventListener('submit', upload); input?.removeEventListener('change', choose);
      cancel?.removeEventListener('click', cancelAll); root.removeEventListener('click', click);
      win.removeEventListener('lific:account-change', accountChanged); win.removeEventListener('lific:scope-change', accountChanged);
    }};
  }

  globalThis.LificTopcoatAttachments = {createClient, attach, viewerKind, filename, markdown, MAX_INLINE_BYTES};
})();
