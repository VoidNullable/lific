use super::*;
use crate::db::{models::*, queries};

fn seed_numbers(pool: &DbPool) {
    let conn = pool.write().unwrap();
    conn.execute_batch(
        "INSERT INTO projects(id, name, identifier) VALUES (10, 'Numbers', 'NUM');
         INSERT INTO issues(project_id, sequence, title) VALUES (10, 1, 'One'), (10, 2, 'Two'), (10, 3, 'Three');
         INSERT INTO pages(project_id, sequence, title) VALUES (10, 1, 'One'), (10, 2, 'Two'), (10, 3, 'Three');
         INSERT INTO plans(project_id, sequence, title) VALUES (10, 1, 'One'), (10, 2, 'Two'), (10, 3, 'Three');",
    ).unwrap();
}

fn remove_highest(conn: &Connection, project: i64) {
    for table in ["issues", "pages"] {
        conn.execute(
            &format!("UPDATE {table} SET deleted_at = '2000-01-01' WHERE project_id = ?1 AND sequence = 3"),
            [project],
        ).unwrap();
    }
    conn.execute(
        "DELETE FROM plans WHERE project_id = ?1 AND sequence = 3",
        [project],
    )
    .unwrap();
    queries::trash::purge_tombstones(conn, 30).unwrap();
}

fn assert_next_numbers(conn: &Connection, expected: i64) {
    let project = resolve_project(conn, "NUM").unwrap();
    assert_eq!(
        queries::create_issue(
            conn,
            &CreateIssue {
                project_id: project,
                title: "Next".into(),
                ..Default::default()
            }
        )
        .unwrap()
        .sequence,
        expected
    );
    assert_eq!(
        queries::create_page(
            conn,
            &CreatePage {
                project_id: Some(project),
                title: "Next".into(),
                ..Default::default()
            }
        )
        .unwrap()
        .sequence,
        Some(expected)
    );
    assert_eq!(
        queries::plans::create_plan(
            conn,
            &CreatePlan {
                project_id: project,
                title: "Next".into(),
                issue_id: None,
                steps: Vec::new()
            }
        )
        .unwrap()
        .sequence,
        expected
    );
    assert_eq!(
        queries::create_page(
            conn,
            &CreatePage {
                title: "Workspace".into(),
                ..Default::default()
            }
        )
        .unwrap()
        .sequence,
        Some(1)
    );
}

#[test]
fn project_archive_preserves_marks_above_remaining_rows() {
    let (source, dir, store) = tests::fixture();
    seed_numbers(&source);
    {
        let conn = source.write().unwrap();
        remove_highest(&conn, 10);
    }
    let archive = dir.path().join("numbers.tar.gz");
    export(&source, &store, "NUM", &archive).unwrap();
    let (dest, _dir, dest_store) = tests::fixture();
    {
        let conn = dest.write().unwrap();
        conn.execute(
            "INSERT INTO projects(id, name, identifier) VALUES (99, 'Other', 'OTH')",
            [],
        )
        .unwrap();
    }
    import(&dest, &dest_store, &archive, "owner").unwrap();
    let conn = dest.write().unwrap();
    assert_ne!(resolve_project(&conn, "NUM").unwrap(), 10);
    assert_next_numbers(&conn, 4);
}

#[test]
fn legacy_project_archive_seeds_marks_before_imported_rows_are_purged() {
    let (source, dir, _store) = tests::fixture();
    seed_numbers(&source);
    for version in [1, 2] {
        let mut manifest = collect_manifest(&source.read().unwrap(), "NUM").unwrap();
        manifest.format_version = version;
        manifest.number_high_water = None;
        let archive = dir.path().join(format!("legacy-{version}.tar.gz"));
        tests::write_manifest(&archive, &manifest, &[]);
        let (dest, _dir, dest_store) = tests::fixture();
        import(&dest, &dest_store, &archive, "owner").unwrap();
        let conn = dest.write().unwrap();
        let project = resolve_project(&conn, "NUM").unwrap();
        remove_highest(&conn, project);
        assert_next_numbers(&conn, 4);
    }
}

#[test]
fn project_archive_marks_cannot_lower_imported_row_maxima() {
    let (source, dir, _store) = tests::fixture();
    seed_numbers(&source);
    let mut manifest = collect_manifest(&source.read().unwrap(), "NUM").unwrap();
    manifest.number_high_water = Some(NumberHighWater::default());
    let archive = dir.path().join("low-marks.tar.gz");
    tests::write_manifest(&archive, &manifest, &[]);
    let (dest, _dir, dest_store) = tests::fixture();
    import(&dest, &dest_store, &archive, "owner").unwrap();
    assert_next_numbers(&dest.write().unwrap(), 4);
}

#[test]
fn project_archive_rejects_missing_or_negative_current_marks() {
    let (source, dir, _store) = tests::fixture();
    seed_numbers(&source);
    let mut manifest = collect_manifest(&source.read().unwrap(), "NUM").unwrap();
    manifest.number_high_water = None;
    let archive = dir.path().join("invalid-marks.tar.gz");
    tests::write_manifest(&archive, &manifest, &[]);
    assert!(
        stage(&archive)
            .err()
            .unwrap()
            .to_string()
            .contains("missing number high-water")
    );
    for marks in [
        NumberHighWater {
            issues: -1,
            ..Default::default()
        },
        NumberHighWater {
            pages: -1,
            ..Default::default()
        },
        NumberHighWater {
            plans: -1,
            ..Default::default()
        },
    ] {
        manifest.number_high_water = Some(marks);
        tests::write_manifest(&archive, &manifest, &[]);
        assert!(
            stage(&archive)
                .err()
                .unwrap()
                .to_string()
                .contains("negative number high-water")
        );
    }
}

#[test]
fn project_archive_bounds_each_number_mark_with_room_for_future_allocations() {
    let (source, _dir, _store) = tests::fixture();
    seed_numbers(&source);
    let mut manifest = collect_manifest(&source.read().unwrap(), "NUM").unwrap();
    manifest.number_high_water = Some(NumberHighWater {
        issues: MAX_NUMBER_HIGH_WATER,
        pages: MAX_NUMBER_HIGH_WATER,
        plans: MAX_NUMBER_HIGH_WATER,
    });
    validate_manifest(&manifest).unwrap();
    for value in [MAX_NUMBER_HIGH_WATER + 1, i64::MAX] {
        for (entity, marks) in [
            (
                "issue",
                NumberHighWater {
                    issues: value,
                    ..Default::default()
                },
            ),
            (
                "page",
                NumberHighWater {
                    pages: value,
                    ..Default::default()
                },
            ),
            (
                "plan",
                NumberHighWater {
                    plans: value,
                    ..Default::default()
                },
            ),
        ] {
            manifest.number_high_water = Some(marks);
            assert!(matches!(
                validate_manifest(&manifest),
                Err(LificError::BadRequest(message))
                    if message == format!(
                        "project archive: {entity} number high-water mark exceeds {MAX_NUMBER_HIGH_WATER}"
                    )
            ));
        }
    }
}
