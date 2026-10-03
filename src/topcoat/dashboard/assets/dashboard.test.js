const {test} = require('node:test');
const assert = require('node:assert/strict');
const dashboard = require('./dashboard.js');
const {homeModel} = dashboard;

const project = (id, identifier, name = identifier) => ({id, identifier, name, description: '', updated_at: '2026-10-01T00:00:00Z'});
const issue = (id, project_id, status = 'todo', priority = 'none', updated_at = '2026-10-01T00:00:00Z') =>
  ({id, project_id, identifier: `LIF-${id}`, title: `Issue ${id}`, status, priority, updated_at, created_at: updated_at});

test('home groups active/todo work, prioritizes active and urgent, and caps each group at six', () => {
  const projects = [project(1, 'LIF'), project(2, 'SEM')];
  const issues = [issue(1, 1, 'todo', 'urgent'), issue(2, 1, 'active', 'low'),
    ...Array.from({length: 6}, (_, index) => issue(index + 3, 1)), issue(20, 2)];
  const model = homeModel({projects, issues, pages: [], groups: [], activity: [], recents: []});
  assert.equal(model.issueGroups.length, 2);
  assert.equal(model.issueGroups[0].total, 8);
  assert.equal(model.issueGroups[0].visible.length, 6);
  assert.deepEqual(model.issueGroups[0].visible.slice(0, 2).map(row => row.id), [2, 1]);
});

test('home preserves catalog project order and canonical group membership independently of work ranking', () => {
  const projects = [project(3, 'THIRD'), project(1, 'FIRST'), project(2, 'SECOND')];
  const groups = [{id: 4, name: 'Work', sort_order: 1, project_ids: [2, 3]}, {id: 2, name: 'Personal', sort_order: 0, project_ids: [1]}];
  const model = homeModel({projects, issues: [issue(1, 3)], pages: [], groups, activity: [], recents: []});
  assert.deepEqual(model.projectGroups.map(group => group.name), ['Personal', 'Work']);
  assert.deepEqual(model.projectGroups[1].projects.map(row => row.id), [3, 2]);
  assert.deepEqual(model.projects.map(row => row.id), [3, 1, 2]);
  assert.deepEqual(projects.map(row => row.id), [3, 1, 2]);
});

test('pinned pages omit workspace and inaccessible project pages, sort newest first and cap at eight', () => {
  const pages = [
    {id: 50, project_id: null, pinned: true, updated_at: '2026-10-02'},
    {id: 51, project_id: 9, pinned: true, updated_at: '2026-10-02'},
    {id: 52, project_id: 1, pinned: false, updated_at: '2026-10-02'},
    ...Array.from({length: 10}, (_, id) => ({id, project_id: 1, pinned: true, updated_at: `2026-10-${String(id + 1).padStart(2, '0')}`})),
  ];
  const model = homeModel({projects: [project(1, 'LIF')], issues: [], pages, groups: [], activity: [], recents: []});
  assert.equal(model.pinnedPages.length, 8);
  assert.equal(model.pinnedPages[0].id, 9);
  assert.equal(model.pinnedPages[0].href, '/LIF/pages/9');
});

test('home recent links keep issue identifiers but numeric page/plan routes and hide revoked projects', () => {
  const recents = [{type: 'issue', project: 'LIF', routeId: 'LIF-2', title: 'Issue'},
    {type: 'page', project: 'LIF', routeId: '5', identifier: 'LIF-DOC-5', title: 'Page'},
    {type: 'plan', project: 'LIF', routeId: '6', title: 'Plan'},
    {type: 'page', project: 'GONE', routeId: '2', title: 'Hidden'},
    {type: 'issue', project: 'LIF', routeId: '//evil.test', title: 'Invalid'}];
  const model = homeModel({projects: [project(1, 'LIF')], issues: [], pages: [], groups: [], activity: [], recents});
  assert.deepEqual(model.recents.map(row => row.href), ['/LIF/issues/LIF-2', '/LIF/pages/5', '/LIF/plans/6']);
});

test('activity digest chooses up to three projects by work recency then project recency when quiet', () => {
  const projects = [project(1, 'LIF'), {...project(2, 'SEM'), updated_at: '2026-10-03'}, project(3, 'A'), project(4, 'B')];
  assert.deepEqual(dashboard.topProjectIds([issue(1, 1, 'active', 'low', '2026-10-04'), issue(2, 3)], projects), [1, 3]);
  assert.deepEqual(dashboard.topProjectIds([], projects), [2, 1, 3]);
});

test('home quick action follows the most recently active project and requires an editing role', () => {
  const input = {projects: [project(1, 'LIF'), project(2, 'SEM')], issues: [issue(1, 2)], pages: [], groups: [], activity: [], recents: [],
    quickProjectId: 2, role: {role: 'maintainer', enforced: true, is_admin: false}};
  assert.equal(homeModel(input).newIssueHref, '/SEM/issues/new');
  assert.equal(homeModel({...input, role: {role: 'viewer', enforced: true, is_admin: false}}).newIssueHref, null);
  assert.equal(homeModel({...input, role: {role: null, enforced: false, is_admin: false}}).newIssueHref, '/SEM/issues/new');
  assert.equal(homeModel({...input, projects: [], quickProjectId: null}).newIssueHref, null);
});

test('overview ranks open work by priority, age and staleness, without terminal issues, and derives completion from counts', () => {
  const input = {project: project(1, 'LIF'), issues: [issue(1, 1, 'done', 'urgent'), issue(2, 1, 'todo', 'high'),
    issue(3, 1, 'active', 'urgent'), issue(4, 1, 'backlog', 'none', '2024-01-01T00:00:00Z')],
    counts: {done: 4, total: 10}, activity: [], role: {role: 'viewer', enforced: true, is_admin: false}};
  const model = dashboard.overviewModel(input, Date.parse('2026-10-01T00:00:00Z'));
  assert.deepEqual(model.attention.map(row => row.id), [4, 3, 2]);
  assert.equal(model.completion, 40);
  assert.equal(model.newIssueHref, null);
  assert.equal(model.settingsHref, null);
  assert.equal(dashboard.overviewModel({...input, role: {role: 'lead', enforced: true, is_admin: false}}).settingsHref, '/LIF/settings');
  assert.equal(dashboard.overviewModel({...input, counts: {done: 0, total: 0}}).completion, 0);
});

test('activity links navigate issues, pages and their comments while unrouteable entities stay text', () => {
  const projects = [project(1, 'LIF')];
  assert.equal(dashboard.activityHref({project_id: 1, entity_type: 'issue', entity_label: 'LIF-1'}, projects), '/LIF/issues/LIF-1');
  assert.equal(dashboard.activityHref({project_id: 1, entity_type: 'page', entity_id: 9}, projects), '/LIF/pages/9');
  assert.equal(dashboard.activityHref({project_id: 1, entity_type: 'comment', page_id: 7}, projects), '/LIF/pages/7');
  assert.equal(dashboard.activityHref({project_id: 1, entity_type: 'comment', issue_id: 7, entity_label: 'LIF-4'}, projects), '/LIF/issues/LIF-4');
  assert.equal(dashboard.activityHref({project_id: 1, entity_type: 'project'}, projects), null);
  assert.equal(dashboard.activityHref({project_id: 2, entity_type: 'page', entity_id: 9}, projects), null);
});

test('activity counter selects the shortest populated window, expires its baseline and resets on account change', () => {
  const counter = dashboard.createActivityCounter();
  counter.seed(12, 0);
  assert.deepEqual(counter.rate(0), {value: 12, unit: 'updates/day'});
  counter.record(2000); counter.record(2100);
  assert.deepEqual(counter.rate(2100), {value: 2, unit: 'updates/s'});
  assert.deepEqual(counter.rate(6100), {value: 2, unit: 'updates/min'});
  assert.deepEqual(counter.rate(72000), {value: 2, unit: 'updates/hr'});
  assert.deepEqual(counter.rate(86400001), {value: 2, unit: 'updates/day'});
  counter.reset();
  assert.equal(counter.rate(86400001).value, 0);
});

const ok = data => ({ok: true, data});
const denied = (status, error = 'Access denied') => ({ok: false, status, error});
const deferred = () => { let resolve; const promise = new Promise(done => {resolve = done;}); return {promise, resolve}; };
const replies = {
  '/projects': [project(1, 'LIF'), project(2, 'SEM')], '/project-groups': [],
  '/issues?status=active&limit=200': [issue(1, 1, 'active')], '/issues?status=todo&limit=200': [issue(2, 2)], '/pages': [],
  '/projects/1/activity?limit=8&offset=0': {items: [{id: 1, project_id: 1, entity_type: 'issue', entity_label: 'LIF-1', ts: '2026-10-02'}]},
  '/projects/2/activity?limit=8&offset=0': {items: []}, '/projects/1/my-role': {role: 'maintainer', enforced: true, is_admin: false},
  '/projects/1/issue-counts': {done: 5, total: 10}, '/issues?project_id=1&limit=1000': [issue(1, 1)],
  '/projects/1/activity?limit=14&offset=0': {items: [{id: 2, project_id: 1, ts: '2026-10-02'}]},
};

function controllerEnv(overrides = {}) {
  const paths = [];
  const updates = [];
  const env = {identity: () => 'private:1', user: () => ({id: 1, username: 'reader'}), recents: () => [],
    request: async path => {paths.push(path); assert.ok(path in replies, `Unexpected request ${path}`); return ok(replies[path]);},
    onChange: state => updates.push(state), now: () => Date.parse('2026-10-02T00:00:00Z'),
    delay: setTimeout, cancel: clearTimeout, ...overrides};
  return {env, paths, updates};
}

test('home loads private existing endpoints and exposes active work, quick creation and merged activity', async () => {
  const {env, paths, updates} = controllerEnv();
  const controller = new dashboard.DashboardController(env);
  await controller.load();
  assert.equal(updates[0].status, 'loading');
  assert.equal(controller.state.status, 'ready');
  assert.equal(controller.state.model.issueTotal, 2);
  assert.equal(controller.state.model.activity[0].href, '/LIF/issues/LIF-1');
  assert.equal(controller.state.model.newIssueHref, '/LIF/issues/new');
  assert.ok(paths.includes('/project-groups'));
  assert.ok(paths.includes('/issues?status=active&limit=200'));
  assert.ok(paths.includes('/issues?status=todo&limit=200'));
  controller.dispose();
});

test('empty instance renders ready home without project metrics/activity or creation requests', async () => {
  const {env, paths} = controllerEnv({request: async path => {paths.push(path); return ok([]);}});
  const controller = new dashboard.DashboardController(env);
  await controller.load();
  assert.equal(controller.state.status, 'ready');
  assert.equal(controller.state.model.issueTotal, 0);
  assert.equal(controller.state.model.newIssueHref, null);
  assert.ok(!paths.some(path => path.includes('/activity') || path.includes('/my-role')));
});

test('project overview deep link resolves identifier case independently of previously selected project', async () => {
  const {env, paths} = controllerEnv();
  const controller = new dashboard.DashboardController(env, 'lif');
  await controller.load();
  assert.equal(controller.state.model.project.identifier, 'LIF');
  assert.equal(controller.state.model.completion, 50);
  assert.equal(controller.state.model.newIssueHref, '/LIF/issues/new');
  assert.equal(controller.state.model.settingsHref, null);
  assert.ok(paths.includes('/projects/1/issue-counts'));
  assert.ok(!paths.includes('/pages'));
});

test('unknown project and failed project fetch provide a retryable error without stale dashboard data', async () => {
  const {env} = controllerEnv();
  const missing = new dashboard.DashboardController(env, 'GONE');
  await missing.load();
  assert.equal(missing.state.status, 'error');
  assert.match(missing.state.error, /GONE.*not found/);
  assert.equal(missing.state.model, null);
  const failing = new dashboard.DashboardController({...env, request: async () => denied(500, 'Server unavailable')});
  await failing.load();
  assert.equal(failing.state.status, 'error');
  assert.equal(failing.state.error, 'Server unavailable');
});

test('partial home failure names the failed section while preserving other loaded information', async () => {
  const {env} = controllerEnv({request: async path => path === '/pages' ? denied(500, 'Pages offline') : ok(replies[path])});
  const controller = new dashboard.DashboardController(env);
  await controller.load();
  assert.equal(controller.state.status, 'ready');
  assert.equal(controller.state.model.issueTotal, 2);
  assert.equal(controller.state.sectionErrors.pages, 'Pages offline');
  assert.deepEqual(controller.state.model.pinnedPages, []);
});

test('revoked overview access clears metrics, activity, issues and actions after a previously successful load', async () => {
  let revoked = false;
  const {env} = controllerEnv({request: async path => revoked && path === '/projects/1/issue-counts' ? denied(403) : ok(replies[path])});
  const controller = new dashboard.DashboardController(env, 'LIF');
  await controller.load();
  revoked = true;
  await controller.load(false);
  assert.equal(controller.state.status, 'error');
  assert.equal(controller.state.model, null);
});

test('late results from an earlier account never restore its private work', async () => {
  const pending = deferred();
  let audience = 'private:1';
  const {env} = controllerEnv({identity: () => audience, request: async path => path === '/projects' ? pending.promise : ok(replies[path])});
  const controller = new dashboard.DashboardController(env);
  const load = controller.load();
  audience = null;
  controller.accountChanged();
  pending.resolve(ok(replies['/projects']));
  await load;
  assert.equal(controller.state.status, 'idle');
  assert.equal(controller.state.model, null);
});

test('public and anonymous scopes make no private dashboard requests', async () => {
  const {env, paths} = controllerEnv({identity: () => null});
  const controller = new dashboard.DashboardController(env);
  await controller.load();
  assert.deepEqual(paths, []);
  assert.equal(controller.state.model, null);
});

test('realtime bursts debounce overview metrics/activity refresh and unrelated projects do not refresh', async () => {
  let now = 0;
  const scheduled = new Map();
  let nextTimer = 0;
  const {env, paths} = controllerEnv({now: () => now, delay: (callback, ms) => {scheduled.set(++nextTimer, {callback, ms}); return nextTimer;}, cancel: id => scheduled.delete(id)});
  const controller = new dashboard.DashboardController(env, 'LIF');
  await controller.load();
  const before = paths.length;
  controller.handleEvent({type: 'issue.updated', project_id: 2});
  assert.equal(scheduled.size, 0);
  controller.handleEvent({type: 'issue.updated', project_id: 1});
  now = 100;
  controller.handleEvent({type: 'page.updated', project_id: 1});
  assert.equal(scheduled.size, 1);
  assert.equal([...scheduled.values()][0].ms, 750);
  const timer = [...scheduled.values()][0]; scheduled.clear();
  await timer.callback();
  assert.equal(paths.length - before, 5);
  assert.equal(controller.state.model.completion, 50);
});

test('continuous realtime updates have a maximum wait and activity baseline survives ordinary refreshes', async () => {
  let now = 0;
  const scheduled = new Map();
  let nextTimer = 0;
  const {env} = controllerEnv({now: () => now, delay: (callback, ms) => {scheduled.set(++nextTimer, {callback, ms}); return nextTimer;}, cancel: id => scheduled.delete(id)});
  const controller = new dashboard.DashboardController(env);
  await controller.load();
  controller.handleEvent({type: 'activity.baseline', day_count: 9});
  controller.handleEvent({type: 'issue.updated', project_id: 1});
  now = 4800;
  controller.handleEvent({type: 'issue.updated', project_id: 1});
  assert.equal([...scheduled.values()][0].ms, 200);
  assert.equal(controller.activityRate().value, 2);
  await controller.load(false);
  assert.equal(controller.activityRate().value, 2);
  controller.accountChanged();
  assert.equal(controller.activityRate().value, 0);
});

test('disposing a dashboard aborts its pending request and stops realtime timers', async () => {
  const pending = deferred();
  let signal;
  const {env} = controllerEnv({request: async (_path, options) => {signal = options.signal; return pending.promise;}});
  const controller = new dashboard.DashboardController(env);
  const load = controller.load();
  controller.dispose();
  assert.equal(signal.aborted, true);
  pending.resolve(ok([]));
  await load;
  assert.equal(controller.state.model, null);
});

test('overview attention explains the age and idle cues used by its importance ranking', () => {
  const model = dashboard.overviewModel({project: project(1, 'LIF'), issues: [issue(1, 1, 'todo', 'high', '2026-09-01T00:00:00Z')],
    counts: null, activity: [], role: null}, Date.parse('2026-10-01T00:00:00Z'));
  assert.equal(model.attention[0].ageLabel, '30d');
  assert.equal(model.attention[0].idleLabel, '30d');
});

test('temporary role lookup failure preserves legacy editing affordances while recording the error', async () => {
  const {env} = controllerEnv({request: async path => path.endsWith('/my-role') ? denied(500, 'Role temporarily unavailable') : ok(replies[path])});
  const controller = new dashboard.DashboardController(env, 'LIF');
  await controller.load();
  assert.equal(controller.state.status, 'ready');
  assert.equal(controller.state.model.newIssueHref, '/LIF/issues/new');
  assert.equal(controller.state.sectionErrors.permissions, 'Role temporarily unavailable');
});

test('definitive project role denial clears an overview and hides home quick creation', async () => {
  const {env} = controllerEnv({request: async path => path.endsWith('/my-role') ? denied(403) : ok(replies[path])});
  const overview = new dashboard.DashboardController(env, 'LIF');
  await overview.load();
  assert.equal(overview.state.status, 'error');
  assert.equal(overview.state.model, null);
  const home = new dashboard.DashboardController(env);
  await home.load();
  assert.equal(home.state.model.newIssueHref, null);
});

test('unavailable or malformed local storage does not prevent home from loading', () => {
  assert.deepEqual(dashboard.readRecents({get localStorage() {throw Error('Storage blocked');}}), []);
  assert.deepEqual(dashboard.readRecents({localStorage: {getItem: () => '{broken'}}), []);
  assert.deepEqual(dashboard.readRecents({localStorage: {getItem: () => '{"invalid":"object"}'}}), []);
});

test('resync immediately clears rendered data and refetches without waiting for the activity baseline', async () => {
  const pending = deferred();
  let refreshing = false;
  const {env} = controllerEnv({request: async path => refreshing && path === '/projects' ? pending.promise : ok(replies[path])});
  const controller = new dashboard.DashboardController(env, 'LIF');
  await controller.load();
  refreshing = true;
  controller.handleEvent({type: 'resync.required'});
  assert.equal(controller.state.status, 'loading');
  assert.equal(controller.state.model, null);
  pending.resolve(ok(replies['/projects']));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(controller.state.status, 'ready');
  controller.dispose();
});
