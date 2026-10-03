//! Shared immutable inputs for shell consumers.

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    db::models::AuthUser,
    server::topcoat_frontend::{
        api::dto::project::Project as ProjectDto,
        session::{Affordances, ProjectRole},
    },
};

use super::ParsedRoute;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectSummary {
    pub(crate) id: i64,
    pub(crate) identifier: String,
    pub(crate) name: String,
    pub(crate) emoji: Option<String>,
}

impl From<&ProjectDto> for ProjectSummary {
    fn from(project: &ProjectDto) -> Self {
        Self {
            id: project.id,
            identifier: project.identifier.clone(),
            name: project.name.clone(),
            emoji: project.emoji.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectGroupSummary {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) sort_order: i64,
    pub(crate) project_ids: Arc<[i64]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectCatalogSnapshot {
    /// Monotonic request generation within one authenticated user's catalog.
    pub(crate) generation: u64,
    pub(crate) groups: Arc<[ProjectGroupSummary]>,
    pub(crate) projects: Arc<[ProjectSummary]>,
}

pub(crate) type ProjectCatalogSubscriber =
    Arc<dyn Fn(Arc<ProjectCatalogSnapshot>) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectCatalogCommand {
    CreateGroup {
        name: String,
    },
    RenameGroup {
        id: i64,
        name: String,
    },
    DeleteGroup {
        id: i64,
    },
    AssignProject {
        project_id: i64,
        group_id: Option<i64>,
    },
    ReorderGroups {
        ids: Vec<i64>,
    },
    ReorderProjects {
        ids: Vec<i64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectCatalogCommandOutcome {
    GroupCreated { group_id: i64 },
    Applied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectCatalogCommandError {
    NotFound,
    Forbidden,
    Conflict,
    InvalidInput(String),
    Unavailable,
}

pub(crate) type ProjectCatalogCommandCompletion = Arc<
    dyn Fn(Result<ProjectCatalogCommandOutcome, ProjectCatalogCommandError>)
        + Send
        + Sync
        + 'static,
>;

pub(crate) type ProjectCatalogCommandHandler =
    Arc<dyn Fn(ProjectCatalogCommand, ProjectCatalogCommandCompletion) + Send + Sync + 'static>;

pub(crate) fn dispatch_project_catalog_command(
    handler: &ProjectCatalogCommandHandler,
    command: ProjectCatalogCommand,
    completion: ProjectCatalogCommandCompletion,
) {
    handler(command, completion);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProjectCatalogSubscription(u64);

#[derive(Default)]
pub(crate) struct ProjectCatalogStore {
    current: Option<Arc<ProjectCatalogSnapshot>>,
    next_subscription: u64,
    subscribers: BTreeMap<ProjectCatalogSubscription, ProjectCatalogSubscriber>,
}

impl ProjectCatalogStore {
    pub(crate) fn subscribe(
        &mut self,
        subscriber: ProjectCatalogSubscriber,
    ) -> ProjectCatalogSubscription {
        self.next_subscription = self.next_subscription.wrapping_add(1);
        let subscription = ProjectCatalogSubscription(self.next_subscription);
        self.subscribers.insert(subscription, subscriber);
        subscription
    }

    pub(crate) fn unsubscribe(&mut self, subscription: ProjectCatalogSubscription) {
        self.subscribers.remove(&subscription);
    }

    /// Publishes only newer fetch generations. Callbacks receive an immutable
    /// shared snapshot and run after the store's subscriber set is copied.
    pub(crate) fn publish(&mut self, snapshot: Arc<ProjectCatalogSnapshot>) -> bool {
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.generation >= snapshot.generation)
        {
            return false;
        }
        self.current = Some(Arc::clone(&snapshot));
        let subscribers = self.subscribers.values().cloned().collect::<Vec<_>>();
        for subscriber in subscribers {
            subscriber(Arc::clone(&snapshot));
        }
        true
    }

    pub(crate) fn current(&self) -> Option<&Arc<ProjectCatalogSnapshot>> {
        self.current.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectContext {
    pub(crate) project: ProjectSummary,
    pub(crate) role: Option<ProjectRole>,
    pub(crate) affordances: Affordances,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellPrincipal<'a> {
    Auth,
    Private {
        user: Option<&'a AuthUser>,
        catalog: Option<Arc<ProjectCatalogSnapshot>>,
        active_project: Option<ProjectContext>,
    },
    Public {
        project: &'a str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellContext<'a> {
    route: &'a ParsedRoute<'a>,
    principal: ShellPrincipal<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellContextError {
    WrongAudience,
    MissingPublicProject,
    ProjectMismatch,
}

impl<'a> ShellContext<'a> {
    pub(crate) fn auth(route: &'a ParsedRoute<'a>) -> Result<Self, ShellContextError> {
        if route.layout != super::Layout::Auth {
            return Err(ShellContextError::WrongAudience);
        }
        Ok(Self {
            route,
            principal: ShellPrincipal::Auth,
        })
    }

    pub(crate) fn private(
        route: &'a ParsedRoute<'a>,
        user: Option<&'a AuthUser>,
        catalog: Option<Arc<ProjectCatalogSnapshot>>,
        active_project: Option<ProjectContext>,
    ) -> Result<Self, ShellContextError> {
        if route.layout != super::Layout::Private {
            return Err(ShellContextError::WrongAudience);
        }
        if active_project.as_ref().is_some_and(|project| {
            route.project.is_some_and(|identifier| {
                !identifier.eq_ignore_ascii_case(&project.project.identifier)
            })
        }) {
            return Err(ShellContextError::ProjectMismatch);
        }
        Ok(Self {
            route,
            principal: ShellPrincipal::Private {
                user,
                catalog,
                active_project,
            },
        })
    }

    pub(crate) fn public(route: &'a ParsedRoute<'a>) -> Result<Self, ShellContextError> {
        if route.layout != super::Layout::Public {
            return Err(ShellContextError::WrongAudience);
        }
        let Some(project) = route.project else {
            return Err(ShellContextError::MissingPublicProject);
        };
        Ok(Self {
            route,
            principal: ShellPrincipal::Public { project },
        })
    }

    pub(crate) fn route(&self) -> &ParsedRoute<'a> {
        self.route
    }

    pub(crate) fn principal(&self) -> &ShellPrincipal<'a> {
        &self.principal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryMode {
    Push,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NavigationRequest {
    pub(crate) href: String,
    pub(crate) history: HistoryMode,
}

/// The shell browser adapter is the sole owner of browser history mutation.
pub(crate) type NavigationCallback = Arc<dyn Fn(NavigationRequest) + Send + Sync + 'static>;

pub(crate) fn request_navigation(callback: &NavigationCallback, request: NavigationRequest) {
    callback(request);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Breadcrumb {
    pub(crate) label: String,
    pub(crate) href: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageTab {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) count: Option<usize>,
    pub(crate) selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageAction {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) shortcut: Option<String>,
    pub(crate) behavior: PageActionBehavior,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PageActionBehavior {
    Navigate(NavigationRequest),
    Dispatch(PageActionCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PageActionCommand {
    SetIssueStatus { issue_id: String, status: String },
    SetIssuePriority { issue_id: String, priority: String },
    OpenPicker { picker: String },
}

pub(crate) type PageActionHandler = Arc<dyn Fn(PageActionCommand) + Send + Sync + 'static>;

pub(crate) fn dispatch_page_action(handler: &PageActionHandler, command: PageActionCommand) {
    handler(command);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageMetadata {
    pub(crate) title: String,
    pub(crate) breadcrumbs: Vec<Breadcrumb>,
    pub(crate) tabs: Vec<PageTab>,
    pub(crate) trailing_actions: Vec<PageAction>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PageRegistry {
    generation: u64,
    route_key: Option<String>,
    metadata: Option<PageMetadata>,
}

impl PageRegistry {
    /// Activating a route invalidates metadata published by earlier renders.
    pub(crate) fn activate(&mut self, route_key: String) -> u64 {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.route_key = Some(route_key);
        self.metadata = None;
        self.generation
    }

    /// Late asynchronous page registrations are ignored after route changes.
    pub(crate) fn publish(&mut self, generation: u64, metadata: PageMetadata) -> bool {
        if generation != self.generation || self.route_key.is_none() {
            return false;
        }
        self.metadata = Some(metadata);
        true
    }

    pub(crate) fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.route_key = None;
        self.metadata = None;
    }

    pub(crate) fn current(&self) -> Option<(&str, &PageMetadata)> {
        self.route_key.as_deref().zip(self.metadata.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(title: &str, action: &str) -> PageMetadata {
        PageMetadata {
            title: title.to_owned(),
            breadcrumbs: vec![Breadcrumb {
                label: title.to_owned(),
                href: None,
            }],
            tabs: vec![PageTab {
                id: "all".to_owned(),
                label: "All".to_owned(),
                count: Some(3),
                selected: true,
            }],
            trailing_actions: vec![PageAction {
                id: action.to_owned(),
                label: action.to_owned(),
                shortcut: None,
                behavior: PageActionBehavior::Dispatch(PageActionCommand::OpenPicker {
                    picker: "create".to_owned(),
                }),
            }],
        }
    }

    #[test]
    fn page_registry_clears_metadata_on_activation_and_rejects_stale_publications() {
        let mut registry = PageRegistry::default();
        let issues_generation = registry.activate("/LIF/issues".to_owned());
        assert!(registry.publish(issues_generation, metadata("Issues", "Create issue")));
        assert_eq!(
            registry.current().unwrap().1.trailing_actions[0].id,
            "Create issue"
        );

        let pages_generation = registry.activate("/LIF/pages".to_owned());
        assert_ne!(issues_generation, pages_generation);
        assert_eq!(registry.current(), None);
        assert!(!registry.publish(issues_generation, metadata("Stale", "Stale action")));
        assert!(registry.publish(pages_generation, metadata("Pages", "Create page")));
        let (route, current) = registry.current().unwrap();
        assert_eq!(route, "/LIF/pages");
        assert_eq!(current.title, "Pages");
        assert_eq!(current.trailing_actions[0].id, "Create page");

        registry.clear();
        assert_eq!(registry.current(), None);
    }

    #[test]
    fn shell_context_constructors_enforce_route_audience_and_project_identity() {
        let auth_route = ParsedRoute::parse("/login");
        let auth_context = ShellContext::auth(&auth_route).unwrap();
        assert!(matches!(auth_context.principal(), ShellPrincipal::Auth));

        let route = ParsedRoute::parse("/public/LIF/issues");
        let context = ShellContext::public(&route).unwrap();

        assert!(matches!(
            context.principal(),
            ShellPrincipal::Public { project: "LIF" }
        ));
        assert_eq!(context.route().project, Some("LIF"));

        let private_route = ParsedRoute::parse("/LIF/issues");
        assert!(matches!(
            ShellContext::public(&private_route),
            Err(ShellContextError::WrongAudience)
        ));
        assert!(matches!(
            ShellContext::auth(&private_route),
            Err(ShellContextError::WrongAudience)
        ));
        assert!(matches!(
            ShellContext::private(&route, None, None, None),
            Err(ShellContextError::WrongAudience)
        ));

        let missing_project = ParsedRoute {
            layout: super::super::Layout::Public,
            page: super::super::Page::NotFound,
            project: None,
            query: "",
            fragment: "",
            redirect: None,
        };
        assert!(matches!(
            ShellContext::public(&missing_project),
            Err(ShellContextError::MissingPublicProject)
        ));

        let mismatched_project = ProjectContext {
            project: ProjectSummary {
                id: 9,
                identifier: "OTHER".to_owned(),
                name: "Other".to_owned(),
                emoji: None,
            },
            role: None,
            affordances: Affordances::default(),
        };
        assert!(matches!(
            ShellContext::private(&private_route, None, None, Some(mismatched_project)),
            Err(ShellContextError::ProjectMismatch)
        ));

        let lowercase_route = ParsedRoute::parse("/lif/issues");
        let canonical_project = ProjectContext {
            project: ProjectSummary {
                id: 1,
                identifier: "LIF".to_owned(),
                name: "Lific".to_owned(),
                emoji: None,
            },
            role: None,
            affordances: Affordances::default(),
        };
        assert!(
            ShellContext::private(&lowercase_route, None, None, Some(canonical_project)).is_ok()
        );
    }

    #[test]
    fn project_catalog_only_publishes_new_generations_to_current_subscribers() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let notifications = Arc::new(AtomicUsize::new(0));
        let mut store = ProjectCatalogStore::default();
        let notified = Arc::clone(&notifications);
        let subscription = store.subscribe(Arc::new(move |_| {
            notified.fetch_add(1, Ordering::SeqCst);
        }));
        let snapshot = Arc::new(ProjectCatalogSnapshot {
            generation: 2,
            groups: Arc::from([]),
            projects: Arc::from([]),
        });

        assert!(store.publish(Arc::clone(&snapshot)));
        assert!(!store.publish(Arc::clone(&snapshot)));
        assert!(!store.publish(Arc::new(ProjectCatalogSnapshot {
            generation: 1,
            groups: Arc::from([]),
            projects: Arc::from([]),
        })));
        assert!(Arc::ptr_eq(store.current().unwrap(), &snapshot));
        assert_eq!(notifications.load(Ordering::SeqCst), 1);

        store.unsubscribe(subscription);
        assert!(store.publish(Arc::new(ProjectCatalogSnapshot {
            generation: 3,
            groups: Arc::from([]),
            projects: Arc::from([]),
        })));
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn navigation_callback_receives_history_mode_and_full_destination() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let callback: NavigationCallback = {
            let captured = Arc::clone(&captured);
            Arc::new(move |request| captured.lock().unwrap().push(request))
        };

        request_navigation(
            &callback,
            NavigationRequest {
                href: "/LIF/issues?status=open#issue-7".to_owned(),
                history: HistoryMode::Push,
            },
        );
        request_navigation(
            &callback,
            NavigationRequest {
                href: "/LIF/issues?status=closed#issue-8".to_owned(),
                history: HistoryMode::Replace,
            },
        );

        let captured = captured.lock().unwrap();
        assert_eq!(captured[0].history, HistoryMode::Push);
        assert_eq!(captured[0].href, "/LIF/issues?status=open#issue-7");
        assert_eq!(captured[1].history, HistoryMode::Replace);
        assert_eq!(captured[1].href, "/LIF/issues?status=closed#issue-8");
    }

    #[test]
    fn project_catalog_command_completion_returns_created_id_and_typed_failure() {
        let completed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let handler: ProjectCatalogCommandHandler = Arc::new(|command, completion| match command {
            ProjectCatalogCommand::CreateGroup { .. } => {
                completion(Ok(ProjectCatalogCommandOutcome::GroupCreated {
                    group_id: 42,
                }))
            }
            ProjectCatalogCommand::DeleteGroup { .. } => {
                completion(Err(ProjectCatalogCommandError::Conflict))
            }
            _ => completion(Ok(ProjectCatalogCommandOutcome::Applied)),
        });

        for command in [
            ProjectCatalogCommand::CreateGroup {
                name: "Recently used".to_owned(),
            },
            ProjectCatalogCommand::DeleteGroup { id: 42 },
        ] {
            let result = Arc::clone(&completed);
            dispatch_project_catalog_command(
                &handler,
                command,
                Arc::new(move |outcome| result.lock().unwrap().push(outcome)),
            );
        }

        let completed = completed.lock().unwrap();
        assert_eq!(
            completed[0],
            Ok(ProjectCatalogCommandOutcome::GroupCreated { group_id: 42 })
        );
        assert_eq!(completed[1], Err(ProjectCatalogCommandError::Conflict));
    }

    #[test]
    fn page_action_dispatch_executes_non_navigation_commands() {
        let captured = Arc::new(std::sync::Mutex::new(None));
        let handler: PageActionHandler = {
            let captured = Arc::clone(&captured);
            Arc::new(move |command| *captured.lock().unwrap() = Some(command))
        };
        let command = PageActionCommand::SetIssueStatus {
            issue_id: "LIF-42".to_owned(),
            status: "Done".to_owned(),
        };

        dispatch_page_action(&handler, command.clone());

        assert_eq!(*captured.lock().unwrap(), Some(command));
    }

    #[test]
    fn project_summary_keeps_only_fields_needed_by_shell_navigation() {
        let project = ProjectDto {
            id: 7,
            name: "Lific".to_owned(),
            identifier: "LIF".to_owned(),
            description: "Project details".to_owned(),
            emoji: Some("🦀".to_owned()),
            lead_user_id: Some(9),
            sort_order: 1,
            created_at: "created".to_owned(),
            updated_at: "updated".to_owned(),
            is_public: false,
        };

        assert_eq!(
            ProjectSummary::from(&project),
            ProjectSummary {
                id: 7,
                identifier: "LIF".to_owned(),
                name: "Lific".to_owned(),
                emoji: Some("🦀".to_owned()),
            }
        );
    }
}
