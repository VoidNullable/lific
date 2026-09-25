//! LIF-486: comment kinds survive a project archive, and format 1 archives,
//! which predate them, still import as ordinary comments.

use super::tests::{fixture, seed, write_manifest};
use super::*;

fn issue_comment_kind(pool: &DbPool) -> String {
    let conn = pool.read().unwrap();
    conn.query_row(
        "SELECT c.kind FROM comments c
           JOIN issues i ON i.id = c.issue_id
           JOIN projects p ON p.id = i.project_id
          WHERE p.identifier = 'LIF' AND i.sequence = 42 AND c.deleted_at IS NULL",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn project_archive_round_trips_a_verification_comment() {
    let (source, dir, store) = fixture();
    seed(&source, &store);
    source
        .write()
        .unwrap()
        .execute(
            "UPDATE comments SET kind = 'verification' WHERE id = 70",
            [],
        )
        .unwrap();
    let archive = dir.path().join("kinds.tar.gz");
    export(&source, &store, "LIF", &archive).unwrap();

    let (dest, _dest_dir, dest_store) = fixture();
    import(&dest, &dest_store, &archive, "owner").unwrap();
    assert_eq!(issue_comment_kind(&dest), "verification");
    let page_kind: String = dest
        .read()
        .unwrap()
        .query_row(
            "SELECT kind FROM comments WHERE page_id IS NOT NULL AND deleted_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(page_kind, "comment");
}

#[test]
fn project_archive_imports_format_1_comments_as_ordinary_comments() {
    let (source, dir, store) = fixture();
    seed(&source, &store);
    let mut manifest = {
        let conn = source.read().unwrap();
        collect_manifest(&conn, "LIF").unwrap()
    };
    // Rebuild the manifest a format 1 exporter would have written: no kind.
    manifest.format_version = 1;
    for table in manifest.tables.iter_mut().filter(|t| t.name == "comments") {
        for row in &mut table.rows {
            row.pop();
        }
    }
    let blobs: Vec<(String, Vec<u8>)> = manifest
        .blobs
        .iter()
        .map(|blob| {
            (
                format!("blobs/{}", blob.sha256),
                store.read(&blob.sha256).unwrap(),
            )
        })
        .collect();
    let blob_refs: Vec<(&str, &[u8])> = blobs
        .iter()
        .map(|(hash, bytes)| (hash.as_str(), bytes.as_slice()))
        .collect();
    let archive = dir.path().join("format-1.tar.gz");
    write_manifest(&archive, &manifest, &blob_refs);

    let (dest, _dest_dir, dest_store) = fixture();
    import(&dest, &dest_store, &archive, "owner").unwrap();
    assert_eq!(issue_comment_kind(&dest), "comment");
}

#[test]
fn project_archive_rejects_an_unknown_comment_kind() {
    let (source, _dir, store) = fixture();
    seed(&source, &store);
    let mut manifest = {
        let conn = source.read().unwrap();
        collect_manifest(&conn, "LIF").unwrap()
    };
    let s = spec("comments").unwrap();
    let table = manifest
        .tables
        .iter_mut()
        .find(|t| t.name == "comments")
        .unwrap();
    s.set(&mut table.rows[0], "kind", "note".into());
    assert!(
        validate_manifest(&manifest)
            .unwrap_err()
            .to_string()
            .contains("invalid comment kind")
    );
}
