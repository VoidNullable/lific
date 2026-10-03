(() => {
  'use strict';

  const escapeHtml = value => String(value ?? '').replace(/[&<>"']/g, char => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[char]);
  const validUsername = value => /^[a-zA-Z0-9_-]{2,}$/.test(value);
  const validEmail = value => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value);
  const recentAuth = result => result?.status === 403 &&
    (result.code === 'recent_auth_required' || /recent.{0,20}(session|auth)|sign in again/i.test(result.error ?? ''));

  function controller(env) {
    const state = {busy:false, error:'', instance:null, user:null, keys:[], bots:[], users:[], settings:null,
      pendingAction:null, pendingPatch:{}, secret:null, sectionError:''};
    const api = (path, options={}) => env.session.request(path, options);
    const send = (path, method, body) => api(path, {method, ...(body === undefined ? {} : {body:JSON.stringify(body)})});
    const done = result => result?.ok ? result.data : null;
    const update = patch => Object.assign(state, patch);
    const loadInstance = async () => {
      const result = await api('/instance');
      if (!result.ok) { update({error:result.error}); return false; }
      update({instance:result.data});
      return true;
    };
    const loadAccount = async () => {
      const result = await env.session.refreshAccount();
      if (!result.ok) { update({error:result.error}); return false; }
      update({user:result.data, error:''});
      return true;
    };
    const loadAccountData = async () => {
      const [keys, bots] = await Promise.all([api('/auth/keys'), api('/auth/bots')]);
      update({keys:done(keys) ?? [], bots:done(bots) ?? [], sectionError:!keys.ok ? keys.error : (!bots.ok ? bots.error : '')});
      return keys.ok && bots.ok;
    };
    const loadAdmin = async () => {
      if (!state.user?.is_admin) return false;
      const [settings, users] = await Promise.all([api('/instance/settings'), api('/users')]);
      if (settings.ok) update({settings:settings.data});
      if (users.ok) update({users:users.data});
      if (!settings.ok || !users.ok) update({sectionError:settings.error || users.error});
      return settings.ok && users.ok;
    };
    const login = async (identity, password) => {
      if (!identity.trim() || !password) return {ok:false, error:'Enter your username or email and password.'};
      const result = await send('/auth/login', 'POST', {identity:identity.trim(), password});
      if (result.ok) { env.session.saveSession(result.data.token); env.navigate('/'); }
      return result;
    };
    const signup = async ({username, email, password, display_name}) => {
      const clean = {username:username.trim(), email:email.trim(), password, ...(display_name ? {display_name} : {})};
      if (!validUsername(clean.username)) return {ok:false, error:'Use at least two letters, numbers, underscores, or hyphens for the username.'};
      if (!validEmail(clean.email)) return {ok:false, error:'Enter a valid email address.'};
      if (password.length < 8) return {ok:false, error:'Password must be at least 8 characters.'};
      if (password.length > 1024) return {ok:false, error:'Password must be 1024 characters or fewer.'};
      if (state.instance && !state.instance.allow_signup && state.instance.has_users)
        return {ok:false, error:'Sign up is closed for this instance.'};
      const result = await send('/auth/signup', 'POST', clean);
      if (result.ok) { env.session.saveSession(result.data.token); env.navigate('/'); }
      return result;
    };
    const changeProfile = async patch => {
      const result = await send('/auth/me', 'PATCH', patch);
      if (result.ok) update({user:result.data});
      return result;
    };
    const changePassword = async (current_password, new_password) => {
      if (new_password.length < 8) return {ok:false, error:'New password must be at least 8 characters.'};
      const result = await send('/auth/me/password', 'POST', {current_password, new_password});
      if (result.ok) {
        env.session.saveSession(result.data.token);
        update({secret:null});
        await loadAccountData();
      }
      return result;
    };
    const createKey = name => runSensitive(async () => {
      const result = await send('/auth/keys', 'POST', {name:name.trim()});
      if (result.ok) {
        const value=result.data.key;
        update({secret:{kind:'API key',value,config:JSON.stringify({mcpServers:{lific:{url:`${globalThis.location.origin}/mcp`,headers:{Authorization:`Bearer ${value}`}}}},null,2)}});
        await loadAccountData();
      }
      return result;
    });
    const revokeKey = id => send(`/auth/keys/${encodeURIComponent(id)}`, 'DELETE');
    const connectBot = tool => runSensitive(async () => {
      const result = await send('/auth/bots', 'POST', {tool});
      if (result.ok) {
        const value=result.data.key;
        update({secret:{kind:'Connected tool key',value,config:JSON.stringify({mcpServers:{[tool]:{url:`${globalThis.location.origin}/mcp`,headers:{Authorization:`Bearer ${value}`}}}},null,2)}});
        await loadAccountData();
      }
      return result;
    });
    const botAction = (id, action) => action === 'delete'
      ? send(`/auth/bots/${encodeURIComponent(id)}`, 'DELETE')
      : send(`/auth/bots/${encodeURIComponent(id)}/${action}`, 'POST');
    const signOutAll = async () => {
      const result = await send('/auth/me/sessions', 'DELETE');
      if (result.ok) { env.session.clearSession(); env.navigate('/login'); }
      return result;
    };
    const logout = async () => { await env.session.logout(); env.navigate('/login'); };
    const saveSettings = async patch => {
      if (state.pendingAction?.kind === 'settings') {
        update({pendingPatch:{...state.pendingPatch, ...patch}, settings:{...state.settings, ...patch}});
        return {ok:false, pending:true};
      }
      const result = await send('/instance/settings', 'PATCH', patch);
      if (result.ok) update({settings:result.data, sectionError:''});
      else if (recentAuth(result)) {
        update({pendingAction:{kind:'settings'}, pendingPatch:{...state.pendingPatch, ...patch},
          settings:{...state.settings, ...patch}});
        return {...result,pending:true};
      } else update({sectionError:result.error, settings:state.settings});
      return result;
    };
    const runSensitive = async action => {
      const result = await action();
      if (recentAuth(result)) { update({pendingAction:{kind:'action', action}}); return {ok:false, pending:true}; }
      return result;
    };
    const adminUserAction = (id, action) => runSensitive(() => send(`/users/${encodeURIComponent(id)}/${action}`, 'POST'));
    const createUser = (username, password, email) => {
      if (!username.trim()) return Promise.resolve({ok:false,error:'Enter a username.'});
      if (password.length < 8 || password.length > 1024) return Promise.resolve({ok:false,error:'Password must be 8–1024 characters.'});
      if (email.trim() && !validEmail(email.trim())) return Promise.resolve({ok:false,error:'Enter a valid email address.'});
      return runSensitive(() => send('/users', 'POST', {username:username.trim(), password,
        ...(email.trim() ? {email:email.trim()} : {})}));
    };
    const reauthenticate = async password => {
      const pending = state.pendingAction;
      if (!pending) return {ok:false, error:'There is no pending action.'};
      const refreshed = await send('/auth/me/refresh', 'POST', password ? {password} : {});
      if (!refreshed.ok) return refreshed;
      env.session.saveSession(refreshed.data.token);
      const retry = pending.kind === 'settings'
        ? await send('/instance/settings', 'PATCH', state.pendingPatch)
        : await pending.action();
      if (retry.ok) {
        update({pendingAction:null, pendingPatch:{}, sectionError:''});
        if (pending.kind === 'settings') update({settings:retry.data});
        else await loadAdmin();
      } else {
        update({pendingAction:recentAuth(retry) ? pending : null,
          sectionError:recentAuth(retry) ? 'Sign in again, then retry this action.' : retry.error});
      }
      return retry;
    };
    const appearance = () => {
      const defaults={theme:'system',accent:'indigo',density:'comfortable',fontScale:'normal',motion:'system'};
      try {
        const storage=env.storage||globalThis.localStorage;
        const values={theme:storage.getItem('lific_theme'),accent:storage.getItem('lific_accent'),density:storage.getItem('lific_density'),
          fontScale:({sm:'small',md:'normal',lg:'large'})[storage.getItem('lific_font_scale')],motion:storage.getItem('lific_motion')};
        return Object.fromEntries(Object.entries(defaults).map(([key,value])=>[key,values[key]||value]));
      } catch { return defaults; }
    };
    const saveAppearance = (key, value) => {
      const prefs = {...appearance(), [key]:value};
      if (env.preferences?.savePreferences) env.preferences.savePreferences(prefs);
      else {
        const font=({small:'sm',normal:'md',large:'lg'})[value];
        try { (env.storage||globalThis.localStorage).setItem(`lific_${key==='fontScale'?'font_scale':key}`,font||value); } catch {}
      }
      return prefs;
    };
    return {state, loadInstance, loadAccount, loadAccountData, loadAdmin, login, signup, changeProfile,
      changePassword, createKey, revokeKey, connectBot, botAction, signOutAll, logout, saveSettings,
      adminUserAction, createUser, reauthenticate, appearance, saveAppearance};
  }

  function renderAuth(root, mode, app) {
    const s = app.state;
    const closed = mode === 'signup' && s.instance && !s.instance.allow_signup && s.instance.has_users;
    if (closed) {
      root.innerHTML = `<h1>Sign up is closed</h1><p>This instance is not accepting new accounts.</p><a href="/login">Log in</a>`;
      return;
    }
    const message = s.instance?.login_message ? `<p class="tc-identity__message">${escapeHtml(s.instance.login_message)}</p>` : '';
    const branding = s.instance?.instance_name ? `<p class="tc-identity__instance">${escapeHtml(s.instance.instance_name)}</p>` : '';
    const error = `<p data-form-error role="alert" class="tc-identity__error" hidden></p>`;
    const show = '<button type="button" class="tc-button tc-identity__show" data-show-password aria-label="Show password">Show</button>';
    if (mode === 'login') root.innerHTML = `${branding}<h1>Log in</h1>${message}${error}<form data-login novalidate>
      <label>Username or email<input name="identity" autocomplete="username" required></label>
      <label>Password<span class="tc-identity__password"><input name="password" type="password" autocomplete="current-password" required>${show}</span></label>
      <button class="tc-button tc-button--primary" type="submit">Log in</button></form>
      ${s.instance?.allow_signup || !s.instance?.has_users ? '<p>New here? <a href="/signup">Create an account</a></p>' : ''}`;
    else root.innerHTML = `${branding}<h1>${s.instance?.has_users ? 'Create your account' : 'Set up your instance'}</h1>${message}${error}<form data-signup novalidate>
      <label>Username<input name="username" autocomplete="username" pattern="[A-Za-z0-9_-]{2,}" required></label>
      <label>Email<input name="email" type="email" autocomplete="email" required></label>
      <label>Password<span class="tc-identity__password"><input name="password" type="password" autocomplete="new-password" minlength="8" maxlength="1024" required>${show}</span></label>
      <p class="tc-identity__hint">Use at least 8 characters.</p>
      <button class="tc-button tc-button--primary" type="submit">${s.instance?.has_users ? 'Create account' : 'Create administrator account'}</button></form>
      <p>Already have an account? <a href="/login">Log in</a></p>`;
    const form = root.querySelector('form');
    form?.addEventListener('submit', async event => {
      event.preventDefault();
      const data = new FormData(form), button = form.querySelector('[type=submit]');
      button.disabled = true;
      const result = mode === 'login'
        ? await app.login(data.get('identity'), data.get('password'))
        : await app.signup({username:data.get('username'), email:data.get('email'), password:data.get('password')});
      if (!result.ok) { const node = root.querySelector('[data-form-error]'); node.textContent = result.error; node.hidden = false; button.disabled = false; }
    });
    root.querySelector('[data-show-password]')?.addEventListener('click', event => {
      const input = root.querySelector('input[name=password]'); input.type = input.type === 'password' ? 'text' : 'password';
      event.currentTarget.textContent = input.type === 'password' ? 'Show' : 'Hide';
    });
  }

  function renderSettings(root, app) {
    const s = app.state, user = s.user;
    if (!user) { root.innerHTML = '<p role="alert">Sign in to manage your account.</p>'; return; }
    const prefs = app.appearance();
    root.innerHTML = `<header><p class="tc-identity__instance">${escapeHtml(user.username)}${user.is_admin ? ' · Administrator' : ''}</p><h1>Account settings</h1></header>
      ${s.sectionError ? `<p role="alert" class="tc-identity__error">${escapeHtml(s.sectionError)}</p>` : ''}
      <section><h2>Profile</h2><form data-profile><label>Display name<input name="display_name" value="${escapeHtml(user.display_name)}"></label>
      <label>Email<input name="email" type="email" value="${escapeHtml(user.email)}"></label><button class="tc-button" type="submit">Save profile</button><p data-profile-result role="status"></p></form></section>
      <section><h2>Appearance</h2><label>Theme<select data-pref="theme" data-tc-preference="theme">${['system','light','dark'].map(x=>`<option ${prefs.theme===x?'selected':''}>${x}</option>`).join('')}</select></label>
      <label>Accent<select data-pref="accent" data-tc-preference="accent">${['indigo','teal','rose','amber','green','violet'].map(x=>`<option ${prefs.accent===x?'selected':''}>${x}</option>`).join('')}</select></label>
      <label>Density<select data-pref="density" data-tc-preference="density">${['comfortable','compact'].map(x=>`<option ${prefs.density===x?'selected':''}>${x}</option>`).join('')}</select></label>
      <label>Text size<select data-pref="fontScale" data-tc-preference="fontScale">${['small','normal','large'].map(x=>`<option ${prefs.fontScale===x?'selected':''}>${x}</option>`).join('')}</select></label>
      <label>Motion<select data-pref="motion" data-tc-preference="motion">${['system','reduced','full'].map(x=>`<option ${prefs.motion===x?'selected':''}>${x}</option>`).join('')}</select></label></section>
      <section><h2>Password</h2><form data-password><label>Current password<input name="current" type="password" autocomplete="current-password" required></label>
      <label>New password<input name="next" type="password" autocomplete="new-password" minlength="8" required></label><button class="tc-button" type="submit">Change password</button><p data-password-result role="status"></p></form></section>
      <section><h2>API keys</h2><form data-key><label>Key name<input name="name" required maxlength="80"></label><button class="tc-button" type="submit">Create key</button></form>
      ${s.keys.map(key=>`<p>${escapeHtml(key.name)} · ${escapeHtml(key.created_at)} ${key.revoked?'· Revoked':`<button class="tc-button" data-revoke-key="${escapeHtml(key.id)}">Revoke</button>`}</p>`).join('')}</section>
      <section><h2>Connected tools</h2><form data-bot><label>Tool ID<input name="tool" placeholder="codex" pattern="[A-Za-z0-9_-]{1,48}" required></label><button class="tc-button" type="submit">Connect tool</button></form>
      ${s.bots.map(bot=>`<p>${escapeHtml(bot.display_name || bot.username)} · ${bot.connected?'Connected':'Disconnected'}
      ${bot.connected?`<button class="tc-button" data-bot-action="disconnect" data-id="${escapeHtml(bot.id)}">Disconnect</button>`:''}
      <button class="tc-button" data-bot-action="delete" data-id="${escapeHtml(bot.id)}">Delete</button></p>`).join('')}</section>
      <section><h2>Sessions</h2><p>Signing out everywhere revokes this session and connected API credentials.</p>
      <button class="tc-button" data-signout-all>Sign out everywhere</button><button class="tc-button" data-logout>Log out</button></section>
      ${user.is_admin ? '<p><a href="/settings/instance">Instance settings</a></p>' : ''}
      ${renderReauth(root,app)}<div data-secret class="tc-identity__secret" hidden></div>`;
    root.querySelector('[data-profile]')?.addEventListener('submit', async e => {
      e.preventDefault(); const form = e.currentTarget, d = new FormData(form), out = await app.changeProfile({display_name:d.get('display_name').trim(), email:d.get('email').trim()});
      root.querySelector('[data-profile-result]').textContent = out.ok ? 'Profile saved.' : out.error;
    });
    root.querySelector('[data-password]')?.addEventListener('submit', async e => {
      e.preventDefault(); const form=e.currentTarget,d=new FormData(form),out=await app.changePassword(d.get('current'),d.get('next'));
      root.querySelector('[data-password-result]').textContent=out.ok?'Password changed. Other sessions and connected tools were revoked.':out.error;
      if(out.ok) form.reset();
    });
    root.querySelector('[data-key]')?.addEventListener('submit', async e => {e.preventDefault(); const d=new FormData(e.currentTarget),out=await app.createKey(d.get('name')); if(!out.ok&&!out.pending) alert(out.error); else if(out.pending) renderSettings(root,app); else showSecret(root,app);});
    root.querySelector('[data-bot]')?.addEventListener('submit', async e => {e.preventDefault(); const d=new FormData(e.currentTarget),out=await app.connectBot(d.get('tool')); if(!out.ok&&!out.pending) alert(out.error); else if(out.pending) renderSettings(root,app); else showSecret(root,app);});
    root.querySelectorAll('[data-revoke-key]').forEach(button => button.addEventListener('click', async () => {
      if(confirm('Revoke this API key?')) {const out=await app.revokeKey(button.dataset.revokeKey); if(!out.ok) alert(out.error); await app.loadAccountData(); renderSettings(root,app);}
    }));
    root.querySelectorAll('[data-bot-action]').forEach(button => button.addEventListener('click', async () => {
      const action=button.dataset.botAction; if(!confirm(`${action==='delete'?'Delete':'Disconnect'} this connected tool?`)) return;
      const out=await app.botAction(button.dataset.id,action); if(!out.ok) alert(out.error); await app.loadAccountData(); renderSettings(root,app);
    }));
    root.querySelector('[data-signout-all]')?.addEventListener('click', async () => {if(confirm('Sign out on every device?')) {const out=await app.signOutAll(); if(!out.ok) alert(out.error);}});
    root.querySelector('[data-logout]')?.addEventListener('click', async () => {if(confirm('Log out?')) await app.logout();});
    root.querySelectorAll('[data-pref]').forEach(select => select.addEventListener('change', () => app.saveAppearance(select.dataset.pref, select.value)));
    root.querySelector('[data-reauth]')?.addEventListener('submit', async e=>{
      e.preventDefault(); const password=new FormData(e.currentTarget).get('password'), out=await app.reauthenticate(password);
      if(out.ok){await app.loadAccountData();renderSettings(root,app);}
      else {const error=root.querySelector('[data-reauth-error]');if(error)error.textContent=out.error;}
    });
    root.querySelector('[data-cancel-reauth]')?.addEventListener('click',()=>{app.state.pendingAction=null;app.state.pendingPatch={};renderSettings(root,app);});
  }

  function envNavigate(app, path) { app.logout ? app.logout() : location.assign(path); }
  function showSecret(root, app) {
    const secret = app.state.secret, node = root.querySelector('[data-secret]');
    if (!secret || !node) return;
    node.hidden = false;
    node.innerHTML = `<h3>${escapeHtml(secret.kind)} (shown once)</h3><code>${escapeHtml(secret.value)}</code><button class="tc-button" data-copy-secret>Copy key</button>
      ${secret.config?`<pre><code>${escapeHtml(secret.config)}</code></pre><button class="tc-button" data-copy-config>Copy client configuration</button>`:''}`;
    node.querySelector('[data-copy-secret]').addEventListener('click', async () => {
      await navigator.clipboard.writeText(secret.value); node.querySelector('[data-copy-secret]').textContent = 'Copied';
    });
    node.querySelector('[data-copy-config]')?.addEventListener('click', async () => {
      await navigator.clipboard.writeText(secret.config); node.querySelector('[data-copy-config]').textContent='Copied';
    });
  }

  function renderReauth(root, app) {
    const state = app.state;
    if (!state.pendingAction) return '';
    return `<section class="tc-identity__reauth"><h2>Confirm your identity</h2><p>This action needs a recent sign-in. Enter your password to continue.</p>
      <form data-reauth><label>Password<input name="password" type="password" autocomplete="current-password" required></label>
      <button class="tc-button" type="submit">Confirm and continue</button><button class="tc-button" type="button" data-cancel-reauth>Cancel</button><p role="alert" data-reauth-error></p></form></section>`;
  }

  function renderInstance(root, app) {
    const s=app.state;
    if (!s.user?.is_admin) {root.innerHTML='<h1>Instance settings</h1><p role="alert">Administrator access is required.</p>';return;}
    const x=s.settings;
    if (!x) {root.textContent=s.sectionError || 'Loading instance settings…';return;}
    root.innerHTML=`<header><p class="tc-identity__instance">${escapeHtml(x.instance_name)}</p><h1>Instance settings</h1></header>
      ${s.sectionError?`<p role="alert" class="tc-identity__error">${escapeHtml(s.sectionError)}</p>`:''}
      <section><h2>Identity and access</h2>
      <label>Instance name<input data-setting="instance_name" value="${escapeHtml(x.instance_name)}"></label>
      <label>Login message<textarea data-setting="login_message">${escapeHtml(x.login_message)}</textarea></label>
      <label>Email domains allowed for signup<input data-setting="signup_email_domains" value="${escapeHtml((x.signup_email_domains||[]).join(', '))}"></label>
      <label>Session lifetime in days<input data-setting="session_lifetime_days" type="number" min="1" value="${escapeHtml(x.session_lifetime_days)}"></label>
      ${[['allow_signup','Allow sign up'],['web_auto_login','Enable passwordless web login'],['authz_enforced','Enforce project roles']].map(([key,label])=>`<label class="tc-identity__check"><input data-setting="${key}" type="checkbox" ${x[key]?'checked':''}>${label}</label>`).join('')}</section>
      <section><h2>Accounts</h2><form data-create-user><label>Username<input name="username" required pattern="[A-Za-z0-9_-]{2,}"></label><label>Initial password<input name="password" type="password" minlength="8" maxlength="1024" required></label><label>Email (optional)<input name="email" type="email"></label><button class="tc-button" type="submit">Create account</button><p role="status" data-create-result></p></form>
      <div class="tc-identity__users">${s.users.map(user=>`<article><strong>${escapeHtml(user.display_name||user.username)}</strong><span>@${escapeHtml(user.username)} · ${user.is_admin?'Administrator':'Member'} · ${user.is_active?'Active':'Deactivated'}</span>
      ${user.id===s.user.id?'<span>Your account</span>':`${user.is_active?`<button class="tc-button" data-user-action="${user.is_admin?'demote':'promote'}" data-id="${escapeHtml(user.id)}">${user.is_admin?'Remove admin':'Make admin'}</button><button class="tc-button" data-user-action="deactivate" data-id="${escapeHtml(user.id)}">Deactivate</button>`:`<button class="tc-button" data-user-action="reactivate" data-id="${escapeHtml(user.id)}">Reactivate</button>`}`}</article>`).join('')}</div></section>
      ${renderReauth(root,app)}`;
    root.querySelectorAll('[data-setting]').forEach(control=>control.addEventListener(control.type==='checkbox'?'change':'change',()=>{
      const key=control.dataset.setting; let value=control.type==='checkbox'?control.checked:control.value;
      if(key==='session_lifetime_days') value=Number(value);
      if(key==='signup_email_domains') value=value.split(',').map(x=>x.trim()).filter(Boolean);
      void app.saveSettings({[key]:value}).then(out=>{if(!out.ok&&!out.pending) renderInstance(root,app); else if(app.state.pendingAction) renderInstance(root,app);});
    }));
    root.querySelector('[data-create-user]')?.addEventListener('submit',async e=>{
      e.preventDefault();const d=new FormData(e.currentTarget),out=await app.createUser(d.get('username'),d.get('password'),d.get('email'));
      root.querySelector('[data-create-result]').textContent=out.ok?'Account created.':out.pending?'Confirm your identity to create this account.':out.error;
      if(out.ok){e.currentTarget.reset();await app.loadAdmin();renderInstance(root,app);}
      else if(out.pending)renderInstance(root,app);
    });
    root.querySelectorAll('[data-user-action]').forEach(button=>button.addEventListener('click',async()=>{
      const action=button.dataset.userAction, verb=action==='promote'?'make this user an administrator':action==='demote'?'remove administrator access':action;
      if(!confirm(`Are you sure you want to ${verb}?`))return;
      const out=await app.adminUserAction(button.dataset.id,action);
      if(out.ok){await app.loadAdmin();renderInstance(root,app);}else if(out.pending)renderInstance(root,app);else {app.state.sectionError=out.error;renderInstance(root,app);}
    }));
    const reauth=root.querySelector('[data-reauth]');
    reauth?.addEventListener('submit',async e=>{e.preventDefault();const password=new FormData(reauth).get('password'),out=await app.reauthenticate(password);
      if(out.ok){await app.loadAdmin();renderInstance(root,app);}else {const error=root.querySelector('[data-reauth-error]');if(error)error.textContent=out.error;}
    });
    root.querySelector('[data-cancel-reauth]')?.addEventListener('click',()=>{app.state.pendingAction=null;app.state.pendingPatch={};void app.loadAdmin().then(()=>renderInstance(root,app));});
  }

  async function attach(root, env={}) {
    const mode=root?.dataset.topcoatIdentity, session=env.session||globalThis.lificSession;
    if(!root||!session||!['login','signup','settings','instance'].includes(mode))return null;
    const app=controller({session,navigate:path=>env.navigate?env.navigate(path):globalThis.location.assign(path),preferences:env.preferences||globalThis.LificTopcoatPreferences});
    const status=root.querySelector('[data-identity-status]'), content=root.querySelector('[data-identity-content]');
    root._app=app;
    root.setAttribute('aria-busy','true');
    try {
      if(mode==='login'||mode==='signup') {
        const hasInstance=await app.loadInstance();
        if(!session.state?.user) {
          try { if(globalThis.localStorage?.getItem('lific_token')) await session.refreshAccount(); } catch {}
        }
        if(session.state?.user) globalThis.location.assign('/');
        if(!hasInstance) throw new Error(app.state.error||'Could not load instance settings.');
        if(mode==='login'&&app.state.instance.web_auto_login) {
          const auto=await session.request('/auth/auto-login',{method:'POST'});
          if(auto.ok){session.saveSession(auto.data.token);globalThis.location.assign('/');return app;}
          app.state.error=auto.error;
        }
        renderAuth(content,mode,app);
        if(app.state.error) {const error=content.querySelector('[data-form-error]');if(error){error.textContent=app.state.error;error.hidden=false;}}
      } else {
        const signedIn=await app.loadAccount();
        if(!signedIn) {globalThis.location.assign('/login');return app;}
        if(mode==='settings') {await app.loadAccountData();renderSettings(content,app);}
        else {await app.loadAdmin();renderInstance(content,app);}
      }
      status.hidden=true;
    } catch(error) {status.textContent=error.message||'Could not load this page.';}
    root.setAttribute('aria-busy','false');
    return app;
  }

  const testApi = {controller, validUsername, validEmail, recentAuth, renderAuth, renderSettings, renderInstance, attach};
  globalThis.LificTopcoatIdentity = testApi;
  if (typeof document !== 'undefined') {
    const root=document.querySelector('[data-topcoat-identity]');
    if(root) void attach(root);
  }
})();
