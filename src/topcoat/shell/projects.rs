//! Project catalog navigation shared by authenticated desktop shell pages.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

use super::context::ProjectCatalogSnapshot;

pub(crate) const SCRIPT: &str = include_str!("assets/projects.js");
pub(crate) const STYLESHEET: &str = include_str!("assets/projects.css");
pub(crate) const SCRIPT_PATH: &str = "/__topcoat-projects.js";
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-projects.css";

/// Render server-ordered groups and projects into the desktop navigation.
/// Public callers pass no catalog, so published projects cannot expose private
/// groups or a project switcher.
pub(crate) fn project_tree<'a>(
    cx: &'a Cx,
    catalog: Option<&ProjectCatalogSnapshot>,
    active_identifier: Option<&str>,
) -> Option<BoxView<'a>> {
    let catalog = catalog?;
    let grouped = catalog
        .groups
        .iter()
        .flat_map(|group| group.project_ids.iter().copied())
        .collect::<std::collections::BTreeSet<_>>();
    let ungrouped = catalog
        .projects
        .iter()
        .filter(|project| !grouped.contains(&project.id))
        .map(|project| {
            (
                project.id,
                project.identifier.clone(),
                project.name.clone(),
                project.emoji.clone(),
                active_identifier
                    .is_some_and(|active| active.eq_ignore_ascii_case(&project.identifier)),
            )
        })
        .collect::<Vec<_>>();
    let groups = catalog
        .groups
        .iter()
        .map(|group| {
            let projects = catalog
                .projects
                .iter()
                .filter(|project| group.project_ids.contains(&project.id))
                .map(|project| {
                    (
                        project.id,
                        project.identifier.clone(),
                        project.name.clone(),
                        project.emoji.clone(),
                        active_identifier
                            .is_some_and(|active| active.eq_ignore_ascii_case(&project.identifier)),
                    )
                })
                .collect::<Vec<_>>();
            (group.id, group.name.clone(), projects)
        })
        .collect::<Vec<_>>();
    let generation = catalog.generation.to_string();
    Some(view! { cx =>
        <section class="tc-projects" aria-label="Projects" data-topcoat-projects="" data-catalog-generation=(generation)>
            <h2 class="tc-projects__heading">"Projects"</h2>
            <div class="tc-projects__groups">
                for (group_id, group_name, projects) in groups {
                    <section class="tc-projects__group" data-group-id=(group_id.to_string()) draggable="true">
                        <h3 class="tc-projects__group-heading">
                            <button type="button" class="tc-projects__disclosure" data-action="disclose"
                                aria-expanded="true" aria-label=(format!("Collapse {group_name}"))>"▾"</button>
                            (group_name)
                        </h3>
                        <div class="tc-projects__project-list">
                            for (project_id, identifier, name, emoji, active) in projects {
                                <a class="tc-projects__project" href=(format!("/{identifier}/overview"))
                                    data-project-id=(project_id.to_string()) draggable="true"
                                    aria-current=(active.then_some("page"))>
                                    if let Some(emoji) = emoji { (format!("{emoji} ")) }
                                    (name)
                                </a>
                            }
                        </div>
                    </section>
                }
                <section class="tc-projects__group tc-projects__group--ungrouped" data-group-id="">
                    <h3 class="tc-projects__group-name">"Ungrouped"</h3>
                    <div class="tc-projects__project-list">
                        for (project_id, identifier, name, emoji, active) in ungrouped {
                            <a class="tc-projects__project" href=(format!("/{identifier}/overview"))
                                data-project-id=(project_id.to_string()) draggable="true"
                                aria-current=(active.then_some("page"))>
                                if let Some(emoji) = emoji { (format!("{emoji} ")) }
                                (name)
                            </a>
                        }
                    </div>
                </section>
            </div>
            <p class="tc-projects__error" role="status" hidden="true"></p>
        </section>
    }.boxed())
}
