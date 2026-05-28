-- LIF-105: labels on pages.
--
-- Pages join to project-scoped labels via page_labels, exactly mirroring
-- issue_labels. labels.project_id stays NOT NULL — workspace-level pages
-- (no project) intentionally have no label affordance for now; allowing
-- workspace-scoped labels is deferred (the original design discussion in
-- LIF-105 covers the tradeoffs).
--
-- ON DELETE CASCADE on both FKs so dropping a page or a label
-- automatically prunes the join row without any application logic.

CREATE TABLE IF NOT EXISTS page_labels (
    page_id   INTEGER NOT NULL REFERENCES pages(id)  ON DELETE CASCADE,
    label_id  INTEGER NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
    PRIMARY KEY (page_id, label_id)
);

CREATE INDEX IF NOT EXISTS idx_page_labels_page  ON page_labels(page_id);
CREATE INDEX IF NOT EXISTS idx_page_labels_label ON page_labels(label_id);
