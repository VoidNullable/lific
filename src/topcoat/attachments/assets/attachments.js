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
  const DEFAULT_UPLOAD_CAP = 10 * 1024 * 1024;
  const resizeMime = mime => ({'image/png':'image/png','image/jpeg':'image/jpeg','image/jpg':'image/jpeg','image/webp':'image/webp'})[mime.toLowerCase()] || null;
  function decideDownscale({width, height, bytes, mime}, cap = DEFAULT_UPLOAD_CAP) {
    const outputMime = resizeMime(mime), edge = Math.max(width, height);
    if (!outputMime || width <= 0 || height <= 0 || edge <= 2560 || (edge <= 4096 && bytes <= cap * .8)) return null;
    const scaled = {width: Math.max(1, Math.round(width * 2560 / edge)), height: Math.max(1, Math.round(height * 2560 / edge))};
    return {...scaled, outputMime, targetEdge: 2560, reason: bytes > cap * .8 ? 'size' : 'dimensions', estimatedBytes: Math.max(1024, Math.round(bytes * scaled.width * scaled.height / (width * height) * (outputMime === 'image/png' ? 1 : .9)))};
  }
  function parseUploadCap(message) {
    const match = String(message).match(/\(\s*max(?:imum)?[:\s]+(\d+)/i) || String(message).match(/max(?:imum)?(?:\s+size)?[:\s]+(\d+)\s*bytes/i);
    return match ? Number(match[1]) : null;
  }
  function replaceImageAlt(source, id, raw) {
    const alt = raw.replace(/[\r\n\t]+/g, ' ').replace(/[[\]]/g, '').replace(/\s{2,}/g, ' ').trim();
    return alt ? source.replace(new RegExp(`!\\[((?:\\\\.|[^\\]])*)\\]\\(/api/attachments/${Number(id)}\\)`), () => `![${alt}](/api/attachments/${Number(id)})`) : source;
  }
  async function imageFor(file, win) {
    const url = win.URL.createObjectURL(file);
    try {return await new Promise(resolve => {const image = new win.Image(); image.onload = () => resolve(image); image.onerror = () => resolve(null); image.src = url;});}
    finally {win.URL.revokeObjectURL(url);}
  }
  async function resizedImage(file, offer, win) {
    try {
      const image = await imageFor(file, win); if (!image) return file;
      const canvas = win.document.createElement('canvas'); canvas.width = offer.width; canvas.height = offer.height;
      canvas.getContext('2d').drawImage(image, 0, 0, offer.width, offer.height);
      const blob = await new Promise(resolve => canvas.toBlob(resolve, offer.outputMime, .85));
      return blob && blob.size < file.size ? new win.File([blob], file.name, {type: offer.outputMime, lastModified: Date.now()}) : file;
    } catch {return file;}
  }
  const isBigPaste = text => text.length > 6000 || text.split('\n').length > 60;
  function resizeCrop(rect,handle,point,bounds) {
    const clamp=(value,min,max)=>Math.max(min,Math.min(max,value));let left=rect.x,top=rect.y,right=rect.x+rect.w,bottom=rect.y+rect.h;
    const x=clamp(point.x,0,bounds.w),y=clamp(point.y,0,bounds.h);
    if(handle.includes('w'))left=clamp(Math.min(x,right-16),0,bounds.w);
    if(handle.includes('e'))right=clamp(Math.max(x,left+16),0,bounds.w);
    if(handle.includes('n'))top=clamp(Math.min(y,bottom-16),0,bounds.h);
    if(handle.includes('s'))bottom=clamp(Math.max(y,top+16),0,bounds.h);
    return {x:left,y:top,w:Math.max(0,right-left),h:Math.max(0,bottom-top)};
  }
  function annotation(file, {win, signal}) {
    const doc = win.document, previous = doc.activeElement, prompt = doc.createElement('div');
    prompt.className = 'tc-annotation-prompt'; prompt.setAttribute('role', 'status'); prompt.textContent = 'Annotate before upload? ';
    const button = (label, action, parent = prompt) => {const node = doc.createElement('button');node.type='button';node.textContent=label;node.addEventListener('click', action);parent.append(node);return node;};
    let timer, dialog = null, finished = false;
    return new Promise(resolve => {
      function finish(result) {if (finished) return;finished = true;win.clearTimeout(timer);win.removeEventListener('keydown', key);signal.removeEventListener('abort', aborted);prompt.remove();dialog?.remove();previous?.focus();resolve(result);}
      const aborted = () => finish(null);
      const key = event => {if(event.key==='Escape'){event.preventDefault();finish(file);}else if(!dialog&&event.key==='Enter'){event.preventDefault();void edit();}};
      async function edit() {
        win.clearTimeout(timer);prompt.remove();
        dialog = doc.createElement('dialog');dialog.className='tc-annotation';dialog.setAttribute('aria-label','Annotate image');doc.body.append(dialog);dialog.showModal();
        dialog.addEventListener('cancel',event=>{event.preventDefault();finish(file);});
        const image = await imageFor(file,win);if (finished) return;if(!image){finish(file);return;}
        const toolbar=doc.createElement('div');toolbar.className='tc-annotation__toolbar';dialog.append(toolbar);
        let tool='arrow',color='#ff3b30',shapes=[],crop=null,active=null;const history=[];
        const canvas=doc.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;canvas.dataset.attachmentAnnotationCanvas='';canvas.style.maxWidth='min(80vw, 100%)';canvas.style.maxHeight='65vh';canvas.style.touchAction='none';dialog.append(canvas);
        const ctx=canvas.getContext('2d'),stroke=Math.max(3,Math.min(10,Math.round(Math.min(canvas.width,canvas.height)/200)));
        const rect = shape => ({x:Math.min(shape.from.x,shape.to.x),y:Math.min(shape.from.y,shape.to.y),w:Math.abs(shape.to.x-shape.from.x),h:Math.abs(shape.to.y-shape.from.y)});
        function draw(context, guides=false) {
          context.clearRect(0,0,canvas.width,canvas.height);context.drawImage(image,0,0);context.lineWidth=stroke;context.lineCap='round';
          for(const shape of [...shapes,...(active&&!active.kind.startsWith('crop')?[active]:[])]) {
            context.strokeStyle=shape.color;context.fillStyle=shape.color;context.beginPath();
            if(shape.kind==='pen'){shape.points.forEach((p,i)=>i?context.lineTo(p.x,p.y):context.moveTo(p.x,p.y));context.stroke();}
            else if(shape.kind==='rect'){const r=rect(shape);context.strokeRect(r.x,r.y,r.w,r.h);}
            else if(shape.kind==='redact'){const r=rect(shape);context.fillStyle='#000';context.fillRect(r.x,r.y,r.w,r.h);}
            else {context.moveTo(shape.from.x,shape.from.y);context.lineTo(shape.to.x,shape.to.y);context.stroke();const angle=Math.atan2(shape.to.y-shape.from.y,shape.to.x-shape.from.x),length=stroke*5;context.beginPath();context.moveTo(shape.to.x,shape.to.y);context.lineTo(shape.to.x-length*Math.cos(angle-.45),shape.to.y-length*Math.sin(angle-.45));context.lineTo(shape.to.x-length*Math.cos(angle+.45),shape.to.y-length*Math.sin(angle+.45));context.closePath();context.fill();}
          }
          const area=active?.kind==='crop'?rect(active):crop;
          if(guides&&area){context.strokeStyle='#3b82f6';context.lineWidth=stroke;context.strokeRect(area.x,area.y,area.w,area.h);for(const handle of ['nw','n','ne','e','se','s','sw','w']){const p=handlePoint(area,handle);context.fillStyle='#3b82f6';context.fillRect(p.x-stroke*2,p.y-stroke*2,stroke*4,stroke*4);}}
        }
        const tools=[];for(const [id,label] of [['arrow','Arrow'],['rect','Rectangle'],['pen','Pen'],['redact','Redact'],['crop','Crop']]) {const node=button(label,()=>{tool=id;for(const [key,control] of tools)control.setAttribute('aria-pressed',String(key===id));},toolbar);node.setAttribute('aria-pressed',String(id===tool));tools.push([id,node]);}
        for(const value of ['#ff3b30','#ffb020','#22c55e','#3b82f6']) {const control=button(`Use colour ${value}`,()=>{color=value;},toolbar);control.style.background=value;}
        const handlePoint=(area,handle)=>({x:handle.includes('w')?area.x:handle.includes('e')?area.x+area.w:area.x+area.w/2,y:handle.includes('n')?area.y:handle.includes('s')?area.y+area.h:area.y+area.h/2});
        function snapshot(){if(history.length===60)history.shift();history.push({shapes:shapes.map(shape=>({...shape,points:shape.points?.map(p=>({...p}))})),crop:crop?{...crop}:null});}
        const undo=()=>{const previous=history.pop();if(previous){shapes=previous.shapes;crop=previous.crop;draw(ctx,true);}};
        button('Undo',undo,toolbar);button('Clear',()=>{snapshot();shapes=[];crop=null;draw(ctx,true);},toolbar);
        const point=event=>{const box=canvas.getBoundingClientRect();return {x:Math.max(0,Math.min(canvas.width,(event.clientX-box.left)*canvas.width/box.width)),y:Math.max(0,Math.min(canvas.height,(event.clientY-box.top)*canvas.height/box.height))};};
        canvas.addEventListener('pointerdown',event=>{snapshot();const p=point(event);const tolerance=14*canvas.width/canvas.getBoundingClientRect().width;const handle=tool==='crop'&&crop?['nw','n','ne','e','se','s','sw','w'].find(handle=>{const h=handlePoint(crop,handle);return Math.abs(h.x-p.x)<=tolerance&&Math.abs(h.y-p.y)<=tolerance;}):null;active=handle?{kind:'crop-resize',handle,initial:{...crop}}:{kind:tool,color,from:p,to:p,points:[p]};canvas.setPointerCapture(event.pointerId);});
        canvas.addEventListener('pointermove',event=>{if(!active)return;if(active.kind==='crop-resize'){crop=resizeCrop(active.initial,active.handle,point(event),{w:canvas.width,h:canvas.height});draw(ctx,true);return;}active.to=point(event);if(active.kind==='pen')active.points.push(active.to);draw(ctx,true);});
        const end=()=>{if(!active)return;if(active.kind==='crop')crop=rect(active);else if(active.kind!=='crop-resize')shapes.push(active);active=null;draw(ctx,true);};
        canvas.addEventListener('pointerup',end);canvas.addEventListener('pointercancel',end);
        dialog.addEventListener('keydown',event=>{if((event.ctrlKey||event.metaKey)&&event.key==='z'){event.preventDefault();undo();}});
        button('Cancel annotation',()=>finish(file),dialog);
        const accept=button('Upload annotated image',async()=>{
          accept.disabled=true;end();if(!shapes.length&&!crop){finish(file);return;}
          try {draw(ctx);const area=crop&&crop.w>=8&&crop.h>=8?crop:{x:0,y:0,w:canvas.width,h:canvas.height};const output=doc.createElement('canvas');output.width=Math.max(1,Math.round(area.w));output.height=Math.max(1,Math.round(area.h));output.getContext('2d').drawImage(canvas,area.x,area.y,area.w,area.h,0,0,output.width,output.height);const mime=['image/jpeg','image/jpg'].includes(file.type)?'image/jpeg':'image/png';const blob=await new Promise(resolve=>output.toBlob(resolve,mime,mime==='image/jpeg'?.92:undefined));const name=file.name.replace(/\.[^.]+$/,'')+'-annotated.'+({'image/jpeg':'jpg','image/webp':'webp'}[mime]||'png');finish(blob?new win.File([blob],name,{type:mime,lastModified:Date.now()}):file);}
          catch{finish(file);}
        },dialog);draw(ctx,true);tools[0][1].focus();
      }
      button('Annotate',()=>void edit());button('Skip annotation',()=>finish(file));doc.body.append(prompt);timer=win.setTimeout(()=>finish(file),4000);win.addEventListener('keydown',key);signal.addEventListener('abort',aborted,{once:true});if(signal.aborted)finish(null);
    });
  }
  function createComposer({root, client, target = null, onUploaded = () => {}, text = null, textarea = null, concurrency = 3, onStatus = () => {}, win = root.ownerDocument.defaultView}) {
    const doc=root.ownerDocument,list=doc.createElement('ul');list.className='tc-pending-uploads';list.setAttribute('aria-label','Pending uploads');root.append(list);
    const items=[],waiters=[],listeners=[];let sequence=0,active=0,disposed=false,preparing=Promise.resolve(),busy=false,pasteOffer=null,suspended=false,identity=client.audience();
    let cap=DEFAULT_UPLOAD_CAP,resizeChoice=null;
    try {cap=Number(win.localStorage.getItem('lific_upload_cap'))||cap;} catch {}
    const valid=item=>!disposed&&items.includes(item)&&item.identity===client.audience();
    const pending=()=>Boolean(pasteOffer)||items.some(item=>['preparing','offer','queued','uploading'].includes(item.status));
    const action=(label,callback,parent)=>{const node=doc.createElement('button');node.type='button';node.textContent=label;node.addEventListener('click',callback);parent.append(node);return node;};
    function render() {
      const focused=doc.activeElement?.closest?.('[data-attachment-pending]'),focus=focused&&list.contains(focused)&&doc.activeElement.matches('input')?{id:focused.dataset.attachmentPending,start:doc.activeElement.selectionStart,end:doc.activeElement.selectionEnd}:null;
      list.replaceChildren();list.hidden=!items.length;
      for(const item of items){const row=doc.createElement('li');row.dataset.attachmentPending=String(item.id);const label=doc.createElement('span');label.textContent=`${item.file.name} · ${item.status==='uploading'?`${item.loaded} of ${item.file.size} bytes`:item.status}`;row.append(label);
        if(item.error){const error=doc.createElement('p');error.textContent=item.error;row.append(error);}
        if(item.status==='error'){if(!item.attachment)action(`Retry ${item.file.name}`,()=>retry(item.id),row);action(`Dismiss ${item.file.name}`,()=>cancel(item.id),row);}
        else if(item.status==='offer'){action(`Resize to 2560px (~${Math.round(item.offer.estimatedBytes/1024)} KB)`,()=>void resize(item),row);action('Keep original',()=>{resizeChoice='original';item.status='queued';render();pump();},row);action(`Cancel upload of ${item.file.name}`,()=>cancel(item.id),row);}
        else if(item.status==='alt'){const field=doc.createElement('input');field.type='text';field.setAttribute('aria-label',`Describe ${item.file.name}`);field.placeholder='Describe this image';field.value=item.altDraft||'';field.addEventListener('input',()=>{item.altDraft=field.value;});row.append(field);const apply=()=>{if(valid(item)){text.write(replaceImageAlt(text.read(),item.attachment.id,field.value));cancel(item.id);}};action('Apply image description',apply,row);action('Skip image description',()=>cancel(item.id),row);field.addEventListener('keydown',event=>{if(event.key==='Enter'){event.preventDefault();apply();}if(event.key==='Escape'){event.preventDefault();cancel(item.id);}});}
        else{const progress=doc.createElement('progress');progress.max=item.file.size||1;progress.value=item.loaded||0;progress.setAttribute('aria-label',`Uploading ${item.file.name}`);row.append(progress);action(`Cancel upload of ${item.file.name}`,()=>cancel(item.id),row);}
        list.append(row);
      }
      if(focus){const field=list.querySelector(`[data-attachment-pending="${focus.id}"] input`);if(field){field.focus({preventScroll:true});field.setSelectionRange(focus.start,focus.end);}}
      const next=pending();if(next!==busy){busy=next;root.dispatchEvent(new win.CustomEvent('lific:attachment-busy',{bubbles:true,detail:{busy}}));}
      if(!next){while(waiters.length)waiters.shift()(items.filter(item=>item.status==='error').map(item=>item.error));}
    }
    async function resize(item){resizeChoice='resize';item.status='preparing';render();item.file=await resizedImage(item.file,item.offer,win);if(valid(item)){item.status='queued';render();pump();}}
    async function prepare(item,source) {
      if(['paste','drop','camera'].includes(source)&&['image/png','image/jpeg','image/jpg','image/webp','image/bmp'].includes(item.file.type.toLowerCase())){const file=await annotation(item.file,{win,signal:item.abort.signal});if(!valid(item)||!file)return;item.file=file;}
      if(resizeMime(item.file.type)){const image=await imageFor(item.file,win);if(!valid(item))return;item.offer=image?decideDownscale({width:image.naturalWidth,height:image.naturalHeight,bytes:item.file.size,mime:item.file.type},cap):null;}
      if(!valid(item))return;
      if(item.offer&&resizeChoice!=='original'){if(resizeChoice==='resize'){await resize(item);return;}item.status='offer';}
      else item.status='queued';render();pump();
    }
    function pump(){if(disposed||suspended)return;for(const item of items){if(active>=concurrency)break;if(item.status==='queued'&&!item.paused){void start(item);}}}
    async function start(item){
      const attempt=(item.attempt||0)+1;item.attempt=attempt;const validAttempt=()=>valid(item)&&item.attempt===attempt;
      active++;item.status='uploading';item.loaded=0;item.error=null;render();onStatus(`Uploading ${item.file.name}…`);
      try {
        const transfer=client.upload(item.file,{target:typeof target==='function'?target():target,onProgress:progress=>{if(validAttempt()){item.loaded=progress.loaded;const progressNode=list.querySelector(`[data-attachment-pending="${item.id}"] progress`);if(progressNode)progressNode.value=progress.loaded;}}});item.transfer=transfer;
        const result=await transfer.result;if(!validAttempt())return;
        if(result.ok){item.attachment=result.data;await onUploaded(result.data,markdown(result.data));if(!validAttempt())return;onStatus(`Uploaded ${result.data.filename}.`);if(result.data.mime?.startsWith('image/')&&text)item.status='alt';else items.splice(items.indexOf(item),1);}
        else {item.status='error';item.error=result.error;const learned=parseUploadCap(result.error);if(learned>0){cap=learned;try {win.localStorage.setItem('lific_upload_cap',String(cap));} catch {}}onStatus(result.error);}
      }catch(error){if(validAttempt()){item.status='error';item.error=item.attachment?`Uploaded ${item.file.name}, but the view could not update: ${error.message}`:error.message;onStatus(item.error);}}
      finally{active--;item.transfer=null;render();pump();}
    }
    function cancel(id){const item=items.find(row=>row.id===id);if(item){items.splice(items.indexOf(item),1);item.abort.abort();item.transfer?.abort();render();pump();}}
    function retry(id){const item=items.find(row=>row.id===id);if(item?.status==='error'&&!item.attachment){item.status='queued';item.paused=false;render();pump();}}
    function wait(){return pending()?new Promise(resolve=>waiters.push(resolve)):Promise.resolve(items.filter(item=>item.status==='error').map(item=>item.error));}
    function enqueue(files,{source='picker'}={}){if(disposed)return Promise.resolve([]);for(const file of files||[]){const item={id:++sequence,file,status:'preparing',identity:client.audience(),abort:new win.AbortController(),loaded:0};items.push(item);preparing=preparing.then(()=>valid(item)?prepare(item,source):undefined).catch(error=>{if(valid(item)){item.status='error';item.error=error.message;render();}});}render();return wait();}
    function cancelAll({retain=false}={}){suspended=true;if(pasteOffer){pasteOffer.node.remove();pasteOffer=null;}for(const item of [...items]){if(retain&&['queued','uploading'].includes(item.status)){item.attempt=(item.attempt||0)+1;item.paused=true;item.status='error';item.error='Upload canceled.';item.transfer?.abort();}else cancel(item.id);}suspended=false;render();onStatus('Upload canceled.');}
    function clearPaste(){pasteOffer?.node.remove();pasteOffer=null;render();}
    function pasteInline(){if(!pasteOffer)return;const value=pasteOffer.value,start=textarea.selectionStart??text.read().length,end=textarea.selectionEnd??start;clearPaste();const current=text.read();text.write(current.slice(0,start)+value+current.slice(end));textarea.setSelectionRange(start+value.length,start+value.length);}
    function pasteAttachment(){if(!pasteOffer)return;const value=pasteOffer.value;clearPaste();const at=new Date(),pad=n=>String(n).padStart(2,'0');const name=`paste-${at.getFullYear()}${pad(at.getMonth()+1)}${pad(at.getDate())}-${pad(at.getHours())}${pad(at.getMinutes())}.txt`;void enqueue([new win.File([value],name,{type:'text/plain'})],{source:'paste'});}
    if(textarea&&text){
      const paste=event=>{if(textarea.disabled||textarea.readOnly)return;const files=Array.from(event.clipboardData?.files||[]);if(files.length){event.preventDefault();void enqueue(files,{source:'paste'});return;}const value=event.clipboardData?.getData('text/plain')||'';if(isBigPaste(value)){event.preventDefault();if(pasteOffer)pasteInline();const node=doc.createElement('div');node.className='tc-paste-offer';node.setAttribute('role','status');node.textContent=`${value.split('\n').length} lines pasted. `;pasteOffer={value,start:textarea.selectionStart??text.read().length,end:textarea.selectionEnd??text.read().length,node};action('Attach pasted text',pasteAttachment,node);action('Paste inline',pasteInline,node);root.append(node);render();}};
      const key=event=>{if(pasteOffer&&event.key==='Enter'){event.preventDefault();event.stopPropagation();pasteAttachment();}else if(pasteOffer&&event.key==='Escape'){event.preventDefault();event.stopPropagation();pasteInline();}};
      const drop=event=>{if(!textarea.disabled&&!textarea.readOnly&&event.dataTransfer?.files?.length){event.preventDefault();void enqueue(event.dataTransfer.files,{source:'drop'});}};
      const drag=event=>{if(event.dataTransfer?.types?.includes('Files'))event.preventDefault();};
      for(const [name,fn] of [['paste',paste],['keydown',key],['drop',drop],['dragover',drag]]){textarea.addEventListener(name,fn);listeners.push(()=>textarea.removeEventListener(name,fn));}
    }
    const changed=()=>{const next=client.audience();if(next!==identity){identity=next;cancelAll();}};
    for(const name of ['lific:account-change','lific:scope-change','lific:session-change']){win.addEventListener(name,changed);listeners.push(()=>win.removeEventListener(name,changed));}
    return {enqueue,retry,cancel,cancelAll,wait,get pending(){return pending();},get items(){return items;},retryAll(){for(const item of items){if(item.status==='error'&&!item.attachment){item.status='queued';item.paused=false;}}render();pump();return wait();},dispose(){disposed=true;cancelAll();list.remove();listeners.forEach(remove=>remove());}};
  }
  function attach(root, {client, text = null, target: requestedTarget = null, onUploaded = () => {}, onDeleted = () => {}, win = root.ownerDocument.defaultView} = {}) {
    const form = root.querySelector('[data-attachment-upload]');
    const input = form?.querySelector('[data-attachment-files]');
    const submit = form?.querySelector('[type=submit]');
    const cancel = form?.querySelector('[data-attachment-cancel]');
    const progress = form?.querySelector('[data-attachment-progress]');
    const status = form?.querySelector('[data-attachment-status]');
    const objectUrls = new Set(), transfers = new Set(), operations = new Set();
    let disposed = false, running = false, remaining = [], uploadGeneration = 0, scopeGeneration = 0, identity = client.audience();
    const say = message => {if (status) status.textContent = message;};
    const composer = createComposer({root, client, target: requestedTarget, onUploaded, text, textarea: text ? root.parentElement?.querySelector('[data-editor-input]') : null, onStatus: say, win});
    function cancelAll() {uploadGeneration++; composer.cancelAll({retain: true}); say('Upload canceled.');}
    function clearScope() {
      scopeGeneration++; cancelAll(); composer.cancelAll();
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
      if (!remaining.length && !composer.items.some(item => item.status === 'error')) {say('Choose a file to upload.'); return;}
      const current = ++uploadGeneration;
      running = true; input.disabled = true; submit.disabled = true; cancel.hidden = false;
      progress.hidden = true;
      try {
        const selected = remaining; remaining = [];
        if (selected.length) await composer.enqueue(selected);
        else await composer.retryAll();
        if (current === uploadGeneration && !disposed && !composer.items.some(item => item.status === 'error')) input.value = '';
      } finally {
        running = false;
        if (!disposed) {input.disabled = false; submit.disabled = false; cancel.hidden = true; progress.hidden = true;}
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
    return {enqueue: (files, options) => composer.enqueue(files, options), get pending() {return composer.pending;}, cancel() {composer.cancelAll(); remaining = []; if(input) input.value = '';}, dispose() {
      disposed = true; clearScope(); composer.dispose();
      form?.removeEventListener('submit', upload); input?.removeEventListener('change', choose);
      cancel?.removeEventListener('click', cancelAll); root.removeEventListener('click', click);
      win.removeEventListener('lific:account-change', accountChanged); win.removeEventListener('lific:scope-change', accountChanged);
    }};
  }

  globalThis.LificTopcoatAttachments = {createClient, createComposer, attach, viewerKind, filename, markdown, decideDownscale, parseUploadCap, replaceImageAlt, resizeCrop, isBigPaste, MAX_INLINE_BYTES};
})();
