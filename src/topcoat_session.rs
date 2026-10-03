//! Request-scoped authentication and browser session bridge for Topcoat.
//!
//! Load [`SCRIPT_PATH`] with a deferred script in the shared document and
//! spread [`bootstrap_attributes`] onto its body. Browser requests read the
//! existing `lific_token` at call time; no token is kept in Topcoat session
//! state. [`Scope::resolve`] is also available to Rust screen handlers using
//! the typed API client. Capabilities control display only; REST remains the
//! authorization boundary.

use reqwest::{Method, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use topcoat::{
    context::Cx,
    router::{response::Response, route},
    view::{Attributes, attributes},
};

use super::topcoat_api::{ApiBaseUrl, ApiClient};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-session.js";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    Private,
    Public(String),
}

impl Scope {
    pub(crate) fn public(project: &str) -> Self {
        Self::Public(project.to_uppercase())
    }

    /// Accept the path after `/api`, matching the existing Svelte boundary.
    /// Public writes and unsupported reads never fall through to private API.
    pub(crate) fn resolve(&self, method: &Method, path: &str) -> ResolvedRequest {
        if !path.starts_with('/') || path.starts_with("//") {
            return ResolvedRequest::Refused;
        }
        let Self::Public(project) = self else {
            return ResolvedRequest::Private {
                path: format!("/api{path}"),
            };
        };
        if *method != Method::GET {
            return ResolvedRequest::Refused;
        }
        let (pathname, search) = path.split_once('?').unwrap_or((path, ""));
        let segments: Vec<_> = pathname.trim_start_matches('/').split('/').collect();
        let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let project_route = matches!(segments.as_slice(), ["projects", id, _] if numeric(id));
        let synthetic = if pathname == "/auth/me" {
            Some((StatusCode::UNAUTHORIZED, json!({"error":"not signed in"})))
        } else if project_route && segments[2] == "my-role" {
            Some((
                StatusCode::OK,
                json!({"role":null,"enforced":true,"is_admin":false}),
            ))
        } else if matches!(segments.as_slice(), ["issues" | "pages", id, "activity"] if numeric(id))
        {
            Some((StatusCode::OK, json!({"items":[],"has_more":false})))
        } else if project_route && matches!(segments[2], "mention-candidates" | "views") {
            Some((StatusCode::OK, json!([])))
        } else {
            None
        };
        if let Some((status, body)) = synthetic {
            return ResolvedRequest::Synthetic { status, body };
        }
        let base = format!("/public/api/projects/{}", urlencoding::encode(project));
        let query: Vec<(String, String)> = serde_urlencoded::from_str(search).unwrap_or_default();
        let has_query = |name: &str| query.iter().any(|(key, _)| key == name);
        let suffix = if pathname == "/projects" {
            Some(String::new())
        } else if project_route && matches!(segments[2], "index" | "changes") {
            Some(format!("/{}{}", segments[2], query_suffix(search)))
        } else if (matches!(pathname, "/modules" | "/labels" | "/folders")
            && has_query("project_id"))
            || matches!(segments.as_slice(), ["issues", "resolve", identifier] if !identifier.is_empty() && identifier.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')))
        {
            Some(pathname.to_owned())
        } else if matches!(segments.as_slice(), ["issues" | "pages", id] | ["issues" | "pages", id, "comments"] if numeric(id))
        {
            Some(format!("{pathname}{}", query_suffix(search)))
        } else if pathname == "/attachments" && has_query("entity_type") && has_query("entity_id") {
            Some(format!("{pathname}?{search}"))
        } else if matches!(segments.as_slice(), ["attachments", id] | ["attachments", id, "thumbnail" | "preview"] if numeric(id))
        {
            Some(pathname.to_owned())
        } else {
            None
        };
        match suffix {
            Some(suffix) => ResolvedRequest::Public {
                path: format!("{base}{suffix}"),
                wrap_project: path == "/projects",
            },
            None => ResolvedRequest::Refused,
        }
    }
}

fn query_suffix(search: &str) -> String {
    if search.is_empty() {
        String::new()
    } else {
        format!("?{search}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ResolvedRequest {
    Private { path: String },
    Public { path: String, wrap_project: bool },
    Synthetic { status: StatusCode, body: Value },
    Refused,
}

impl ResolvedRequest {
    /// Explicit injection for private calls, explicit omission for public
    /// calls. Fresh reqwest clients have no default headers or cookie store.
    pub(crate) fn request(
        &self,
        base_url: &ApiBaseUrl,
        bearer: Option<&str>,
        method: Method,
    ) -> Option<RequestBuilder> {
        match self {
            Self::Private { path } => {
                Some(ApiClient::new(base_url.clone(), bearer).request(method, path))
            }
            Self::Public { path, .. } if method == Method::GET => {
                Some(ApiClient::new(base_url.clone(), None).request(method, path))
            }
            Self::Public { .. } | Self::Synthetic { .. } | Self::Refused => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ProjectRole {
    Viewer,
    Maintainer,
    Lead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct RoleInputs {
    pub(crate) role: Option<ProjectRole>,
    pub(crate) enforced: bool,
    pub(crate) is_admin: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Affordances {
    pub(crate) edit: bool,
    pub(crate) manage: bool,
    pub(crate) comment: bool,
    pub(crate) publish: bool,
    pub(crate) admin: bool,
}

impl RoleInputs {
    pub(crate) fn affordances(self, scope: &Scope, is_project_lead: bool) -> Affordances {
        if matches!(scope, Scope::Public(_)) {
            return Affordances::default();
        }
        let unrestricted = !self.enforced || self.is_admin;
        Affordances {
            edit: unrestricted
                || matches!(self.role, Some(ProjectRole::Maintainer | ProjectRole::Lead)),
            manage: unrestricted || self.role == Some(ProjectRole::Lead),
            comment: unrestricted || self.role.is_some(),
            publish: self.is_admin || self.role == Some(ProjectRole::Lead) || is_project_lead,
            admin: self.is_admin,
        }
    }
}

/// Static data attributes are supported by Topcoat 0.9. Browser state is
/// initialized after load, so tokens never enter server-generated markup.
pub(crate) fn bootstrap_attributes(cx: &Cx, scope: &Scope, require_session: bool) -> Attributes {
    let project = match scope {
        Scope::Private => None,
        Scope::Public(project) => Some(project.as_str()),
    };
    attributes! { cx =>
        data-lific-public-project=(project)
        data-lific-require-session=(if require_session { "true" } else { "false" })
        data-lific-session-state="loading"
    }
}

#[route(GET "/__topcoat-session.js")]
async fn browser_script() -> topcoat::Result<Response> {
    Ok(Response::builder()
        .header("content-type", "text/javascript; charset=utf-8")
        .header("cache-control", "no-cache")
        .body(topcoat::router::Body::from(BROWSER_SCRIPT))?)
}

/// This deliberately uses ordinary browser REST requests rather than a
/// Topcoat procedure: procedures cannot forward per-call bearer headers.
pub(crate) const BROWSER_SCRIPT: &str = r#"
(() => {
    'use strict';
    const TOKEN = 'lific_token';
    const emptyRole = () => ({role:null, enforced:false, is_admin:false});
    const state = {user:null, publicProject:null, role:emptyRole(), projectId:null, loading:true, error:null};
    const roles = new Map();
    let generation = 0;
    let roleGeneration = 0;
    const stored = () => localStorage.getItem(TOKEN);
    const audience = () => `${generation}:${state.publicProject ?? ''}:${stored() ?? ''}`;
    const refused = () => ({ok:false, status:403, error:"This isn't available in the public view."});
    const audienceChanged = () => ({ok:false, status:null, code:'audience_changed', error:'The view changed. Please try again.'});

    function resolve(path, method='GET') {
        if (!path.startsWith('/') || path.startsWith('//')) return {kind:'refused'};
        if (state.publicProject === null) return {kind:'private',url:`/api${path}`};
        if (method !== 'GET') return {kind:'refused'};
        const [pathname,search=''] = path.split('?');
        const query = new URLSearchParams(search);
        if (pathname === '/auth/me') return {kind:'synthetic',status:401,body:{error:'not signed in'}};
        if (/^\/projects\/\d+\/my-role$/.test(pathname)) return {kind:'synthetic',status:200,body:{role:null,enforced:true,is_admin:false}};
        if (/^\/(issues|pages)\/\d+\/activity$/.test(pathname)) return {kind:'synthetic',status:200,body:{items:[],has_more:false}};
        if (/^\/projects\/\d+\/(mention-candidates|views)$/.test(pathname)) return {kind:'synthetic',status:200,body:[]};
        const base = `/public/api/projects/${encodeURIComponent(state.publicProject)}`;
        const suffix = search ? `?${search}` : '';
        let match;
        if (pathname === '/projects') return {kind:'public',url:base,wrapProject:path === '/projects'};
        if ((match = pathname.match(/^\/projects\/\d+\/(index|changes)$/))) return {kind:'public',url:`${base}/${match[1]}${suffix}`};
        if (/^\/(modules|labels|folders)$/.test(pathname) && query.has('project_id')) return {kind:'public',url:`${base}${pathname}`};
        if (/^\/issues\/resolve\/[A-Za-z0-9_-]+$/.test(pathname)) return {kind:'public',url:`${base}${pathname}`};
        if (/^\/(issues|pages)\/\d+(\/comments)?$/.test(pathname)) return {kind:'public',url:`${base}${pathname}${suffix}`};
        if (pathname === '/attachments' && query.has('entity_type') && query.has('entity_id')) return {kind:'public',url:`${base}/attachments?${search}`};
        if (/^\/attachments\/\d+(\/thumbnail|\/preview)?$/.test(pathname)) return {kind:'public',url:`${base}${pathname}`};
        return {kind:'refused'};
    }

    function affordances(isLead=false) {
        if (state.publicProject !== null) return {edit:false,manage:false,comment:false,publish:false,admin:false};
        const {role,enforced,is_admin} = state.role;
        const unrestricted = !enforced || is_admin;
        return {
            edit:unrestricted || role === 'maintainer' || role === 'lead',
            manage:unrestricted || role === 'lead',
            comment:unrestricted || ['viewer','maintainer','lead'].includes(role),
            publish:is_admin || role === 'lead' || isLead,
            admin:is_admin || state.user?.is_admin === true,
        };
    }

    function scopedRoute(route) {
        if (state.publicProject === null || route.startsWith('/public/') || route === '/login' || route === '/signup') return route;
        const normalized = route.startsWith('/') ? route : `/${route}`;
        const overview = normalized.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/(overview|settings)$/);
        return overview ? `/public/${overview[1]}/issues` : `/public${normalized}`;
    }

    function notify() {
        const body = document.body;
        if (body) body.dataset.lificSessionState = state.loading ? 'loading' : state.publicProject !== null ? 'public' : state.user ? 'authenticated' : 'anonymous';
        for (const element of document.querySelectorAll('[data-lific-capability]')) {
            const visible = affordances(element.dataset.lificProjectLead === 'true')[element.dataset.lificCapability] === true;
            element.hidden = !visible;
        }
        for (const element of document.querySelectorAll('[data-lific-account-name]')) element.textContent = state.user?.display_name || state.user?.username || '';
        window.dispatchEvent(new CustomEvent('lific:account-change', {detail:{...state,role:{...state.role}}}));
    }

    function clearVisible() {
        generation++;
        roleGeneration++;
        state.user = null;
        state.role = emptyRole();
        state.projectId = null;
        state.error = null;
        roles.clear();
        notify();
    }

    function clearSession() {
        localStorage.removeItem(TOKEN);
        clearVisible();
        window.dispatchEvent(new CustomEvent('lific:session-change'));
    }

    function saveSession(token) {
        localStorage.setItem(TOKEN, token);
        clearVisible();
        window.dispatchEvent(new CustomEvent('lific:session-change'));
    }

    function redirectAnonymous() {
        const route = location.hash.startsWith('#/') ? location.hash.slice(1) : location.pathname;
        if (state.publicProject === null && !stored() && document.body?.dataset.lificRequireSession !== 'false' && route !== '/login' && route !== '/signup') {
            location.replace('/login');
        }
    }

    async function request(path, options={}) {
        const method = (options.method || 'GET').toUpperCase();
        const resolved = resolve(path,method);
        if (resolved.kind === 'refused') return refused();
        if (resolved.kind === 'synthetic') return resolved.status >= 400
            ? {ok:false,status:resolved.status,error:resolved.body.error}
            : {ok:true,status:resolved.status,data:resolved.body,headers:new Headers()};
        const session = audience();
        let responseAudience = session;
        const token = stored();
        const headers = new Headers(options.headers);
        // Callers cannot override the audience's credentials, including a
        // credential accidentally carried from a private component.
        headers.delete('Authorization');
        headers.delete('Cookie');
        if (resolved.kind === 'private' && token) headers.set('Authorization',`Bearer ${token}`);
        if (options.body && !(options.body instanceof FormData) && !headers.has('Content-Type')) headers.set('Content-Type','application/json');
        try {
            const response = await fetch(resolved.url, {...options,method,headers,credentials:resolved.kind === 'public' ? 'omit' : 'same-origin'});
            if (responseAudience !== audience()) return audienceChanged();
            if (response.status === 401 && resolved.kind === 'private') {
                clearSession();
                redirectAnonymous();
                // This request's own expiry handling changes the session.
                // Further transitions while its body loads still discard it.
                responseAudience = audience();
            }
            let body;
            try { body = await response.json(); }
            catch {
                if (responseAudience !== audience()) return audienceChanged();
                return {ok:false,status:response.status,error:`Invalid API response (HTTP ${response.status})`,headers:response.headers};
            }
            if (responseAudience !== audience()) return audienceChanged();
            if (!response.ok) return {ok:false,status:response.status,error:body?.error || `HTTP ${response.status}`,code:body?.code,current:body?.current,headers:response.headers};
            if (resolved.wrapProject) body = [body];
            return {ok:true,status:response.status,data:body,headers:response.headers};
        } catch (error) {
            if (responseAudience !== audience()) return audienceChanged();
            return {ok:false,status:null,error:"Couldn't reach the server. Check your connection and try again."};
        }
    }

    async function refreshAccount() {
        if (state.publicProject !== null) return {ok:false,status:401,error:'not signed in'};
        const session = audience();
        if (!stored()) {
            state.user = null;
            notify();
            redirectAnonymous();
            return {ok:false,status:401,error:'not signed in'};
        }
        const result = await request('/auth/me');
        if (session !== audience()) return result;
        if (result.ok) { state.user = result.data; state.error = null; }
        else { state.error = result.error; }
        notify();
        return result;
    }

    async function logout() {
        if (state.publicProject !== null) return refused();
        const session = audience();
        const result = await request('/auth/logout', {method:'POST'});
        if (session === audience()) { clearSession(); redirectAnonymous(); }
        return result;
    }

    function setPublicProject(project) {
        const next = project === null ? null : project.toUpperCase();
        if (next === state.publicProject) return;
        state.publicProject = next;
        clearVisible();
        state.loading = false;
        notify();
        window.dispatchEvent(new CustomEvent('lific:scope-change', {detail:next}));
    }

    async function loadRole(projectId, force=false) {
        const session = audience();
        const load = ++roleGeneration;
        state.projectId = projectId;
        if (state.publicProject !== null) {
            state.role = {role:null,enforced:true,is_admin:false};
            notify();
            return;
        }
        const cacheKey = `${stored() ?? ''}:${projectId}`;
        if (!force && roles.has(cacheKey)) { state.role=roles.get(cacheKey); notify(); return; }
        state.role=emptyRole();
        notify();
        const result = await request(`/projects/${projectId}/my-role`);
        if (session !== audience() || load !== roleGeneration) return;
        if (result.ok) { state.role=result.data; roles.set(cacheKey,result.data); }
        // Transient role failures preserve the existing display fail-open
        // behavior; actual mutation authorization always stays server-side.
        notify();
        return result;
    }

    async function bootstrap() {
        state.loading = true;
        notify();
        if (state.publicProject === null) await refreshAccount();
        const projectId = document.body?.dataset.lificProjectId;
        if (projectId) await loadRole(Number(projectId));
        state.loading = false;
        notify();
    }

    window.lificSession = {state,resolve,request,bootstrap,refreshAccount,logout,saveSession,clearSession,setPublicProject,loadRole,affordances,scopedRoute};
    window.addEventListener('storage', event => {
        if (event.key === TOKEN || event.key === null) { clearVisible(); void bootstrap(); }
    });
    window.addEventListener('lific:account-refresh', () => { void refreshAccount(); });
    function routeScope() {
        const route = location.hash.startsWith('#/') ? location.hash.slice(1) : location.pathname;
        const match = route.match(/^\/public\/([^/]+)/);
        return match ? decodeURIComponent(match[1]) : null;
    }
    function routeChanged() {
        setPublicProject(routeScope());
        void bootstrap();
    }
    window.addEventListener('hashchange',routeChanged);
    window.addEventListener('popstate',routeChanged);
    function start() {
        const project = document.body?.dataset.lificPublicProject || routeScope();
        setPublicProject(project);
        document.addEventListener('click', event => {
            if (event.target.closest('[data-lific-logout]')) { event.preventDefault(); void logout(); }
        });
        void bootstrap();
    }
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded',start,{once:true});
    else start();
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::{Method, StatusCode, header::AUTHORIZATION};
    use topcoat::{
        context::Cx,
        view::{ViewExt, view},
    };

    fn local_api_base() -> ApiBaseUrl {
        ApiBaseUrl::parse("http://127.0.0.1").unwrap()
    }

    #[test]
    fn public_scope_rewrites_supported_reads_and_omits_bearer_credentials() {
        let scope = Scope::public("lif");
        let base_url = local_api_base();
        for (path, expected) in [
            ("/projects", "/public/api/projects/LIF"),
            ("/projects/42/index", "/public/api/projects/LIF/index"),
            (
                "/projects/42/changes?since=5&limit=20",
                "/public/api/projects/LIF/changes?since=5&limit=20",
            ),
            ("/modules?project_id=42", "/public/api/projects/LIF/modules"),
            ("/labels?project_id=42", "/public/api/projects/LIF/labels"),
            ("/folders?project_id=42", "/public/api/projects/LIF/folders"),
            (
                "/issues/resolve/LIF-9",
                "/public/api/projects/LIF/issues/resolve/LIF-9",
            ),
            (
                "/issues/9?include_waits=true",
                "/public/api/projects/LIF/issues/9?include_waits=true",
            ),
            (
                "/issues/9/comments?before=10",
                "/public/api/projects/LIF/issues/9/comments?before=10",
            ),
            ("/pages/9", "/public/api/projects/LIF/pages/9"),
            (
                "/pages/9/comments",
                "/public/api/projects/LIF/pages/9/comments",
            ),
            (
                "/attachments?entity_type=issue&entity_id=9",
                "/public/api/projects/LIF/attachments?entity_type=issue&entity_id=9",
            ),
            ("/attachments/9", "/public/api/projects/LIF/attachments/9"),
            (
                "/attachments/9/thumbnail",
                "/public/api/projects/LIF/attachments/9/thumbnail",
            ),
            (
                "/attachments/9/preview",
                "/public/api/projects/LIF/attachments/9/preview",
            ),
        ] {
            let resolved = scope.resolve(&Method::GET, path);
            assert_eq!(
                resolved,
                ResolvedRequest::Public {
                    path: expected.into(),
                    wrap_project: path == "/projects"
                }
            );
            let request = resolved
                .request(&base_url, Some("private-token"), Method::GET)
                .unwrap()
                .build()
                .unwrap();
            assert_eq!(request.url().path(), expected.split('?').next().unwrap());
            assert!(!request.headers().contains_key(AUTHORIZATION));
            assert!(!request.headers().contains_key("cookie"));
        }
    }

    #[test]
    fn public_scope_refuses_unsupported_routes_and_every_write() {
        let scope = Scope::public("LIF");
        for path in [
            "/users",
            "/instance",
            "/issues",
            "/modules",
            "/projects/42/members",
            "/attachments?entity_id=9",
            "https://other.example/api",
        ] {
            assert_eq!(scope.resolve(&Method::GET, path), ResolvedRequest::Refused);
        }
        for method in [Method::POST, Method::PATCH, Method::DELETE, Method::PUT] {
            assert_eq!(
                scope.resolve(&method, "/issues/9"),
                ResolvedRequest::Refused
            );
            assert_eq!(scope.resolve(&method, "/auth/me"), ResolvedRequest::Refused);
        }
        assert_eq!(
            Scope::public("a/b").resolve(&Method::GET, "/projects"),
            ResolvedRequest::Public {
                path: "/public/api/projects/A%2FB".into(),
                wrap_project: true
            }
        );
    }

    #[test]
    fn private_requests_inject_only_the_explicit_current_bearer() {
        let base_url = local_api_base();
        let resolved = Scope::Private.resolve(&Method::PATCH, "/auth/me");
        assert_eq!(
            resolved,
            ResolvedRequest::Private {
                path: "/api/auth/me".into()
            }
        );
        let request = resolved
            .request(&base_url, Some("stored-token"), Method::PATCH)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(request.headers()[AUTHORIZATION], "Bearer stored-token");
        let anonymous = resolved
            .request(&base_url, None, Method::GET)
            .unwrap()
            .build()
            .unwrap();
        assert!(!anonymous.headers().contains_key(AUTHORIZATION));
    }

    #[test]
    fn public_synthetic_account_role_and_history_never_send_requests() {
        let scope = Scope::public("LIF");
        let base_url = local_api_base();
        let ResolvedRequest::Synthetic { status, body } = scope.resolve(&Method::GET, "/auth/me")
        else {
            panic!("expected anonymous account");
        };
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "not signed in");
        for (path, expected) in [
            (
                "/projects/42/my-role",
                serde_json::json!({"role":null,"enforced":true,"is_admin":false}),
            ),
            (
                "/issues/9/activity?before=10",
                serde_json::json!({"items":[],"has_more":false}),
            ),
            (
                "/pages/9/activity",
                serde_json::json!({"items":[],"has_more":false}),
            ),
            ("/projects/42/mention-candidates", serde_json::json!([])),
            ("/projects/42/views", serde_json::json!([])),
        ] {
            let resolved = scope.resolve(&Method::GET, path);
            assert_eq!(
                resolved,
                ResolvedRequest::Synthetic {
                    status: StatusCode::OK,
                    body: expected
                }
            );
            assert!(
                resolved
                    .request(&base_url, Some("secret"), Method::GET)
                    .is_none()
            );
        }
    }

    #[test]
    fn role_affordances_match_viewer_maintainer_lead_admin_and_public() {
        for (role, edit, manage) in [
            (ProjectRole::Viewer, false, false),
            (ProjectRole::Maintainer, true, false),
            (ProjectRole::Lead, true, true),
        ] {
            let inputs = RoleInputs {
                role: Some(role),
                enforced: true,
                is_admin: false,
            };
            let affordances = inputs.affordances(&Scope::Private, false);
            assert_eq!(affordances.edit, edit);
            assert_eq!(affordances.manage, manage);
            assert!(affordances.comment);
            assert_eq!(affordances.publish, role == ProjectRole::Lead);
            assert!(!affordances.admin);
            assert_eq!(
                inputs.affordances(&Scope::public("LIF"), false),
                Affordances::default()
            );
        }
        let admin = RoleInputs {
            role: None,
            enforced: true,
            is_admin: true,
        };
        assert_eq!(
            admin.affordances(&Scope::Private, false),
            Affordances {
                edit: true,
                manage: true,
                comment: true,
                publish: true,
                admin: true
            }
        );
        let legacy = RoleInputs {
            role: None,
            enforced: false,
            is_admin: false,
        };
        let affordances = legacy.affordances(&Scope::Private, false);
        assert!(affordances.edit && affordances.manage && affordances.comment);
        assert!(!affordances.publish && !affordances.admin);
        assert!(legacy.affordances(&Scope::Private, true).publish);
    }

    #[tokio::test]
    async fn session_bootstrap_attributes_use_supported_topcoat_attribute_api() {
        let context = Cx::default();
        let cx = &context;
        let markup =
            view! { cx => <div (bootstrap_attributes(cx, &Scope::public("lif"), false))></div> };
        let html = markup.single().await.unwrap().render(cx);
        assert!(html.contains("data-lific-public-project=\"LIF\""));
        assert!(html.contains("data-lific-require-session=\"false\""));
        assert!(html.contains("data-lific-session-state=\"loading\""));
        assert!(!html.contains("lific_token"));
    }

    #[tokio::test]
    async fn session_browser_script_is_served_by_topcoat_discovery() {
        use http_body_util::BodyExt;
        use topcoat::router::RouterBuilderDiscoverExt;
        use tower::ServiceExt;

        let router = topcoat::router::Router::builder().discover().build();
        let response = topcoat::router::tower::TowerService::new(router)
            .oneshot(
                axum::http::Request::builder()
                    .uri(SCRIPT_PATH)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "text/javascript; charset=utf-8"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), BROWSER_SCRIPT.as_bytes());
    }

    // Execute the delivered browser adapter itself. The fake browser supplies
    // only storage, fetch and DOM boundaries, not a copy of its implementation.
    fn browser_contract(test: &str) {
        let mut source = String::from(BROWSER_TEST_ENV);
        source.push_str(BROWSER_SCRIPT);
        source.push_str("\n(async () => {\n");
        source.push_str(test);
        source
            .push_str("\n})().catch(error => { console.error(error); process.exitCode = 1; });\n");
        let output = std::process::Command::new("node")
            .arg("--input-type=commonjs")
            .arg("-e")
            .arg(source)
            .output()
            .expect("node from the repository's devenv is required for browser adapter tests");
        assert!(
            output.status.success(),
            "browser contract failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn stored_token_bootstrap_expiry_clearing_and_unauthenticated_redirect() {
        browser_contract(
            r#"
            localStorage.setItem('lific_token', 'stored');
            replies.push(reply(200, user));
            await lificSession.bootstrap();
            assert.equal(calls[0].url, '/api/auth/me');
            assert.equal(calls[0].options.headers.get('Authorization'), 'Bearer stored');
            assert.deepEqual(lificSession.state.user, user);
            replies.push(reply(401, {error:'Session expired'}));
            await lificSession.refreshAccount();
            assert.equal(localStorage.getItem('lific_token'), null);
            assert.equal(lificSession.state.user, null);
            assert.equal(location.pathname, '/login');
            location.pathname = '/LIF/issues';
            await lificSession.bootstrap();
            assert.equal(calls.length, 2);
            assert.equal(location.pathname, '/login');
        "#,
        );
    }

    #[test]
    fn logout_clears_local_session_even_on_failure_and_refresh_keeps_valid_token() {
        browser_contract(
            r#"
            localStorage.setItem('lific_token', 'stored');
            replies.push(reply(200, user));
            await lificSession.refreshAccount();
            replies.push(reply(503, {error:'offline'}));
            const failed = await lificSession.refreshAccount();
            assert.equal(failed.status, 503);
            assert.equal(localStorage.getItem('lific_token'), 'stored');
            assert.deepEqual(lificSession.state.user, user);
            replies.push(reply(500, {error:'failed logout'}));
            const result = await lificSession.logout();
            assert.equal(result.status, 500);
            assert.equal(calls[2].url, '/api/auth/logout');
            assert.equal(calls[2].options.method, 'POST');
            assert.equal(localStorage.getItem('lific_token'), null);
            assert.equal(lificSession.state.user, null);
            assert.equal(location.pathname, '/login');
        "#,
        );
    }

    #[test]
    fn browser_public_scope_omits_all_credentials_refuses_writes_and_wraps_projects() {
        browser_contract(
            r#"
            localStorage.setItem('lific_token', 'signed-in-admin');
            lificSession.setPublicProject('lif');
            await lificSession.bootstrap();
            assert.equal(calls.length, 0);
            assert.equal(localStorage.getItem('lific_token'), 'signed-in-admin');
            assert.equal(location.pathname, '/LIF/issues');
            replies.push(reply(200, {id:42, identifier:'LIF'}));
            const project = await lificSession.request('/projects', {headers:{Authorization:'other', Cookie:'cookie'}});
            assert.deepEqual(project.data, [{id:42,identifier:'LIF'}]);
            assert.equal(calls[0].url, '/public/api/projects/LIF');
            assert.equal(calls[0].options.credentials, 'omit');
            assert.equal(calls[0].options.headers.get('Authorization'), null);
            assert.equal(calls[0].options.headers.get('Cookie'), null);
            assert.equal((await lificSession.request('/users')).status, 403);
            assert.equal((await lificSession.request('/issues/9', {method:'PATCH'})).status, 403);
            assert.equal((await lificSession.request('/auth/me')).status, 401);
            assert.equal(calls.length, 1);
            assert.equal(localStorage.getItem('lific_token'), 'signed-in-admin');
        "#,
        );
    }

    #[test]
    fn browser_public_read_rewrites_and_scope_navigation_match_existing_routes() {
        browser_contract(
            r#"
            lificSession.setPublicProject('lif');
            for (const [path,expected] of [
                ['/projects/42/index','/public/api/projects/LIF/index'],
                ['/projects/42/changes?since=8','/public/api/projects/LIF/changes?since=8'],
                ['/modules?project_id=42','/public/api/projects/LIF/modules'],
                ['/labels?project_id=42','/public/api/projects/LIF/labels'],
                ['/folders?project_id=42','/public/api/projects/LIF/folders'],
                ['/issues/resolve/LIF-9','/public/api/projects/LIF/issues/resolve/LIF-9'],
                ['/issues/9','/public/api/projects/LIF/issues/9'],
                ['/issues/9/comments?before=8','/public/api/projects/LIF/issues/9/comments?before=8'],
                ['/pages/9','/public/api/projects/LIF/pages/9'],
                ['/pages/9/comments','/public/api/projects/LIF/pages/9/comments'],
                ['/attachments?entity_type=issue&entity_id=9','/public/api/projects/LIF/attachments?entity_type=issue&entity_id=9'],
                ['/attachments/9','/public/api/projects/LIF/attachments/9'],
                ['/attachments/9/thumbnail','/public/api/projects/LIF/attachments/9/thumbnail'],
                ['/attachments/9/preview','/public/api/projects/LIF/attachments/9/preview'],
            ]) {
                replies.push(reply(200, {}));
                const result = await lificSession.request(path);
                assert.equal(result.ok,true);
                assert.equal(calls.at(-1).url,expected);
                assert.equal(calls.at(-1).options.credentials,'omit');
            }
            assert.equal(lificSession.scopedRoute('/LIF/overview'), '/public/LIF/issues');
            assert.equal(lificSession.scopedRoute('/LIF/settings'), '/public/LIF/issues');
            assert.equal(lificSession.scopedRoute('/LIF/issues/LIF-9'), '/public/LIF/issues/LIF-9');
            assert.equal(lificSession.scopedRoute('/public/LIF/issues'), '/public/LIF/issues');
            assert.equal(lificSession.scopedRoute('/login'), '/login');
            localStorage.setItem('lific_token','retained');
            replies.push(reply(401,{error:'public unavailable'}));
            assert.equal((await lificSession.request('/issues/9')).status,401);
            assert.equal(localStorage.getItem('lific_token'),'retained');
            assert.equal(location.pathname,'/LIF/issues');
            location.hash='#/public/other/issues';
            dispatchEvent(new CustomEvent('hashchange'));
            assert.equal(lificSession.state.publicProject,'OTHER');
            location.hash='#/LIF/issues';
            replies.push(reply(200,user));
            dispatchEvent(new CustomEvent('hashchange'));
            await new Promise(resolve => setTimeout(resolve,0));
            assert.equal(lificSession.state.publicProject,null);
            assert.equal(calls.at(-1).url,'/api/auth/me');
            assert.equal(calls.at(-1).options.headers.get('Authorization'),'Bearer retained');
            assert.deepEqual(lificSession.state.user,user);
        "#,
        );
    }

    #[test]
    fn role_visibility_never_authorizes_or_blocks_private_api_requests() {
        browser_contract(
            r#"
            localStorage.setItem('lific_token','viewer');
            replies.push(reply(200,{role:'viewer',enforced:true,is_admin:false}));
            await lificSession.loadRole(42);
            assert.equal(controls.edit.hidden,true);
            replies.push(reply(403,{error:'Requires Maintainer'}));
            const result = await lificSession.request('/issues/9',{method:'PATCH',body:'{}'});
            assert.equal(calls.at(-1).url,'/api/issues/9');
            assert.equal(calls.at(-1).options.headers.get('Authorization'),'Bearer viewer');
            assert.equal(result.status,403);
            assert.equal(result.error,'Requires Maintainer');
            assert.equal(localStorage.getItem('lific_token'),'viewer');
        "#,
        );
    }

    #[test]
    fn account_and_role_refresh_discard_stale_session_and_scope_responses() {
        browser_contract(
            r#"
            localStorage.setItem('lific_token', 'old');
            let complete;
            replies.push(new Promise(resolve => { complete = resolve; }));
            const pending = lificSession.refreshAccount();
            lificSession.saveSession('new');
            complete(reply(401, {error:'expired old session'}));
            await pending;
            assert.equal(localStorage.getItem('lific_token'), 'new');
            replies.push(reply(200, user));
            await lificSession.refreshAccount();
            let roleComplete;
            replies.push(new Promise(resolve => { roleComplete = resolve; }));
            const pendingRole = lificSession.loadRole(42);
            lificSession.setPublicProject('LIF');
            roleComplete(reply(200, {role:'lead',enforced:true,is_admin:true}));
            await pendingRole;
            assert.deepEqual(lificSession.affordances(), {edit:false,manage:false,comment:false,publish:false,admin:false});
            assert.equal(lificSession.state.user, null);
            lificSession.setPublicProject(null);
            replies.push(reply(200, {role:'viewer',enforced:true,is_admin:false}));
            await lificSession.loadRole(42);
            assert.equal(lificSession.affordances().edit, false);
            assert.equal(lificSession.affordances().comment, true);
        "#,
        );
    }

    #[test]
    fn private_response_is_discarded_when_scope_changes_while_body_is_parsed() {
        browser_contract(
            r#"
            localStorage.setItem('lific_token','private-reader');
            let complete;
            const pendingBody = new Promise(resolve => { complete=resolve; });
            replies.push({...reply(200,{}), json:() => pendingBody});
            const pending = lificSession.request('/issues/9');
            await new Promise(resolve => setTimeout(resolve,0));
            lificSession.setPublicProject('LIF');
            complete({id:9,description:'private material'});
            const result = await pending;
            assert.equal(result.ok,false);
            assert.equal(result.code,'audience_changed');
            assert.equal(result.status,null);
            assert.equal('data' in result,false);
            assert.equal('headers' in result,false);
            assert.equal(localStorage.getItem('lific_token'),'private-reader');
        "#,
        );
    }

    #[test]
    fn response_is_discarded_after_token_replacement() {
        browser_contract(
            r#"
            for (const status of [200,409]) {
            localStorage.setItem('lific_token','old-reader');
            let complete;
            replies.push(new Promise(resolve => { complete=resolve; }));
            const pending = lificSession.request('/issues/9');
            lificSession.saveSession('new-reader');
            complete(reply(status,{error:'old private error',code:'update_conflict',current:{description:'old private material'}}));
            const result = await pending;
            assert.equal(result.ok,false);
            assert.equal(result.code,'audience_changed');
            assert.equal(result.status,null);
            assert.equal('data' in result,false);
            assert.equal('current' in result,false);
            assert.equal(result.error.includes('old private'),false);
            assert.equal(localStorage.getItem('lific_token'),'new-reader');
            }
        "#,
        );
    }

    #[test]
    fn public_response_is_discarded_after_public_project_changes() {
        browser_contract(
            r#"
            lificSession.setPublicProject('LIF');
            let complete;
            replies.push(new Promise(resolve => { complete=resolve; }));
            const pending = lificSession.request('/projects');
            lificSession.setPublicProject('OTHER');
            complete(reply(200,{id:42,identifier:'LIF'}));
            const result = await pending;
            assert.equal(result.ok,false);
            assert.equal(result.code,'audience_changed');
            assert.equal(result.status,null);
            assert.equal('data' in result,false);
            assert.equal(lificSession.state.publicProject,'OTHER');
        "#,
        );
    }

    #[test]
    fn browser_role_visibility_and_private_error_metadata_remain_observable() {
        browser_contract(
            r#"
            for (const [role,edit,manage] of [['viewer',false,false],['maintainer',true,false],['lead',true,true]]) {
                replies.push(reply(200, {role,enforced:true,is_admin:false}));
                await lificSession.loadRole(42, true);
                assert.equal(lificSession.affordances().edit, edit);
                assert.equal(lificSession.affordances().manage, manage);
                assert.equal(lificSession.affordances().comment, true);
                assert.equal(controls.edit.hidden, !edit);
                assert.equal(controls.manage.hidden, !manage);
            }
            replies.push(reply(200, {role:null,enforced:true,is_admin:true}));
            await lificSession.loadRole(42, true);
            assert.equal(controls.admin.hidden, false);
            assert.equal(controls.publish.hidden, false);
            replies.push(reply(409, {error:'changed',code:'update_conflict',current:{seq:8}}, {'x-comment-has-more':'true'}));
            const conflict = await lificSession.request('/issues/9', {method:'PATCH',body:JSON.stringify({expected_seq:7})});
            assert.equal(conflict.status, 409);
            assert.equal(conflict.code, 'update_conflict');
            assert.deepEqual(conflict.current, {seq:8});
            assert.equal(conflict.headers.get('x-comment-has-more'), 'true');
            assert.equal(calls.at(-1).url, '/api/issues/9');
        "#,
        );
    }

    const BROWSER_TEST_ENV: &str = r#"
        const assert = require('node:assert/strict');
        global.window = global;
        const listeners = {};
        global.addEventListener = (name, fn) => { (listeners[name] ??= []).push(fn); };
        global.dispatchEvent = event => { for (const fn of listeners[event.type] ?? []) fn(event); };
        global.CustomEvent = class { constructor(type, props={}) { this.type=type; this.detail=props.detail; } };
        const store = new Map();
        global.localStorage = {getItem:key => store.get(key) ?? null, setItem:(key,value) => store.set(key,value), removeItem:key => store.delete(key)};
        global.location = {pathname:'/LIF/issues',hash:'',replace(path) { this.pathname=path; }};
        const controls = Object.fromEntries(['edit','manage','comment','publish','admin'].map(capability => [capability,{dataset:{lificCapability:capability},hidden:false,disabled:false}]));
        global.document = {readyState:'loading',body:{dataset:{lificRequireSession:'true'}},querySelectorAll:selector => selector === '[data-lific-capability]' ? Object.values(controls) : [],addEventListener() {}};
        const calls=[], replies=[];
        global.fetch = async (url,options) => { calls.push({url,options}); const result = replies.shift(); if (result === undefined) throw new Error('unexpected fetch '+url); return await result; };
        const reply = (status,body,headers={}) => ({ok:status>=200&&status<300,status,headers:new Headers(headers),json:async () => body});
        const user = {id:1,username:'reader',email:'reader@example.test',display_name:'Reader',is_admin:false};
    "#;
}
