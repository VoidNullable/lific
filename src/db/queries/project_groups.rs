//! Per-user sidebar folders, stored as project groups.
//!
//! A folder holds projects and other folders, up to [`MAX_DEPTH`] levels. Each
//! container (the top level or a folder) orders its folders and projects
//! together. A folder's rank is `project_groups.sort_order` and a project's is
//! its `user_project_order` row, and every write renumbers both in
//! depth-first order of the whole tree, so the flat personal project order
//! (`GET /api/projects`) reads the same way the sidebar does.
//!
//! Ownership mirrors `queries::views`: every function targeting an existing
//! folder resolves it through the caller's own rows, which folds "no such
//! folder" and "someone else's folder" into the same `NotFound`, so a folder id
//! can't be used to probe another user's sidebar.
//!
//! `visible` is the caller's project visibility (`None` means every project),
//! resolved by the caller inside the same transaction. Memberships can outlive
//! access: a project the caller can no longer see is left out of the tree but
//! keeps its folder, so it reappears there if access comes back.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params};

use crate::db::models::{
    CreateProjectGroup, MoveSidebarItem, ProjectGroup, SidebarItem, SidebarLayout,
    UpdateProjectGroup,
};
use crate::error::LificError;

/// Deepest nesting allowed: a top-level folder is level 1.
pub const MAX_DEPTH: usize = 4;
/// Folders per user. A sidebar past this is no longer navigable, and the
/// tree is rebuilt in memory on every write.
pub const MAX_FOLDERS: i64 = 200;
pub const MAX_NAME_CHARS: usize = 80;

pub type Visible = Option<HashSet<i64>>;

fn validate_name(name: &str) -> Result<(), LificError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(LificError::BadRequest("Enter a folder name.".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(LificError::BadRequest(format!(
            "Folder names can be at most {MAX_NAME_CHARS} characters."
        )));
    }
    Ok(())
}

/// Map the `UNIQUE (user_id, name)` violation onto a 409 the client can show
/// verbatim. Names stay unique per user, not per parent, so moving a folder or
/// deleting its parent can never collide with a sibling.
fn constraint_err(name: &str) -> impl Fn(rusqlite::Error) -> LificError + '_ {
    move |e| match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            LificError::Conflict(format!("You already have a folder named '{name}'."))
        }
        other => other.into(),
    }
}

fn not_found(id: i64) -> LificError {
    LificError::NotFound(format!("folder {id} not found"))
}

const CYCLE: &str = "A folder can't go inside itself or one of its own subfolders.";

struct FolderRow {
    id: i64,
    user_id: i64,
    name: String,
    sort_order: i64,
    created_at: String,
    updated_at: String,
}

/// Sort key inside one container. Ranked items share one number space;
/// projects the user has never ordered follow, in the legacy order. A folder
/// wins a tie with a project, which is what keeps folders above loose
/// projects for anyone who has not rearranged since migration 078.
type Key = (u8, i64, u8, i64);

/// The caller's tree, loaded whole. Small by construction ([`MAX_FOLDERS`]
/// plus the visible projects), so every write loads, edits and renumbers it.
struct Tree {
    folders: HashMap<i64, FolderRow>,
    /// Effective parent of each folder, after cycle repair.
    parent: HashMap<i64, Option<i64>>,
    children: HashMap<Option<i64>, Vec<SidebarItem>>,
    /// Visible projects in a folder, and the folder.
    project_folder: HashMap<i64, i64>,
    /// Every visible project, wherever it sits.
    projects: HashSet<i64>,
}

impl Tree {
    fn load(conn: &Connection, user_id: i64, visible: &Visible) -> Result<Tree, LificError> {
        let mut stmt = conn.prepare_cached(
            "SELECT id, user_id, name, parent_id, sort_order, created_at, updated_at
             FROM project_groups WHERE user_id = ?1 ORDER BY sort_order, id",
        )?;
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok((
                    FolderRow {
                        id: row.get(0)?,
                        user_id: row.get(1)?,
                        name: row.get(2)?,
                        sort_order: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    },
                    row.get::<_, Option<i64>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let ids: HashSet<i64> = rows.iter().map(|(f, _)| f.id).collect();
        let order: Vec<i64> = rows.iter().map(|(f, _)| f.id).collect();
        let mut parent: HashMap<i64, Option<i64>> = rows
            .iter()
            .map(|(f, p)| (f.id, p.filter(|p| ids.contains(p))))
            .collect();
        // The query layer never writes a cycle, but a hand-edited database
        // could hold one. Cut the edge that closes each loop so every folder
        // still renders somewhere instead of the walk below never ending.
        for &start in &order {
            let mut seen = HashSet::from([start]);
            let mut last = start;
            while let Some(Some(p)) = parent.get(&last).copied() {
                if !seen.insert(p) {
                    parent.insert(last, None);
                    break;
                }
                last = p;
            }
        }

        let mut stmt =
            conn.prepare_cached("SELECT project_id FROM user_project_order WHERE user_id = ?1")?;
        let ranked: HashSet<i64> = stmt
            .query_map([user_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let projects: Vec<(i64, i64)> = super::list_projects_for_user(conn, user_id)?
            .into_iter()
            .filter(|p| visible.as_ref().is_none_or(|v| v.contains(&p.id)))
            .map(|p| (p.id, p.sort_order))
            .collect();

        let mut stmt = conn.prepare_cached(
            "SELECT i.project_id, i.group_id
             FROM project_group_items i
             JOIN project_groups g ON g.id = i.group_id
             WHERE g.user_id = ?1
             ORDER BY i.group_id",
        )?;
        let mut membership: HashMap<i64, i64> = HashMap::new();
        for row in stmt.query_map(params![user_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })? {
            let (project, group) = row?;
            membership.entry(project).or_insert(group);
        }

        let mut keyed: HashMap<Option<i64>, Vec<(Key, SidebarItem)>> = HashMap::new();
        for (folder, _) in &rows {
            keyed.entry(parent[&folder.id]).or_default().push((
                (0, folder.sort_order, 0, folder.id),
                SidebarItem::Folder { id: folder.id },
            ));
        }
        let mut project_folder = HashMap::new();
        for (pos, (id, rank)) in projects.iter().enumerate() {
            let container = membership.get(id).copied();
            if let Some(folder) = container {
                project_folder.insert(*id, folder);
            }
            let key = if ranked.contains(id) {
                (0, *rank, 1, pos as i64)
            } else {
                (1, pos as i64, 1, 0)
            };
            keyed
                .entry(container)
                .or_default()
                .push((key, SidebarItem::Project { id: *id }));
        }
        let children = keyed
            .into_iter()
            .map(|(container, mut items)| {
                items.sort_by_key(|(key, _)| *key);
                (container, items.into_iter().map(|(_, item)| item).collect())
            })
            .collect();

        Ok(Tree {
            folders: rows.into_iter().map(|(f, _)| (f.id, f)).collect(),
            parent,
            children,
            project_folder,
            projects: projects.into_iter().map(|(id, _)| id).collect(),
        })
    }

    fn folder(&self, id: i64) -> Result<&FolderRow, LificError> {
        self.folders.get(&id).ok_or_else(|| not_found(id))
    }

    /// Levels above and including `folder`; the top level is 0.
    fn depth(&self, folder: Option<i64>) -> usize {
        let mut depth = 0;
        let mut cur = folder;
        while let Some(id) = cur {
            depth += 1;
            cur = self.parent.get(&id).copied().flatten();
        }
        depth
    }

    /// Levels `folder` occupies, itself included.
    fn height(&self, folder: i64) -> usize {
        1 + self
            .children
            .get(&Some(folder))
            .into_iter()
            .flatten()
            .filter_map(|item| match item {
                SidebarItem::Folder { id } => Some(self.height(*id)),
                SidebarItem::Project { .. } => None,
            })
            .max()
            .unwrap_or(0)
    }

    /// Whether `target` is `ancestor` or sits somewhere inside it.
    fn is_within(&self, target: Option<i64>, ancestor: i64) -> bool {
        let mut cur = target;
        while let Some(id) = cur {
            if id == ancestor {
                return true;
            }
            cur = self.parent.get(&id).copied().flatten();
        }
        false
    }

    fn container_of(&self, item: SidebarItem) -> Option<i64> {
        match item {
            SidebarItem::Folder { id } => self.parent.get(&id).copied().flatten(),
            SidebarItem::Project { id } => self.project_folder.get(&id).copied(),
        }
    }

    fn detach(&mut self, item: SidebarItem) -> (Option<i64>, usize) {
        let container = self.container_of(item);
        let list = self.children.entry(container).or_default();
        let index = list.iter().position(|i| *i == item).unwrap_or(list.len());
        if index < list.len() {
            list.remove(index);
        }
        (container, index)
    }

    fn insert_at(&mut self, container: Option<i64>, index: usize, item: SidebarItem) {
        let list = self.children.entry(container).or_default();
        list.insert(index.min(list.len()), item);
        match item {
            SidebarItem::Folder { id } => {
                self.parent.insert(id, container);
            }
            SidebarItem::Project { id } => match container {
                Some(folder) => {
                    self.project_folder.insert(id, folder);
                }
                None => {
                    self.project_folder.remove(&id);
                }
            },
        }
    }

    /// Depth-first walk: `(item, container, depth of container)`.
    fn walk(&self) -> Vec<(SidebarItem, Option<i64>, usize)> {
        let mut out = Vec::new();
        let mut stack: Vec<(SidebarItem, Option<i64>, usize)> = self
            .children
            .get(&None)
            .into_iter()
            .flatten()
            .rev()
            .map(|item| (*item, None, 0))
            .collect();
        let mut visited = HashSet::new();
        while let Some((item, container, depth)) = stack.pop() {
            out.push((item, container, depth));
            if let SidebarItem::Folder { id } = item
                && visited.insert(id)
            {
                for child in self.children.get(&Some(id)).into_iter().flatten().rev() {
                    stack.push((*child, Some(id), depth + 1));
                }
            }
        }
        out
    }

    /// Write the tree's order: depth-first ranks for every folder and visible
    /// project, and each folder's parent.
    fn persist(&self, conn: &Connection, user_id: i64) -> Result<(), LificError> {
        conn.execute(
            "DELETE FROM user_project_order WHERE user_id = ?1",
            [user_id],
        )?;
        for (rank, (item, container, _)) in self.walk().into_iter().enumerate() {
            match item {
                SidebarItem::Folder { id } => {
                    conn.execute(
                        "UPDATE project_groups SET sort_order = ?1, parent_id = ?2
                         WHERE id = ?3 AND user_id = ?4",
                        params![rank as i64, container, id, user_id],
                    )?;
                }
                SidebarItem::Project { id } => {
                    conn.execute(
                        "INSERT INTO user_project_order (user_id, project_id, sort_order)
                         VALUES (?1, ?2, ?3)",
                        params![user_id, id, rank as i64],
                    )?;
                }
            }
        }
        Ok(())
    }

    fn layout(&self) -> SidebarLayout {
        let groups = self
            .walk()
            .into_iter()
            .filter_map(|(item, container, depth)| {
                let SidebarItem::Folder { id } = item else {
                    return None;
                };
                let row = &self.folders[&id];
                let items: Vec<SidebarItem> =
                    self.children.get(&Some(id)).cloned().unwrap_or_default();
                Some(ProjectGroup {
                    id,
                    user_id: row.user_id,
                    name: row.name.clone(),
                    parent_id: container,
                    sort_order: row.sort_order,
                    project_ids: items
                        .iter()
                        .filter_map(|i| match i {
                            SidebarItem::Project { id } => Some(*id),
                            SidebarItem::Folder { .. } => None,
                        })
                        .collect(),
                    items,
                    depth: depth + 1,
                    created_at: row.created_at.clone(),
                    updated_at: row.updated_at.clone(),
                })
            })
            .collect();
        SidebarLayout {
            root: self.children.get(&None).cloned().unwrap_or_default(),
            groups,
            max_depth: MAX_DEPTH,
        }
    }

    /// Validate and apply one move, storage included except the ranks, which
    /// the caller writes once with [`Tree::persist`].
    fn apply_move(
        &mut self,
        conn: &Connection,
        user_id: i64,
        input: &MoveSidebarItem,
    ) -> Result<(), LificError> {
        let MoveSidebarItem {
            item,
            parent_id: target,
            before,
        } = *input;
        if let Some(p) = target {
            self.folder(p)?;
        }
        match item {
            SidebarItem::Folder { id } => {
                let name = self.folder(id)?.name.clone();
                if self.is_within(target, id) {
                    return Err(LificError::BadRequest(CYCLE.into()));
                }
                let levels = self.depth(target) + self.height(id);
                if levels > MAX_DEPTH {
                    return Err(LificError::BadRequest(format!(
                        "Folders can nest {MAX_DEPTH} levels deep. Moving '{name}' there would \
                         make {levels} levels."
                    )));
                }
            }
            SidebarItem::Project { id } => {
                if !self.projects.contains(&id) {
                    return Err(LificError::NotFound(format!("project {id} not found")));
                }
            }
        }
        if before == Some(item) {
            if self.container_of(item) == target {
                return Ok(());
            }
            return Err(LificError::BadRequest(
                "That spot is no longer in the folder. Refresh and try again.".into(),
            ));
        }

        let (from, _) = self.detach(item);
        let list = self.children.entry(target).or_default();
        let index = match before {
            None => list.len(),
            Some(anchor) => list.iter().position(|i| *i == anchor).ok_or_else(|| {
                LificError::BadRequest(
                    "That spot is no longer in the folder. Refresh and try again.".into(),
                )
            })?,
        };
        self.insert_at(target, index, item);

        if let SidebarItem::Project { id } = item
            && from != target
        {
            conn.execute(
                "DELETE FROM project_group_items
                 WHERE project_id = ?1
                   AND group_id IN (SELECT id FROM project_groups WHERE user_id = ?2)",
                params![id, user_id],
            )?;
            if let Some(folder) = target {
                conn.execute(
                    "INSERT INTO project_group_items (group_id, project_id) VALUES (?1, ?2)",
                    params![folder, id],
                )?;
            }
        }
        if let SidebarItem::Folder { id } = item
            && from != target
        {
            conn.execute(
                "UPDATE project_groups SET parent_id = ?1, updated_at = datetime('now')
                 WHERE id = ?2 AND user_id = ?3",
                params![target, id, user_id],
            )?;
        }
        Ok(())
    }
}

/// The caller's whole sidebar tree.
pub fn layout(
    conn: &Connection,
    user_id: i64,
    visible: &Visible,
) -> Result<SidebarLayout, LificError> {
    Ok(Tree::load(conn, user_id, visible)?.layout())
}

/// The caller's folders, depth-first, each with its ordered contents.
pub fn list_groups(
    conn: &Connection,
    user_id: i64,
    visible: &Visible,
) -> Result<Vec<ProjectGroup>, LificError> {
    Ok(layout(conn, user_id, visible)?.groups)
}

fn group_in(layout: SidebarLayout, id: i64) -> Result<ProjectGroup, LificError> {
    layout
        .groups
        .into_iter()
        .find(|g| g.id == id)
        .ok_or_else(|| not_found(id))
}

/// Move a project or folder: into `parent_id` (the top level when `None`),
/// directly before `before`, or last. Refuses cycles and anything that would
/// nest past [`MAX_DEPTH`], and changes nothing when it refuses.
pub fn move_item(
    conn: &Connection,
    user_id: i64,
    visible: &Visible,
    input: &MoveSidebarItem,
) -> Result<SidebarLayout, LificError> {
    super::savepoint(conn, "move_sidebar_item", || {
        let mut tree = Tree::load(conn, user_id, visible)?;
        tree.apply_move(conn, user_id, input)?;
        tree.persist(conn, user_id)?;
        layout(conn, user_id, visible)
    })
}

/// Reorder folders among their siblings: in each container, the listed
/// folders take the folder positions first, in list order, and omitted
/// folders follow in their current order. Projects stay where they are.
pub fn reorder_groups(
    conn: &Connection,
    user_id: i64,
    visible: &Visible,
    ids: &[i64],
) -> Result<Vec<ProjectGroup>, LificError> {
    super::savepoint(conn, "reorder_project_groups", || {
        let mut tree = Tree::load(conn, user_id, visible)?;
        let mut seen = HashMap::new();
        for (index, id) in ids.iter().enumerate() {
            if seen.insert(*id, index).is_some() {
                return Err(LificError::BadRequest(
                    "duplicate group id in reorder list".into(),
                ));
            }
            if !tree.folders.contains_key(id) {
                return Err(LificError::BadRequest(
                    "invalid group id in reorder list".into(),
                ));
            }
        }
        for list in tree.children.values_mut() {
            let slots: Vec<usize> = list
                .iter()
                .enumerate()
                .filter(|(_, i)| matches!(i, SidebarItem::Folder { .. }))
                .map(|(n, _)| n)
                .collect();
            let mut folders: Vec<(usize, SidebarItem)> = slots
                .iter()
                .enumerate()
                .map(|(current, slot)| {
                    let item = list[*slot];
                    let SidebarItem::Folder { id } = item else {
                        unreachable!()
                    };
                    (seen.get(&id).copied().unwrap_or(ids.len() + current), item)
                })
                .collect();
            folders.sort_by_key(|(key, _)| *key);
            for (slot, (_, item)) in slots.into_iter().zip(folders) {
                list[slot] = item;
            }
        }
        tree.persist(conn, user_id)?;
        list_groups(conn, user_id, visible)
    })
}

/// Create a folder in `parent_id` (the top level when `None`), after the
/// folders already there. With `contains`, that project or folder moves into
/// it in the same write, so a refused move leaves no empty folder behind.
pub fn create_group(
    conn: &Connection,
    user_id: i64,
    visible: &Visible,
    input: &CreateProjectGroup,
) -> Result<ProjectGroup, LificError> {
    validate_name(&input.name)?;
    let name = input.name.trim();
    super::savepoint(conn, "create_project_group", || {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM project_groups WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;
        if count >= MAX_FOLDERS {
            return Err(LificError::BadRequest(format!(
                "You can have at most {MAX_FOLDERS} folders. Delete one to make room."
            )));
        }
        let tree = Tree::load(conn, user_id, visible)?;
        if let Some(parent) = input.parent_id {
            let parent_name = tree.folder(parent)?.name.clone();
            if tree.depth(Some(parent)) + 1 > MAX_DEPTH {
                return Err(LificError::BadRequest(format!(
                    "Folders can nest {MAX_DEPTH} levels deep, and '{parent_name}' is already \
                     at level {MAX_DEPTH}."
                )));
            }
        }
        conn.execute(
            "INSERT INTO project_groups (user_id, name, parent_id, sort_order)
             VALUES (?1, ?2, ?3, 0)",
            params![user_id, name, input.parent_id],
        )
        .map_err(constraint_err(name))?;
        let id = conn.last_insert_rowid();

        let mut tree = Tree::load(conn, user_id, visible)?;
        let item = SidebarItem::Folder { id };
        let (container, _) = tree.detach(item);
        let after_folders = tree
            .children
            .get(&container)
            .and_then(|list| {
                list.iter()
                    .rposition(|i| matches!(i, SidebarItem::Folder { .. }))
            })
            .map_or(0, |n| n + 1);
        tree.insert_at(container, after_folders, item);
        if let Some(contains) = input.contains {
            tree.apply_move(
                conn,
                user_id,
                &MoveSidebarItem {
                    item: contains,
                    parent_id: Some(id),
                    before: None,
                },
            )?;
        }
        tree.persist(conn, user_id)?;
        group_in(layout(conn, user_id, visible)?, id)
    })
}

/// Rename a folder. Ownership is re-checked here, so callers may pass an id
/// straight from a request body.
pub fn update_group(
    conn: &Connection,
    id: i64,
    user_id: i64,
    visible: &Visible,
    input: &UpdateProjectGroup,
) -> Result<ProjectGroup, LificError> {
    Tree::load(conn, user_id, visible)?.folder(id)?;
    if let Some(name) = &input.name {
        validate_name(name)?;
        let name = name.trim();
        conn.execute(
            "UPDATE project_groups SET name = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND user_id = ?3",
            params![name, id, user_id],
        )
        .map_err(constraint_err(name))?;
    }
    group_in(layout(conn, user_id, visible)?, id)
}

/// Delete a folder. Its contents (subfolders and projects, including projects
/// the caller can no longer see) move to its parent, in its place. No project
/// is deleted or hidden.
pub fn delete_group(
    conn: &Connection,
    id: i64,
    user_id: i64,
    visible: &Visible,
) -> Result<bool, LificError> {
    super::savepoint(conn, "delete_project_group", || {
        let mut tree = Tree::load(conn, user_id, visible)?;
        tree.folder(id)?;
        let (parent, index) = tree.detach(SidebarItem::Folder { id });
        let contents = tree.children.remove(&Some(id)).unwrap_or_default();
        for (offset, child) in contents.into_iter().enumerate() {
            tree.insert_at(parent, index + offset, child);
        }
        conn.execute(
            "UPDATE project_groups SET parent_id = ?1 WHERE parent_id = ?2 AND user_id = ?3",
            params![parent, id, user_id],
        )?;
        if let Some(parent) = parent {
            conn.execute(
                "UPDATE OR IGNORE project_group_items SET group_id = ?1 WHERE group_id = ?2",
                params![parent, id],
            )?;
        }
        let changed = conn.execute(
            "DELETE FROM project_groups WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )?;
        tree.folders.remove(&id);
        tree.persist(conn, user_id)?;
        Ok(changed > 0)
    })
}

/// Move `project_id` to the end of `group_id`, or to the end of the top level
/// when `group_id` is `None`. The older, positionless form of [`move_item`].
pub fn assign_project(
    conn: &Connection,
    user_id: i64,
    visible: &Visible,
    project_id: i64,
    group_id: Option<i64>,
) -> Result<(), LificError> {
    move_item(
        conn,
        user_id,
        visible,
        &MoveSidebarItem {
            item: SidebarItem::Project { id: project_id },
            parent_id: group_id,
            before: None,
        },
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::CreateUser;
    use crate::db::{self, queries};

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_user(conn: &Connection, username: &str) -> i64 {
        queries::users::create_user(
            conn,
            &CreateUser {
                username: username.into(),
                email: format!("{username}@test.local"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap()
        .id
    }

    fn seed_project(conn: &Connection, ident: &str) -> i64 {
        queries::create_project(
            conn,
            &crate::db::models::CreateProject {
                conventions: None,
                name: format!("Project {ident}"),
                identifier: ident.into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap()
        .id
    }

    const ALL: Visible = None;

    fn create(
        conn: &Connection,
        user: i64,
        name: &str,
        parent: Option<i64>,
    ) -> Result<i64, LificError> {
        create_group(
            conn,
            user,
            &ALL,
            &CreateProjectGroup {
                name: name.into(),
                parent_id: parent,
                contains: None,
            },
        )
        .map(|g| g.id)
    }

    fn folder(conn: &Connection, user: i64, name: &str, parent: Option<i64>) -> i64 {
        create(conn, user, name, parent).unwrap()
    }

    fn mv(
        conn: &Connection,
        user: i64,
        item: SidebarItem,
        parent: Option<i64>,
        before: Option<SidebarItem>,
    ) -> Result<SidebarLayout, LificError> {
        move_item(
            conn,
            user,
            &ALL,
            &MoveSidebarItem {
                item,
                parent_id: parent,
                before,
            },
        )
    }

    fn f(id: i64) -> SidebarItem {
        SidebarItem::Folder { id }
    }
    fn p(id: i64) -> SidebarItem {
        SidebarItem::Project { id }
    }

    fn tree(conn: &Connection, user: i64) -> SidebarLayout {
        layout(conn, user, &ALL).unwrap()
    }

    fn items_of(layout: &SidebarLayout, id: i64) -> Vec<SidebarItem> {
        layout
            .groups
            .iter()
            .find(|g| g.id == id)
            .unwrap()
            .items
            .clone()
    }

    #[test]
    fn group_reorder_rolls_back_when_a_later_rank_update_fails() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = seed_user(&conn, "alice");
        let a = folder(&conn, user, "A", None);
        let b = folder(&conn, user, "B", None);
        conn.execute_batch(
            "CREATE TRIGGER reject_group_rank BEFORE UPDATE OF sort_order ON project_groups
            WHEN NEW.sort_order = 1 BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
        )
        .unwrap();
        assert!(reorder_groups(&conn, user, &ALL, &[b, a]).is_err());
        let groups = list_groups(&conn, user, &ALL).unwrap();
        assert_eq!(
            groups
                .iter()
                .map(|g| (g.id, g.sort_order))
                .collect::<Vec<_>>(),
            [(a, 0), (b, 1)]
        );
    }

    #[test]
    fn created_group_comes_back_empty_named_and_top_level() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let created = create_group(
            &conn,
            alice,
            &ALL,
            &CreateProjectGroup {
                name: "  Work ".into(),
                parent_id: None,
                contains: None,
            },
        )
        .unwrap();
        assert_eq!(created.name, "Work");
        assert_eq!(created.parent_id, None);
        assert_eq!(created.depth, 1);
        assert!(created.project_ids.is_empty() && created.items.is_empty());
        let listed = list_groups(&conn, alice, &ALL).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
    }

    #[test]
    fn duplicate_name_for_same_user_conflicts_but_another_user_may_reuse_it() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let work = folder(&conn, alice, "Work", None);
        let err = create(&conn, alice, "Work", None).unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");
        // Unique per user, not per parent: a subfolder may not reuse it either.
        let err = create(&conn, alice, "Work", Some(work)).unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");
        create(&conn, bob, "Work", None).expect("a different user may reuse the name");
    }

    #[test]
    fn one_users_groups_are_invisible_to_another() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        folder(&conn, alice, "Personal", None);
        assert!(list_groups(&conn, bob, &ALL).unwrap().is_empty());
    }

    #[test]
    fn blank_and_overlong_names_are_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let err = create(&conn, alice, "   ", None).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
        let long = "x".repeat(MAX_NAME_CHARS + 1);
        let err = create(&conn, alice, &long, None).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
        create(&conn, alice, &"x".repeat(MAX_NAME_CHARS), None).expect("at the limit");
    }

    #[test]
    fn folder_count_is_capped_with_a_clear_error() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        for n in 0..MAX_FOLDERS {
            conn.execute(
                "INSERT INTO project_groups (user_id, name) VALUES (?1, ?2)",
                params![alice, format!("F{n}")],
            )
            .unwrap();
        }
        let LificError::BadRequest(message) = create(&conn, alice, "One more", None).unwrap_err()
        else {
            panic!("expected BadRequest");
        };
        assert!(message.contains("at most 200 folders"), "{message}");
    }

    // ── nesting, cycles, depth ──────────────────────────────────

    #[test]
    fn folders_nest_and_report_parent_and_depth() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let work = folder(&conn, alice, "Work", None);
        let clients = folder(&conn, alice, "Clients", Some(work));
        let acme = folder(&conn, alice, "Acme", Some(clients));
        let layout = tree(&conn, alice);
        assert_eq!(layout.root, [f(work)]);
        assert_eq!(items_of(&layout, work), [f(clients)]);
        assert_eq!(items_of(&layout, clients), [f(acme)]);
        let depths: Vec<_> = layout
            .groups
            .iter()
            .map(|g| (g.id, g.parent_id, g.depth))
            .collect();
        assert_eq!(
            depths,
            [
                (work, None, 1),
                (clients, Some(work), 2),
                (acme, Some(clients), 3)
            ]
        );
        assert_eq!(layout.max_depth, MAX_DEPTH);
    }

    #[test]
    fn creating_past_the_depth_cap_is_refused_and_level_four_is_allowed() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let l1 = folder(&conn, alice, "L1", None);
        let l2 = folder(&conn, alice, "L2", Some(l1));
        let l3 = folder(&conn, alice, "L3", Some(l2));
        let l4 = folder(&conn, alice, "L4", Some(l3));
        let LificError::BadRequest(message) = create(&conn, alice, "L5", Some(l4)).unwrap_err()
        else {
            panic!("expected BadRequest");
        };
        assert!(message.contains("4 levels deep"), "{message}");
        assert_eq!(list_groups(&conn, alice, &ALL).unwrap().len(), 4);
    }

    #[test]
    fn moving_a_subtree_past_the_depth_cap_is_refused_without_changes() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let a1 = folder(&conn, alice, "A1", None);
        let a2 = folder(&conn, alice, "A2", Some(a1));
        let a3 = folder(&conn, alice, "A3", Some(a2));
        let b1 = folder(&conn, alice, "B1", None);
        folder(&conn, alice, "B2", Some(b1));
        let before = tree(&conn, alice);
        // B1 is two levels tall; under A3 (level 3) it would reach level 5.
        let LificError::BadRequest(message) = mv(&conn, alice, f(b1), Some(a3), None).unwrap_err()
        else {
            panic!("expected BadRequest");
        };
        assert!(message.contains("would make 5 levels"), "{message}");
        assert_eq!(tree(&conn, alice), before);
        // Under A2 it reaches exactly level 4.
        let after = mv(&conn, alice, f(b1), Some(a2), None).unwrap();
        assert_eq!(items_of(&after, a2), [f(a3), f(b1)]);
        assert_eq!(after.groups.iter().map(|g| g.depth).max(), Some(4));
    }

    #[test]
    fn a_folder_cannot_move_into_itself_or_a_descendant() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let a = folder(&conn, alice, "A", None);
        let b = folder(&conn, alice, "B", Some(a));
        let c = folder(&conn, alice, "C", Some(b));
        let before = tree(&conn, alice);
        for target in [a, b, c] {
            let err = mv(&conn, alice, f(a), Some(target), None).unwrap_err();
            assert!(
                matches!(&err, LificError::BadRequest(m) if m == CYCLE),
                "got {err:?}"
            );
        }
        assert_eq!(tree(&conn, alice), before);
        // Out to the top level is fine.
        let after = mv(&conn, alice, f(c), None, None).unwrap();
        assert_eq!(after.root, [f(a), f(c)]);
    }

    #[test]
    fn a_hand_written_cycle_still_renders_every_folder() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let a = folder(&conn, alice, "A", None);
        let b = folder(&conn, alice, "B", Some(a));
        conn.execute(
            "UPDATE project_groups SET parent_id = ?1 WHERE id = ?2",
            params![b, a],
        )
        .unwrap();
        let layout = tree(&conn, alice);
        assert_eq!(layout.groups.len(), 2);
        assert_eq!(layout.root.len(), 1);
        // The next write stores the repaired shape.
        mv(&conn, alice, layout.root[0], None, None).unwrap();
        let parents: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_groups WHERE parent_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(parents, 1);
    }

    #[test]
    fn moving_into_another_users_folder_or_an_unknown_one_is_not_found() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let project = seed_project(&conn, "APP");
        let alice_work = folder(&conn, alice, "Work", None);
        let bob_side = folder(&conn, bob, "Side", None);
        for (item, parent) in [
            (p(project), Some(alice_work)),
            (f(alice_work), None),
            (f(bob_side), Some(99999)),
        ] {
            let err = mv(&conn, bob, item, parent, None).unwrap_err();
            assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
        }
    }

    // ── ordering ────────────────────────────────────────────────

    #[test]
    fn projects_and_folders_share_one_order_per_container() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let one = seed_project(&conn, "ONE");
        let two = seed_project(&conn, "TWO");
        let three = seed_project(&conn, "THREE");
        let work = folder(&conn, alice, "Work", None);
        // New folders sit above loose projects.
        assert_eq!(tree(&conn, alice).root, [f(work), p(one), p(two), p(three)]);
        // A project before the folder, then the folder after the last project.
        assert_eq!(
            mv(&conn, alice, p(three), None, Some(f(work)))
                .unwrap()
                .root,
            [p(three), f(work), p(one), p(two)]
        );
        assert_eq!(
            mv(&conn, alice, f(work), None, None).unwrap().root,
            [p(three), p(one), p(two), f(work)]
        );
        // Into the folder, reordered inside it, and out again.
        mv(&conn, alice, p(one), Some(work), None).unwrap();
        mv(&conn, alice, p(two), Some(work), None).unwrap();
        let layout = mv(&conn, alice, p(two), Some(work), Some(p(one))).unwrap();
        assert_eq!(items_of(&layout, work), [p(two), p(one)]);
        assert_eq!(layout.root, [p(three), f(work)]);
        let layout = mv(&conn, alice, p(one), None, Some(p(three))).unwrap();
        assert_eq!(layout.root, [p(one), p(three), f(work)]);
        assert_eq!(items_of(&layout, work), [p(two)]);
        // A later folder created alongside lands after the existing folders.
        let later = folder(&conn, alice, "Later", None);
        assert_eq!(
            tree(&conn, alice).root,
            [p(one), p(three), f(work), f(later)]
        );
        // The flat personal project order follows the tree depth-first.
        let flat: Vec<i64> = queries::list_projects_for_user(&conn, alice)
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(flat, [one, three, two]);
    }

    #[test]
    fn a_stale_drop_anchor_is_refused_and_dropping_on_itself_is_a_no_op() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let one = seed_project(&conn, "ONE");
        let two = seed_project(&conn, "TWO");
        let work = folder(&conn, alice, "Work", None);
        let before = tree(&conn, alice);
        let err = mv(&conn, alice, p(one), Some(work), Some(p(two))).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
        assert_eq!(tree(&conn, alice), before);
        assert_eq!(
            mv(&conn, alice, p(one), None, Some(p(one))).unwrap().root,
            before.root
        );
        let err = mv(&conn, alice, p(one), Some(work), Some(p(one))).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
    }

    #[test]
    fn reorder_moves_folders_among_their_siblings_only() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let one = seed_project(&conn, "ONE");
        let a = folder(&conn, alice, "A", None);
        let b = folder(&conn, alice, "B", None);
        let inner1 = folder(&conn, alice, "I1", Some(a));
        let inner2 = folder(&conn, alice, "I2", Some(a));
        mv(&conn, alice, p(one), None, Some(f(b))).unwrap();
        reorder_groups(&conn, alice, &ALL, &[inner2, b]).unwrap();
        let layout = tree(&conn, alice);
        assert_eq!(layout.root, [f(b), p(one), f(a)]);
        assert_eq!(items_of(&layout, a), [f(inner2), f(inner1)]);
    }

    #[test]
    fn one_users_moves_never_change_another_users_order() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let one = seed_project(&conn, "ONE");
        let two = seed_project(&conn, "TWO");
        let bobs = folder(&conn, bob, "Mine", None);
        mv(&conn, bob, p(two), None, Some(p(one))).unwrap();
        let bob_before = tree(&conn, bob);
        let legacy: Vec<i64> = queries::list_projects(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.sort_order)
            .collect();
        let work = folder(&conn, alice, "Work", None);
        mv(&conn, alice, p(one), Some(work), None).unwrap();
        mv(&conn, alice, f(work), None, None).unwrap();
        assert_eq!(tree(&conn, bob), bob_before);
        assert_eq!(bob_before.root, [f(bobs), p(two), p(one)]);
        let after: Vec<i64> = queries::list_projects(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.sort_order)
            .collect();
        assert_eq!(after, legacy, "the shared project order is never written");
    }

    // ── assignment ──────────────────────────────────────────────

    #[test]
    fn assigning_a_project_twice_moves_it_instead_of_duplicating_it() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let work = folder(&conn, alice, "Work", None);
        let personal = folder(&conn, alice, "Personal", None);
        assign_project(&conn, alice, &ALL, project, Some(work)).unwrap();
        assign_project(&conn, alice, &ALL, project, Some(personal)).unwrap();
        let layout = tree(&conn, alice);
        assert!(items_of(&layout, work).is_empty());
        assert_eq!(items_of(&layout, personal), [p(project)]);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_group_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn assigning_none_ungroups_the_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let work = folder(&conn, alice, "Work", None);
        assign_project(&conn, alice, &ALL, project, Some(work)).unwrap();
        assign_project(&conn, alice, &ALL, project, None).unwrap();
        let layout = tree(&conn, alice);
        assert!(layout.groups[0].project_ids.is_empty());
        assert_eq!(layout.root, [f(work), p(project)]);
    }

    #[test]
    fn two_users_may_file_the_same_project_differently() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let project = seed_project(&conn, "APP");
        let alice_work = folder(&conn, alice, "Work", None);
        let bob_side = folder(&conn, bob, "Side", None);
        let bob_inner = folder(&conn, bob, "Inner", Some(bob_side));
        assign_project(&conn, alice, &ALL, project, Some(alice_work)).unwrap();
        assign_project(&conn, bob, &ALL, project, Some(bob_inner)).unwrap();
        assert_eq!(items_of(&tree(&conn, alice), alice_work), [p(project)]);
        assert_eq!(items_of(&tree(&conn, bob), bob_inner), [p(project)]);
    }

    #[test]
    fn a_project_the_caller_cannot_see_cannot_be_moved() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let work = folder(&conn, alice, "Work", None);
        let nothing = Some(HashSet::new());
        let err = move_item(
            &conn,
            alice,
            &nothing,
            &MoveSidebarItem {
                item: p(project),
                parent_id: Some(work),
                before: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
    }

    // ── create with contents ────────────────────────────────────

    #[test]
    fn creating_a_folder_around_an_item_is_one_write() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let created = create_group(
            &conn,
            alice,
            &ALL,
            &CreateProjectGroup {
                name: "Wrapped".into(),
                parent_id: None,
                contains: Some(p(project)),
            },
        )
        .unwrap();
        assert_eq!(created.items, [p(project)]);
        // A refused move rolls the new folder back too.
        let l1 = folder(&conn, alice, "L1", None);
        let l2 = folder(&conn, alice, "L2", Some(l1));
        let l3 = folder(&conn, alice, "L3", Some(l2));
        let before = tree(&conn, alice);
        let err = create_group(
            &conn,
            alice,
            &ALL,
            &CreateProjectGroup {
                name: "Too deep".into(),
                parent_id: Some(l3),
                contains: Some(f(created.id)),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
        assert_eq!(tree(&conn, alice), before);
        let err = create_group(
            &conn,
            alice,
            &ALL,
            &CreateProjectGroup {
                name: "Inside itself".into(),
                parent_id: Some(l2),
                contains: Some(f(l1)),
            },
        )
        .unwrap_err();
        assert!(
            matches!(&err, LificError::BadRequest(m) if m == CYCLE),
            "got {err:?}"
        );
        assert_eq!(tree(&conn, alice), before);
    }

    // ── delete and cascades ─────────────────────────────────────

    #[test]
    fn deleting_a_folder_moves_its_contents_into_its_place() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let one = seed_project(&conn, "ONE");
        let two = seed_project(&conn, "TWO");
        let three = seed_project(&conn, "THREE");
        let outer = folder(&conn, alice, "Outer", None);
        let middle = folder(&conn, alice, "Middle", Some(outer));
        let inner = folder(&conn, alice, "Inner", Some(middle));
        mv(&conn, alice, p(one), Some(outer), None).unwrap();
        mv(&conn, alice, p(two), Some(middle), None).unwrap();
        mv(&conn, alice, p(three), Some(middle), Some(f(inner))).unwrap();
        assert_eq!(items_of(&tree(&conn, alice), outer), [f(middle), p(one)]);

        assert!(delete_group(&conn, middle, alice, &ALL).unwrap());
        let layout = tree(&conn, alice);
        assert_eq!(
            items_of(&layout, outer),
            [p(three), f(inner), p(two), p(one)]
        );
        assert_eq!(
            layout.groups.iter().find(|g| g.id == inner).unwrap().depth,
            2
        );
        for project in [one, two, three] {
            queries::get_project(&conn, project).expect("projects survive");
        }

        assert!(delete_group(&conn, outer, alice, &ALL).unwrap());
        let layout = tree(&conn, alice);
        assert_eq!(layout.root, [p(three), f(inner), p(two), p(one)]);
        let memberships: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_group_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(memberships, 0);
    }

    #[test]
    fn deleting_a_folder_keeps_a_hidden_projects_filing_with_the_parent() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let secret = seed_project(&conn, "SEC");
        let outer = folder(&conn, alice, "Outer", None);
        let inner = folder(&conn, alice, "Inner", Some(outer));
        mv(&conn, alice, p(secret), Some(inner), None).unwrap();
        let hidden = Some(HashSet::new());
        assert!(
            layout(&conn, alice, &hidden).unwrap().groups[1]
                .items
                .is_empty()
        );
        delete_group(&conn, inner, alice, &hidden).unwrap();
        // Access comes back: the project is in the folder that took over.
        assert_eq!(items_of(&tree(&conn, alice), outer), [p(secret)]);
    }

    #[test]
    fn deleting_another_users_group_is_not_found() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let work = folder(&conn, alice, "Work", None);
        let err = delete_group(&conn, work, bob, &ALL).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
        assert_eq!(list_groups(&conn, alice, &ALL).unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_project_drops_its_memberships() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let work = folder(&conn, alice, "Work", None);
        assign_project(&conn, alice, &ALL, project, Some(work)).unwrap();
        queries::delete_project(&conn, project).unwrap();
        assert!(
            list_groups(&conn, alice, &ALL).unwrap()[0]
                .project_ids
                .is_empty()
        );
    }

    #[test]
    fn deleting_a_user_cascades_their_groups_away() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let work = folder(&conn, alice, "Work", None);
        folder(&conn, alice, "Inner", Some(work));
        conn.execute("DELETE FROM users WHERE id = ?1", params![alice])
            .unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_groups", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    // ── rename ──────────────────────────────────────────────────

    #[test]
    fn renaming_onto_an_existing_name_conflicts() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        folder(&conn, alice, "Work", None);
        let personal = folder(&conn, alice, "Personal", None);
        let err = update_group(
            &conn,
            personal,
            alice,
            &ALL,
            &UpdateProjectGroup {
                name: Some("Work".into()),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");
    }

    #[test]
    fn renaming_keeps_the_groups_place_and_contents() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let work = folder(&conn, alice, "Work", None);
        let inner = folder(&conn, alice, "Inner", Some(work));
        assign_project(&conn, alice, &ALL, project, Some(inner)).unwrap();
        let renamed = update_group(
            &conn,
            inner,
            alice,
            &ALL,
            &UpdateProjectGroup {
                name: Some("Day job".into()),
            },
        )
        .unwrap();
        assert_eq!(renamed.name, "Day job");
        assert_eq!(renamed.parent_id, Some(work));
        assert_eq!(renamed.items, [p(project)]);
    }
}
