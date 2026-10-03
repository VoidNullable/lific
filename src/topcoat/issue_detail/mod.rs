//! Shared contracts for independently implemented issue-detail components.

use super::api::dto::issue::Issue;

/// A new generation belongs to every route activation, including revisits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RouteKey {
    pub(crate) issue_id: i64,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Capabilities {
    pub(crate) edit: bool,
    pub(crate) comment: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScalarChange {
    Title(String),
    Status(String),
    Priority(String),
    Module(Option<i64>),
    Labels(Vec<String>),
    StartDate(String),
    TargetDate(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Panel {
    Comments,
    Relations,
    Waits,
}

/// Components emit intents; the route owns dispatch and the write queue.
pub(crate) struct DetailIntent {
    pub(crate) route: RouteKey,
    pub(crate) action: Intent,
}

pub(crate) enum Intent {
    SetScalar(ScalarChange),
    EditDescription(String),
    SaveDescription,
    MutatePanel(PanelAction),
    /// The panel has updated its own rows; refresh the parent's Issue next.
    PanelMutationCompleted(Panel),
    Delete,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PanelAction {
    CreateComment {
        content: String,
    },
    EditComment {
        comment_id: i64,
        content: String,
    },
    DeleteComment {
        comment_id: i64,
    },
    LinkRelation {
        source: String,
        target: String,
        kind: RelationKind,
    },
    UnlinkRelation {
        source: String,
        target: String,
    },
    ReverseRelation {
        source: String,
        target: String,
    },
    AddWait(WaitInput),
    ClearWait {
        wait_id: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationKind {
    Blocks,
    RelatesTo,
    Duplicate,
}

/// Matches the existing wait endpoint's mutually exclusive `user`/`from` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WaitInput {
    User {
        username: String,
        note: String,
    },
    Date {
        from: String,
        until: Option<String>,
        note: String,
    },
}

pub(crate) struct ScalarProps<'a> {
    pub(crate) route: RouteKey,
    pub(crate) issue: &'a Issue,
    pub(crate) capabilities: Capabilities,
}

pub(crate) struct EditorProps<'a> {
    pub(crate) route: RouteKey,
    pub(crate) text: &'a str,
    pub(crate) saved_description: &'a str,
    pub(crate) dirty: bool,
    pub(crate) expected_seq: i64,
    pub(crate) capabilities: Capabilities,
}

pub(crate) struct CollaborationProps<'a> {
    pub(crate) route: RouteKey,
    pub(crate) issue: &'a Issue,
    pub(crate) capabilities: Capabilities,
}

/// Capture at dispatch, after earlier issue/panel writes have completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorSave {
    pub(crate) route: RouteKey,
    pub(crate) expected_seq: i64,
    pub(crate) description: String,
    edit_revision: u64,
}

pub(crate) enum DetailEvent {
    ScalarApplied {
        route: RouteKey,
        issue: Issue,
    },
    EditorApplied {
        save: EditorSave,
        issue: Issue,
    },
    CollaborationApplied {
        route: RouteKey,
        panel: Panel,
        issue: Issue,
    },
    Conflict {
        route: RouteKey,
        current: Issue,
    },
    Deleted {
        route: RouteKey,
    },
    Restored {
        route: RouteKey,
        issue: Issue,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyOutcome {
    Applied,
    Stale,
}

pub(crate) struct Coordinator {
    route: RouteKey,
    issue: Issue,
    text: String,
    dirty: bool,
    edit_revision: u64,
    deleted: bool,
}

impl Coordinator {
    pub(crate) fn new(generation: u64, issue: Issue) -> Self {
        Self {
            route: RouteKey {
                issue_id: issue.id,
                generation,
            },
            text: issue.description.clone(),
            issue,
            dirty: false,
            edit_revision: 0,
            deleted: false,
        }
    }

    pub(crate) fn edit(&mut self, text: String) {
        self.text = text;
        self.dirty = self.text != self.issue.description;
        self.edit_revision += 1;
    }

    pub(crate) fn editor_props(&self, capabilities: Capabilities) -> EditorProps<'_> {
        EditorProps {
            route: self.route,
            text: &self.text,
            saved_description: &self.issue.description,
            dirty: self.dirty,
            expected_seq: self.issue.expected_seq(),
            capabilities: self.capabilities(capabilities),
        }
    }

    pub(crate) fn scalar_props(&self, capabilities: Capabilities) -> ScalarProps<'_> {
        ScalarProps {
            route: self.route,
            issue: &self.issue,
            capabilities: self.capabilities(capabilities),
        }
    }

    pub(crate) fn collaboration_props(&self, capabilities: Capabilities) -> CollaborationProps<'_> {
        CollaborationProps {
            route: self.route,
            issue: &self.issue,
            capabilities: self.capabilities(capabilities),
        }
    }

    fn capabilities(&self, capabilities: Capabilities) -> Capabilities {
        match self.deleted {
            true => Capabilities {
                edit: false,
                comment: false,
            },
            false => capabilities,
        }
    }

    #[must_use]
    pub(crate) fn editor_save(&self) -> Option<EditorSave> {
        (!self.deleted).then(|| EditorSave {
            route: self.route,
            expected_seq: self.issue.expected_seq(),
            description: self.text.clone(),
            edit_revision: self.edit_revision,
        })
    }

    pub(crate) fn apply(&mut self, event: DetailEvent) -> ApplyOutcome {
        match event {
            DetailEvent::ScalarApplied { route, issue }
            | DetailEvent::CollaborationApplied { route, issue, .. }
            | DetailEvent::Conflict {
                route,
                current: issue,
            } => self.publish(route, issue, None, false),
            DetailEvent::EditorApplied { save, issue } => {
                self.publish(save.route, issue, Some(save), false)
            }
            DetailEvent::Restored { route, issue } => self.publish(route, issue, None, true),
            DetailEvent::Deleted { route } => match route == self.route {
                true => {
                    self.deleted = true;
                    ApplyOutcome::Applied
                }
                false => ApplyOutcome::Stale,
            },
        }
    }

    /// Validate completions once at the route boundary before changing props.
    fn publish(
        &mut self,
        route: RouteKey,
        issue: Issue,
        save: Option<EditorSave>,
        restoring: bool,
    ) -> ApplyOutcome {
        let current = route == self.route
            && issue.id == self.route.issue_id
            && issue.seq >= self.issue.seq
            && (!self.deleted || restoring);
        match current {
            false => ApplyOutcome::Stale,
            true => {
                self.issue = issue;
                self.deleted = false;
                match save {
                    Some(save) => match save.edit_revision == self.edit_revision
                        && save.description == self.text
                    {
                        true => {
                            self.text = self.issue.description.clone();
                            self.dirty = false;
                        }
                        false => self.dirty = true,
                    },
                    None => {
                        if !self.dirty {
                            self.text = self.issue.description.clone();
                        }
                    }
                }
                ApplyOutcome::Applied
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTE: RouteKey = RouteKey {
        issue_id: 31,
        generation: 1,
    };
    const EDITABLE: Capabilities = Capabilities {
        edit: true,
        comment: true,
    };

    fn issue(seq: i64, description: &str) -> Issue {
        serde_json::from_value(serde_json::json!({
            "id": 31, "project_id": 7, "sequence": 4, "identifier": "ENG-4",
            "title": "Engine", "description": description, "status": "started",
            "priority": "high", "module_id": null, "sort_order": 0,
            "start_date": null, "target_date": null,
            "created_at": "2026-10-02T12:00:00Z", "updated_at": "2026-10-02T12:00:00Z",
            "seq": seq, "labels": []
        }))
        .unwrap()
    }

    #[test]
    fn scalar_response_advances_the_next_editor_saves_expected_seq() {
        let mut coordinator = Coordinator::new(ROUTE.generation, issue(7, "Saved"));
        coordinator.edit("Draft".to_owned());
        coordinator.apply(DetailEvent::ScalarApplied {
            route: ROUTE,
            issue: issue(9, "Saved"),
        });

        let save = coordinator.editor_save().unwrap();
        assert_eq!(save.expected_seq, 9);
        assert_eq!(save.description, "Draft");
    }

    #[test]
    fn collaboration_response_preserves_unsaved_description() {
        let mut coordinator = Coordinator::new(ROUTE.generation, issue(7, "Saved"));
        coordinator.edit("Unsaved markdown".to_owned());
        coordinator.apply(DetailEvent::CollaborationApplied {
            route: ROUTE,
            panel: Panel::Comments,
            issue: issue(11, "Saved"),
        });

        let props = coordinator.editor_props(EDITABLE);
        assert_eq!(props.text, "Unsaved markdown");
        assert!(props.dirty);
        assert_eq!(props.expected_seq, 11);
    }

    #[test]
    fn older_editor_response_cannot_replace_newer_text_or_sequence() {
        let mut coordinator = Coordinator::new(ROUTE.generation, issue(7, "Saved"));
        coordinator.edit("First edit".to_owned());
        let old_save = coordinator.editor_save().unwrap();
        coordinator.edit("New edit".to_owned());
        let new_save = coordinator.editor_save().unwrap();
        coordinator.apply(DetailEvent::EditorApplied {
            save: new_save,
            issue: issue(12, "New edit"),
        });
        coordinator.apply(DetailEvent::EditorApplied {
            save: old_save,
            issue: issue(9, "First edit"),
        });

        let props = coordinator.editor_props(EDITABLE);
        assert_eq!(props.text, "New edit");
        assert_eq!(props.expected_seq, 12);
        assert!(!props.dirty);
    }
}
