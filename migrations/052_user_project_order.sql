-- Sidebar order is personal. Until the first reorder, read legacy project
-- ranks directly instead of copying every project for every user at upgrade.
CREATE TABLE user_project_order (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    sort_order INTEGER NOT NULL,
    PRIMARY KEY (user_id, project_id)
);

CREATE INDEX idx_user_project_order_project ON user_project_order(project_id);
