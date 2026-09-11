use super::*;
use crate::db::queries;

fn fixture() -> (DbPool, tempfile::TempDir, AttachmentStore) {
    let pool = db::open_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = AttachmentStore::new(dir.path().to_path_buf());
    {
        let conn = pool.write().unwrap();
        conn.execute_batch("INSERT INTO users(id,username,email,password_hash,is_admin) VALUES (1,'owner','owner@example.test','not-a-hash',1);").unwrap();
    }
    (pool, dir, store)
}

fn seed(pool: &DbPool, store: &AttachmentStore) {
    let hash = store.write(b"project archive attachment").unwrap();
    let conn = pool.write().unwrap();
    conn.execute_batch("INSERT INTO users(id,username,email,password_hash) VALUES(2,'author','secret@example.test','PRIVATE PASSWORD HASH');
        UPDATE _actor_state SET user_id=2,transport='api';
        INSERT INTO projects(id,name,identifier) VALUES(10,'Portable','LIF'),(20,'PRIVATE NEIGHBOR','SEC');
        INSERT INTO modules(id,project_id,name) VALUES(3,10,'Core');
        INSERT INTO labels(id,project_id,name) VALUES(4,10,'Bug');
        INSERT INTO folders(id,project_id,name) VALUES(5,10,'Docs');
        INSERT INTO folders(id,project_id,parent_id,name) VALUES(6,10,5,'Nested');
        INSERT INTO issues(id,project_id,sequence,title,module_id,source) VALUES(30,10,42,'Portable issue',3,'github:123'),(31,10,43,'Deleted issue',3,NULL),(32,20,1,'PRIVATE ISSUE',NULL,NULL);
        INSERT INTO pages(id,project_id,sequence,folder_id,title,content,status,pinned) VALUES(40,10,7,6,'Guide','Historical text','active',1),(41,10,8,6,'Deleted page','Deleted page body','archived',0);
        UPDATE pages SET content='Current text' WHERE id=40;
        INSERT INTO plans(id,project_id,sequence,issue_id,title) VALUES(50,10,9,30,'Plan');
        INSERT INTO plan_steps(id,plan_id,title,issue_id) VALUES(60,50,'Parent',30);
        INSERT INTO plan_steps(id,plan_id,parent_step_id,title,done) VALUES(61,50,60,'Child',1);
        INSERT INTO comments(id,issue_id,user_id,content) VALUES(70,30,2,'Authored comment'),(71,31,2,'Deleted parent comment');
        INSERT INTO comments(id,page_id,user_id,content) VALUES(72,40,2,'Page comment'),(73,41,2,'Deleted page comment');
        INSERT INTO issue_labels VALUES(30,4);
        INSERT INTO page_labels VALUES(40,4);
        INSERT INTO issue_relations VALUES(30,31,'relates_to'),(30,32,'blocks');
        INSERT INTO page_issue_links VALUES(40,30);
        UPDATE issues SET status='done' WHERE id=30;
        UPDATE issues SET deleted_at='2025-02-03 00:00:00' WHERE id=31;
        UPDATE pages SET deleted_at='2025-02-03 00:00:00' WHERE id=41;").unwrap();
    conn.execute("INSERT INTO attachments(id,sha256,filename,mime,size_bytes,uploader_id) VALUES(80,?1,'note.txt','text/plain',26,2)", [&hash]).unwrap();
    conn.execute_batch(
        "INSERT INTO attachment_links VALUES(80,'issue',30,'2025-01-01 00:00:00');
        INSERT INTO attachment_links VALUES(80,'comment',71,'2025-01-01 00:00:00');
        INSERT INTO attachment_links VALUES(80,'comment',70,'2025-01-01 00:00:00');
        UPDATE issues SET description='[note](/api/attachments/80)' WHERE id=30;
        UPDATE comments SET content='[note](/api/attachments/80)' WHERE id=70;",
    )
    .unwrap();
    // Store historical timestamps without replaying timestamp triggers.
    let tx = conn.unchecked_transaction().unwrap();
    let triggers = db::migrate::suspend_triggers(&tx).unwrap();
    tx.execute_batch("UPDATE issues SET created_at='2020-01-01 00:00:00',updated_at='2021-01-01 00:00:00' WHERE project_id=10;
        UPDATE comments SET created_at='2020-01-02 00:00:00',updated_at='2021-01-02 00:00:00';").unwrap();
    for sql in triggers {
        tx.execute_batch(&sql).unwrap();
    }
    tx.commit().unwrap();
}

fn write_manifest(path: &Path, manifest: &Manifest, blobs: &[(&str, &[u8])]) {
    let gzip =
        flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gzip);
    append(
        &mut tar,
        "manifest.json",
        &serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
    for (name, bytes) in blobs {
        append(&mut tar, name, bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
}

#[test]
fn project_archive_roundtrip_preserves_graph_without_neighbor_secrets_or_accounts() {
    let (source, dir, store) = fixture();
    seed(&source, &store);
    let archive = dir.path().join("project.tar.gz");
    let before = {
        let c = source.read().unwrap();
        collect_manifest(&c, "LIF").unwrap()
    };
    export(&source, &store, "LIF", &archive).unwrap();
    let staged = stage(&archive).unwrap();
    let json = serde_json::to_string(&staged.manifest).unwrap();
    assert!(!json.contains("PRIVATE NEIGHBOR"));
    assert!(!json.contains("PRIVATE ISSUE"));
    assert!(!json.contains("PRIVATE PASSWORD HASH"));
    assert!(!json.contains("secret@example.test"));
    let (dest, _destdir, deststore) = fixture();
    {
        let c = dest.write().unwrap();
        c.execute_batch("INSERT INTO projects(id,name,identifier) VALUES(100,'Existing','EX'); INSERT INTO issues(id,project_id,sequence,title) VALUES(100,100,1,'Existing issue');").unwrap();
    }
    let expected_maps = {
        let c = dest.read().unwrap();
        allocate_maps(&c, &before).unwrap()
    };
    let result = import(&dest, &deststore, &archive, "owner").unwrap();
    assert_eq!(result.project, "LIF");
    assert!(!result.external_references.is_empty());
    let c = dest.read().unwrap();
    let actual = collect_manifest(&c, "LIF").unwrap();
    for table in &before.tables {
        let s = spec(&table.name).unwrap();
        let expected = table
            .rows
            .iter()
            .map(|r| {
                imported_row(s, r, &expected_maps, &mut RewriteState::new(&[]).unwrap()).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual.rows(s.name),
            expected,
            "every preserved column of {}",
            s.name
        );
    }
    assert_eq!(
        c.query_row("SELECT count(*) FROM users", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let project: i64 = c
        .query_row("SELECT id FROM projects WHERE identifier='LIF'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(project > 100);
    let issue:(i64,String,String,String)=c.query_row("SELECT id,description,created_at,updated_at FROM issues WHERE project_id=?1 AND sequence=42",[project],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert!(issue.0 > 100);
    assert_eq!(issue.2, "2020-01-01 00:00:00");
    assert_eq!(issue.3, "2021-01-01 00:00:00");
    let att: i64 = c
        .query_row("SELECT id FROM attachments", [], |r| r.get(0))
        .unwrap();
    assert_eq!(issue.1, format!("[note](/api/attachments/{att})"));
    let comment: i64 = c
        .query_row(
            "SELECT id FROM comments WHERE issue_id=?1 AND deleted_at IS NULL",
            [issue.0],
            |r| r.get(0),
        )
        .unwrap();
    let rendered = queries::comments::get_comment(&c, comment).unwrap();
    assert_eq!(rendered.user_id, -1);
    assert!(rendered.author.contains("author"));
    assert!(rendered.author.contains("imported"));
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM comments WHERE user_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM issue_relations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM comments WHERE deleted_at IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM plan_steps", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM audit_log WHERE project_id=?1 AND transport='imported'",
            [project],
            |r| r.get::<_, usize>(0)
        )
        .unwrap(),
        before.rows("audit_log").len()
    );
    assert!(
        c.query_row(
            "SELECT count(*) FROM audit_log WHERE project_id=?1 AND old_value='Historical text'",
            [project],
            |r| r.get::<_, i64>(0)
        )
        .unwrap()
            > 0
    );
    assert_eq!(c.query_row("SELECT count(*) FROM status_transitions WHERE seq IS NULL OR actor_user_id IS NOT NULL",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM search_index WHERE entity_type='issue' AND entity_id=?1",
            [issue.0],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(c.query_row("SELECT count(*) FROM search_index s JOIN issues i ON s.entity_type='issue' AND s.entity_id=i.id WHERE i.deleted_at IS NOT NULL",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(c.query_row("SELECT count(*) FROM project_members WHERE project_id=?1 AND user_id=1 AND role='lead'",[project],|r|r.get::<_,i64>(0)).unwrap(),1);
    let hash: String = c
        .query_row("SELECT sha256 FROM attachments", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        deststore.read(&hash).unwrap(),
        b"project archive attachment"
    );
    drop(c);
    assert!(
        import(&dest, &deststore, &archive, "owner")
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    let c = source.read().unwrap();
    let after = collect_manifest(&c, "LIF").unwrap();
    assert_eq!(
        serde_json::to_value(before.tables).unwrap(),
        serde_json::to_value(after.tables).unwrap()
    );
    // A second export must retain inert attribution and the original history.
    let again = dir.path().join("again.tar.gz");
    export(&dest, &deststore, "LIF", &again).unwrap();
    let (third, _third_dir, third_store) = fixture();
    import(&third, &third_store, &again, "owner").unwrap();
}

#[test]
fn project_archive_rejects_unknown_schema_foreign_ids_and_parent_cycles() {
    let (pool, _dir, store) = fixture();
    seed(&pool, &store);
    let c = pool.read().unwrap();
    let mut m = collect_manifest(&c, "LIF").unwrap();
    m.format_version = 999;
    assert!(validate_manifest(&m).is_err());
    m.format_version = VERSION;
    m.tables[0].rows[0].push("extra".into());
    assert!(validate_manifest(&m).is_err());
    m.tables[0].rows[0].pop();
    m.tables[1].rows[0][1] = 20.into();
    assert!(validate_manifest(&m).is_err());
    m.tables[1].rows[0][1] = 10.into();
    let folders = m.tables.iter_mut().find(|t| t.name == "folders").unwrap();
    folders.rows[0][2] = 6.into();
    assert!(validate_manifest(&m).is_err());
}

#[test]
fn project_archive_rejects_missing_corrupt_duplicate_and_unsafe_entries() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let c = pool.read().unwrap();
    let m = collect_manifest(&c, "LIF").unwrap();
    let path = dir.path().join("hostile.tar.gz");
    write_manifest(&path, &m, &[]);
    assert!(stage(&path).is_err());
    let name = format!("blobs/{}", m.blobs[0].sha256);
    write_manifest(&path, &m, &[(&name, b"wrong")]);
    assert!(stage(&path).is_err());
    write_manifest(
        &path,
        &m,
        &[
            (&name, b"project archive attachment"),
            (&name, b"project archive attachment"),
        ],
    );
    assert!(stage(&path).is_err());
    write_manifest(&path, &m, &[("unexpected", b"x")]);
    assert!(stage(&path).is_err());
    write_manifest(&path, &m, &[("blobs/not-a-hash", b"x")]);
    assert!(stage(&path).is_err());
}

#[test]
fn project_archive_requires_destination_admin_and_rolls_back_invalid_rows() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let c = pool.read().unwrap();
    let mut m = collect_manifest(&c, "LIF").unwrap();
    let path = dir.path().join("bad.tar.gz");
    let name = format!("blobs/{}", m.blobs[0].sha256);
    write_manifest(&path, &m, &[(&name, b"project archive attachment")]);
    let (dest, _destdir, deststore) = fixture();
    assert!(import(&dest, &deststore, &path, "nobody").is_err());
    m.tables
        .iter_mut()
        .find(|t| t.name == "issues")
        .unwrap()
        .rows[0][5] = "not-a-status".into();
    write_manifest(&path, &m, &[(&name, b"project archive attachment")]);
    assert!(import(&dest, &deststore, &path, "owner").is_err());
    let c = dest.read().unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(
        c.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='trigger'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap()
            > 30
    );
}

#[test]
fn project_archive_never_overwrites_existing_corrupt_shared_blob() {
    let (source, dir, store) = fixture();
    seed(&source, &store);
    let archive = dir.path().join("project.tar.gz");
    export(&source, &store, "LIF", &archive).unwrap();
    let m = stage(&archive).unwrap().manifest;
    let (dest, _destdir, deststore) = fixture();
    let path = deststore.path_for(&m.blobs[0].sha256).unwrap();
    std::fs::write(&path, b"existing corrupt data").unwrap();
    assert!(import(&dest, &deststore, &archive, "owner").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"existing corrupt data");
    let c = dest.read().unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[cfg(unix)]
#[test]
fn project_archive_rejects_symlink_inputs_and_writes_private_archives() {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let archive = dir.path().join("archive.tar.gz");
    export(&pool, &store, "LIF", &archive).unwrap();
    assert_eq!(
        std::fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let link = dir.path().join("link");
    symlink(&archive, &link).unwrap();
    assert!(stage(&link).is_err());
    assert!(export(&pool, &store, "LIF", &archive).is_err());
}

#[test]
fn project_archive_cli_rejects_export_without_out() {
    use crate::cli::Cli;
    use clap::{Parser, error::ErrorKind};
    let error = Cli::try_parse_from(["lific", "project-archive", "export", "LIF"])
        .err()
        .unwrap();
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    assert!(error.to_string().contains("--out"));
}

#[test]
fn project_archive_cli_rejects_import_without_user() {
    use crate::cli::Cli;
    use clap::{Parser, error::ErrorKind};
    let error = Cli::try_parse_from(["lific", "project-archive", "import", "archive.tar.gz"])
        .err()
        .unwrap();
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    assert!(error.to_string().contains("--user"));
}

#[test]
fn project_archive_cli_parses_import_path_and_owner() {
    use crate::cli::{Cli, Command, ProjectArchiveAction};
    use clap::Parser;
    let parsed = Cli::try_parse_from([
        "lific",
        "project-archive",
        "import",
        "archive.tar.gz",
        "--user",
        "owner",
    ])
    .unwrap();
    assert!(
        matches!(parsed.command,Command::ProjectArchive { action:ProjectArchiveAction::Import { archive,user } } if user=="owner" && archive==Path::new("archive.tar.gz"))
    );
}

#[test]
fn project_archive_cli_parses_export_project_output_and_database() {
    use crate::cli::{Cli, Command, ProjectArchiveAction};
    use clap::Parser;
    let parsed = Cli::try_parse_from([
        "lific",
        "--db",
        "existing.db",
        "project-archive",
        "export",
        "LIF",
        "--out",
        "archive.tar.gz",
    ])
    .unwrap();
    assert_eq!(parsed.db.as_deref(), Some(Path::new("existing.db")));
    assert!(
        matches!(parsed.command,Command::ProjectArchive { action:ProjectArchiveAction::Export { project,out } } if project=="LIF" && out==Path::new("archive.tar.gz"))
    );
}

#[test]
fn project_archive_cli_uses_local_operator_dispatch_and_requires_an_existing_database() {
    use crate::cli::Cli;
    use clap::Parser;
    for args in [
        [
            "lific",
            "project-archive",
            "export",
            "LIF",
            "--out",
            "archive.tar.gz",
        ],
        [
            "lific",
            "project-archive",
            "import",
            "archive.tar.gz",
            "--user",
            "owner",
        ],
    ] {
        let parsed = Cli::try_parse_from(args).unwrap();
        let plan = crate::cli::runtime::plan(&parsed.command);
        assert!(!plan.is_data());
        assert!(plan.requires_existing_database());
    }
}

fn raw_entry(path: &Path, name: &str, kind: u8, size: u64) {
    let gzip =
        flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gzip);
    let mut header = tar::Header::new_ustar();
    header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
    header.set_entry_type(tar::EntryType::new(kind));
    header.set_size(size);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append(&header, &[][..]).unwrap();
    tar.into_inner().unwrap().finish().unwrap();
}

#[test]
fn project_archive_refuses_tar_traversal_links_devices_and_declared_expansion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.tar.gz");
    for name in [
        "../outside",
        "/absolute",
        "blobs/../../outside",
        "blobs\\outside",
    ] {
        raw_entry(&path, name, b'0', 0);
        assert!(stage(&path).is_err(), "{name}");
    }
    for kind in *b"123456Lxg" {
        raw_entry(&path, "manifest.json", kind, 0);
        assert!(stage(&path).is_err(), "entry type {kind}");
    }
    raw_entry(&path, "manifest.json", b'0', Limits::CLI.max_metadata + 1);
    assert!(stage(&path).err().unwrap().to_string().contains("64 MiB"));
    File::create(&path)
        .unwrap()
        .set_len(Limits::CLI.max_compressed + 1)
        .unwrap();
    assert!(
        stage(&path)
            .err()
            .unwrap()
            .to_string()
            .contains("compressed archive")
    );
}

#[test]
fn project_archive_rejects_trailing_members_bad_crc_nested_rows_and_duplicate_json_keys() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let path = dir.path().join("archive.tar.gz");
    export(&pool, &store, "LIF", &path).unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut appended = original.clone();
    appended.extend_from_slice(&original);
    std::fs::write(&path, &appended).unwrap();
    assert!(stage(&path).is_err());
    let mut corrupt = original.clone();
    let index = corrupt.len() - 8;
    corrupt[index] ^= 0xff;
    std::fs::write(&path, &corrupt).unwrap();
    assert!(stage(&path).is_err());
    std::fs::write(&path, &original[..original.len() - 5]).unwrap();
    assert!(stage(&path).is_err());
    assert!(
        serde_json::from_str::<Table>(r#"{"name":"issues","rows":[[{"sql":"DROP TABLE users"}]]}"#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<Table>(r#"{"name":"issues","name":"users","rows":[]}"#).is_err()
    );
    assert!(serde_json::from_str::<Table>(r#"{"name":"issues","rows":[[]]}"#).is_err());
}

#[test]
fn project_archive_bounds_blob_sizes_and_disables_unresolved_local_attachment_ids() {
    let (pool, _dir, store) = fixture();
    seed(&pool, &store);
    let c = pool.read().unwrap();
    let mut m = collect_manifest(&c, "LIF").unwrap();
    m.blobs[0].size = Limits::CLI.max_blob + 1;
    assert!(validate_manifest(&m).is_err());
    let m = collect_manifest(&c, "LIF").unwrap();
    let mut maps = source_maps(&m).unwrap();
    maps.get_mut("attachments").unwrap().insert(80, 123);
    let mut external = RewriteState::new(&[]).unwrap();
    let result = remap_markdown(
        "[one](/api/attachments/80) [other](/api/attachments/999) [absolute](https://source.test/api/attachments/80)",
        &maps,
        &mut external,
    ).unwrap();
    assert_eq!(
        result,
        "[one](/api/attachments/123) [other](/unresolved-source-attachment/999) [absolute](https://source.test/api/attachments/80)"
    );
    assert_eq!(external.references.len(), 2);
}

#[test]
fn project_archive_preserves_destination_settings_and_keeps_published_source_private() {
    let (source, dir, store) = fixture();
    seed(&source, &store);
    let has_public = {
        let c = source.write().unwrap();
        let present: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('projects') WHERE name='is_public')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if present {
            c.execute("UPDATE projects SET is_public=1 WHERE id=10", [])
                .unwrap();
        }
        present
    };
    let path = dir.path().join("archive.tar.gz");
    export(&source, &store, "LIF", &path).unwrap();
    let (dest, _destdir, deststore) = fixture();
    let settings_before = {
        let c = dest.write().unwrap();
        queries::settings::ensure(&c, false).unwrap();
        c.execute("UPDATE instance_settings SET authz_enforced=1", [])
            .unwrap();
        c.query_row("SELECT authz_enforced FROM instance_settings", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    import(&dest, &deststore, &path, "owner").unwrap();
    let c = dest.read().unwrap();
    assert_eq!(
        c.query_row("SELECT authz_enforced FROM instance_settings", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        settings_before
    );
    if has_public {
        assert_eq!(
            c.query_row(
                "SELECT is_public FROM projects WHERE identifier='LIF'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    assert_eq!(c.query_row("SELECT count(*) FROM attachments_fts WHERE extracted_text='project archive attachment'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    let project: i64 = c
        .query_row("SELECT id FROM projects WHERE identifier='LIF'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let cursor: i64 = c
        .query_row("SELECT value FROM sync_seq", [], |r| r.get(0))
        .unwrap();
    let serialized =
        serde_json::to_value(queries::changes::list_changes(&c, project, 0, 500).unwrap()).unwrap();
    assert!(serialized.to_string().contains("imported"));
    assert!(cursor > 0);
}

#[test]
fn project_archive_failed_blob_install_rolls_back_rows_and_removes_only_new_files() {
    let (source, dir, store) = fixture();
    seed(&source, &store);
    let hash = store.write(b"second file").unwrap();
    {
        let c = source.write().unwrap();
        c.execute("INSERT INTO attachments(id,sha256,filename,mime,size_bytes) VALUES(81,?1,'second.txt','text/plain',11)",[&hash]).unwrap();
        c.execute(
            "INSERT INTO attachment_links VALUES(81,'issue',30,'2025-01-01')",
            [],
        )
        .unwrap();
    }
    let path = dir.path().join("archive.tar.gz");
    export(&source, &store, "LIF", &path).unwrap();
    let m = stage(&path).unwrap().manifest;
    let (dest, _destdir, deststore) = fixture();
    // Blob entries are hash-sorted. Let the first install succeed, then fail.
    let first = deststore.path_for(&m.blobs[0].sha256).unwrap();
    let second = deststore.path_for(&m.blobs[1].sha256).unwrap();
    std::fs::write(&second, b"existing bytes").unwrap();
    assert!(import(&dest, &deststore, &path, "owner").is_err());
    assert!(!first.exists());
    assert_eq!(std::fs::read(&second).unwrap(), b"existing bytes");
    let c = dest.read().unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT value FROM sync_seq", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM attachments_fts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn project_archive_snapshot_survives_concurrent_wal_write_and_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let pool = db::open(&path).unwrap();
    {
        let c = pool.write().unwrap();
        c.execute_batch("INSERT INTO projects(name,identifier) VALUES('Before','LIF'); INSERT INTO issues(project_id,sequence,title) VALUES(1,1,'Before');").unwrap();
    }
    let c = pool.read().unwrap();
    let tx = c.unchecked_transaction().unwrap();
    let before = collect_manifest(&tx, "LIF").unwrap();
    {
        let writer = pool.write().unwrap();
        writer
            .execute_batch("UPDATE projects SET name='After'; UPDATE issues SET title='After';")
            .unwrap();
        writer
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE)")
            .unwrap();
    }
    let still_before = collect_manifest(&tx, "LIF").unwrap();
    assert_eq!(
        serde_json::to_value(before.tables).unwrap(),
        serde_json::to_value(still_before.tables).unwrap()
    );
    tx.commit().unwrap();
    assert_eq!(
        c.query_row("SELECT name FROM projects", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "After"
    );
}

#[test]
fn project_archive_uses_canonical_project_identifiers_before_importing_any_rows() {
    let (source, dir, store) = fixture();
    seed(&source, &store);
    let conn = source.read().unwrap();
    let mut manifest = collect_manifest(&conn, "LIF").unwrap();
    let projects = spec("projects").unwrap();
    let path = dir.path().join("invalid-identifier.tar.gz");
    let blob = format!("blobs/{}", manifest.blobs[0].sha256);
    let (dest, _dest_dir, dest_store) = fixture();
    for identifier in [
        "DOC", "doc", "lif", "Lif", "1LIF", "123", "", "TOOLONG", "LI-F", "ÉT",
    ] {
        projects.set(
            &mut manifest.tables[0].rows[0],
            "identifier",
            identifier.into(),
        );
        let expected = queries::validate_identifier(identifier)
            .unwrap_err()
            .to_string();
        assert_eq!(
            validate_manifest(&manifest).unwrap_err().to_string(),
            expected
        );
        write_manifest(&path, &manifest, &[(&blob, b"project archive attachment")]);
        assert_eq!(
            import(&dest, &dest_store, &path, "owner")
                .unwrap_err()
                .to_string(),
            expected
        );
    }
    for identifier in ["A", "LIF", "PRO2", "A1234"] {
        projects.set(
            &mut manifest.tables[0].rows[0],
            "identifier",
            identifier.into(),
        );
        validate_manifest(&manifest).unwrap();
    }
    let conn = dest.read().unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn project_archive_preserves_absolute_attachment_urls_in_markdown_and_raw_html() {
    let maps = BTreeMap::from([("attachments", BTreeMap::from([(80, 123)]))]);
    for input in [
        "HTTPS://host/api/attachments/80",
        "[file](HtTpS://host/api/attachments/80)",
        "<HTTPS://host/api/attachments/80>",
        "<a href=https://host/api/attachments/80>file</a>",
        "<a href=HTTPS://host/api/attachments/80>file</a>",
        "<img src=HtTp://host/api/attachments/80>",
        "<a href='HTTPS://host/api/attachments/80'>file</a>",
        "<img src=\"https://host/api/attachments/80\">",
        "<a href=//host/api/attachments/80>file</a>",
        "[file](//host/api/attachments/80)",
        "https://host/(path)/api/attachments/80",
        "https://host/api/attachments/80?next=/api/attachments/80",
    ] {
        let mut state = RewriteState::new(&[]).unwrap();
        assert_eq!(remap_markdown(input, &maps, &mut state).unwrap(), input);
        assert_eq!(
            state.references.len(),
            1,
            "absolute URL must be reported: {input}"
        );
        assert!(
            state
                .references
                .first()
                .unwrap()
                .starts_with("absolute attachment URL")
        );
    }
    for input in [
        "[file](/api/attachments/80)",
        "<a href=/api/attachments/80>file</a>",
        "<img src=\"/api/attachments/80\">",
        "HTTPS://host/api/attachments/80\u{2003}<a href=/api/attachments/80>x</a>",
    ] {
        let mut state = RewriteState::new(&[]).unwrap();
        let result = remap_markdown(input, &maps, &mut state).unwrap();
        assert!(result.contains("/api/attachments/123"));
        if input.starts_with("HTTPS") {
            assert!(result.starts_with("HTTPS://host/api/attachments/80"));
        }
    }
}

#[test]
fn project_archive_caps_generated_references_and_output_while_rewriting_across_rows() {
    let maps = BTreeMap::from([("attachments", BTreeMap::new())]);
    let mut state = RewriteState::new(&["already reported".into()]).unwrap();
    state.max_references = 3;
    remap_markdown("/api/attachments/1 /api/attachments/2", &maps, &mut state).unwrap();
    // Deduplication still works at the boundary, but a new row cannot reset it.
    remap_markdown("/api/attachments/1", &maps, &mut state).unwrap();
    assert!(
        remap_markdown("/api/attachments/3 /api/attachments/4", &maps, &mut state)
            .unwrap_err()
            .to_string()
            .contains("too many external")
    );
    assert_eq!(state.references.len(), 3);

    let mut state = RewriteState::new(&[]).unwrap();
    state.max_output_bytes = 35;
    let first = remap_markdown("/api/attachments/1", &maps, &mut state).unwrap();
    assert_eq!(state.output_bytes, first.len());
    assert!(
        remap_markdown("/api/attachments/2", &maps, &mut state)
            .unwrap_err()
            .to_string()
            .contains("rewritten content")
    );
    assert!(state.output_bytes <= 35);
    let before = state.output_bytes;
    assert!(remap_markdown(&"x".repeat(100), &maps, &mut state).is_err());
    assert_eq!(
        state.output_bytes, before,
        "oversized chunks are rejected before appending"
    );

    let mut state = RewriteState::new(&[]).unwrap();
    state.max_reference_bytes = 100;
    remap_markdown("/api/attachments/1", &maps, &mut state).unwrap();
    assert!(
        remap_markdown("/api/attachments/2", &maps, &mut state)
            .unwrap_err()
            .to_string()
            .contains("reference byte limit")
    );
    assert_eq!(state.references.len(), 1);
    assert!(state.reference_bytes <= 100);
    let before = state.reference_bytes;
    assert!(
        remap_markdown(
            &format!("/api/attachments/{}", "1".repeat(101)),
            &maps,
            &mut state
        )
        .is_err()
    );
    assert_eq!(state.reference_bytes, before);
}

#[test]
fn project_archive_generated_reference_overflow_aborts_the_import_transaction() {
    use std::fmt::Write as _;
    let (source, dir, store) = fixture();
    seed(&source, &store);
    let conn = source.read().unwrap();
    let mut manifest = collect_manifest(&conn, "LIF").unwrap();
    manifest.external_references.clear();
    let mut body = String::new();
    for id in 1..=Limits::CLI.max_rows + 2 {
        write!(body, "/api/attachments/{id} ").unwrap();
    }
    let issues = manifest
        .tables
        .iter_mut()
        .find(|t| t.name == "issues")
        .unwrap();
    spec("issues")
        .unwrap()
        .set(&mut issues.rows[0], "description", body.into());
    validate_manifest(&manifest).unwrap();
    let path = dir.path().join("reference-bomb.tar.gz");
    let blob = format!("blobs/{}", manifest.blobs[0].sha256);
    write_manifest(&path, &manifest, &[(&blob, b"project archive attachment")]);
    let (dest, _dest_dir, dest_store) = fixture();
    let error = import(&dest, &dest_store, &path, "owner").unwrap_err();
    assert!(error.to_string().contains("too many external references"));
    let conn = dest.read().unwrap();
    for table in [
        "projects",
        "issues",
        "attachments",
        "project_archive_provenance",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(
        conn.query_row("SELECT value FROM sync_seq", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn project_archive_attachment_uploader_filter_matches_imported_display_without_an_account() {
    use crate::db::models::ProjectAttachmentQuery;
    let (source, dir, store) = fixture();
    seed(&source, &store);
    let path = dir.path().join("archive.tar.gz");
    export(&source, &store, "LIF", &path).unwrap();
    let (dest, _dest_dir, dest_store) = fixture();
    import(&dest, &dest_store, &path, "owner").unwrap();
    let conn = dest.read().unwrap();
    let project = conn
        .query_row("SELECT id FROM projects WHERE identifier='LIF'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let unfiltered = queries::attachments::list_project_attachments(
        &conn,
        project,
        &ProjectAttachmentQuery::default(),
    )
    .unwrap();
    let displayed = unfiltered.items[0].uploader.as_ref().unwrap();
    assert_eq!(displayed, "author (imported)");
    let filtered = queries::attachments::list_project_attachments(
        &conn,
        project,
        &ProjectAttachmentQuery {
            uploader: Some(displayed.to_uppercase()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(filtered.total_count, unfiltered.total_count);
    assert_eq!(filtered.total_bytes, unfiltered.total_bytes);
    assert_eq!(filtered.items.len(), 1);
    assert!(filtered.items[0].uploader_id.is_none());
}

#[test]
fn project_archive_roundtrip_does_not_require_the_publication_schema() {
    let (source, dir, store) = fixture();
    let (dest, _dest_dir, dest_store) = fixture();
    for pool in [&source, &dest] {
        let conn = pool.write().unwrap();
        let has_public: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('projects') WHERE name='is_public')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if has_public {
            // Remove only migration 051's optional objects from this test DB.
            // The same test also runs unchanged when 050 is the latest migration.
            conn.execute_batch(
                "DROP TRIGGER IF EXISTS audit_projects_publication;
                 DROP INDEX IF EXISTS idx_projects_public;
                 ALTER TABLE projects DROP COLUMN is_public;",
            )
            .unwrap();
        }
    }
    seed(&source, &store);
    let path = dir.path().join("before-publication.tar.gz");
    export(&source, &store, "LIF", &path).unwrap();
    assert_eq!(
        import(&dest, &dest_store, &path, "owner").unwrap().project,
        "LIF"
    );
}

#[test]
fn project_archive_column_types_match_every_static_schema_column() {
    let (source, _dir, store) = fixture();
    seed(&source, &store);
    let conn = source.read().unwrap();
    let mut manifest = collect_manifest(&conn, "LIF").unwrap();
    for s in SPECS {
        let mut statement = conn
            .prepare(&format!("PRAGMA table_info({})", s.name))
            .unwrap();
        let types = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .unwrap()
            .collect::<std::result::Result<BTreeMap<_, _>, _>>()
            .unwrap();
        let table = manifest
            .tables
            .iter()
            .position(|t| t.name == s.name)
            .unwrap();
        for (index, column) in s.cols().into_iter().enumerate() {
            let wrong = match types[column].as_str() {
                "INTEGER" | "REAL" => Value::String("not-a-number".into()),
                "TEXT" => Value::from(123),
                other => panic!("unreviewed schema type {}.{column}: {other}", s.name),
            };
            let original = std::mem::replace(&mut manifest.tables[table].rows[0][index], wrong);
            assert!(
                validate_manifest(&manifest)
                    .unwrap_err()
                    .to_string()
                    .contains(&format!("{}.{column}", s.name))
            );
            manifest.tables[table].rows[0][index] = original;
        }
    }
    for table in ["projects", "folders", "issues", "pages"] {
        let s = spec(table).unwrap();
        let index = manifest
            .tables
            .iter()
            .position(|t| t.name == table)
            .unwrap();
        let old = s.get(&manifest.tables[index].rows[0], "sort_order").clone();
        s.set(
            &mut manifest.tables[index].rows[0],
            "sort_order",
            0.25.into(),
        );
        assert_eq!(
            validate_manifest(&manifest).is_ok(),
            table != "projects",
            "{table}"
        );
        s.set(&mut manifest.tables[index].rows[0], "sort_order", old);
    }
}

#[test]
fn project_archive_hostile_types_mime_and_booleans_leave_both_databases_unchanged() {
    let (source, dir, store) = fixture();
    seed(&source, &store);
    let conn = source.read().unwrap();
    let mut manifest = collect_manifest(&conn, "LIF").unwrap();
    let source_before = serde_json::to_value(&manifest.tables).unwrap();
    let path = dir.path().join("hostile-types.tar.gz");
    let blob = format!("blobs/{}", manifest.blobs[0].sha256);
    let (dest, _dest_dir, dest_store) = fixture();
    for (table, column, value) in [
        ("attachments", "width", Value::from("wide")),
        ("attachments", "height", Value::from("320")),
        ("attachments", "width", Value::from(0.5)),
        (
            "attachments",
            "mime",
            Value::from("text/plain\r\nX-Injected: yes"),
        ),
        ("attachments", "mime", Value::from("text/plain\0")),
        ("attachments", "mime", Value::from("application/x-unknown")),
        ("attachments", "filename", Value::from("file\0.txt")),
        ("attachments", "filename", Value::from("x".repeat(1025))),
        ("pages", "content", Value::from(123)),
        ("comments", "content", Value::from(456)),
        ("issues", "title", Value::from(789)),
        ("projects", "sort_order", Value::from(0.5)),
        ("projects", "sort_order", Value::from(1.0)),
        ("plan_steps", "done", Value::from(2)),
        ("pages", "pinned", Value::from(-1)),
    ] {
        let s = spec(table).unwrap();
        let index = manifest
            .tables
            .iter()
            .position(|t| t.name == table)
            .unwrap();
        let original = s.get(&manifest.tables[index].rows[0], column).clone();
        s.set(&mut manifest.tables[index].rows[0], column, value);
        assert!(validate_manifest(&manifest).is_err(), "{table}.{column}");
        write_manifest(&path, &manifest, &[(&blob, b"project archive attachment")]);
        assert!(
            import(&dest, &dest_store, &path, "owner").is_err(),
            "{table}.{column}"
        );
        s.set(&mut manifest.tables[index].rows[0], column, original);
    }
    assert_eq!(
        serde_json::to_value(collect_manifest(&conn, "LIF").unwrap().tables).unwrap(),
        source_before
    );
    let conn = dest.read().unwrap();
    for table in [
        "projects",
        "audit_log",
        "attachments",
        "project_members",
        "search_index",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(
        conn.query_row("SELECT value FROM sync_seq", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn project_archive_required_nulls_fail_transactionally_without_a_partial_project() {
    let (source, dir, store) = fixture();
    seed(&source, &store);
    let conn = source.read().unwrap();
    let mut manifest = collect_manifest(&conn, "LIF").unwrap();
    let before = serde_json::to_value(&manifest.tables).unwrap();
    let path = dir.path().join("required-null.tar.gz");
    let blob = format!("blobs/{}", manifest.blobs[0].sha256);
    let (dest, _dest_dir, dest_store) = fixture();
    for (table, column) in [
        ("projects", "name"),
        ("issues", "description"),
        ("comments", "created_at"),
        ("plan_steps", "done"),
        ("pages", "pinned"),
        ("attachments", "mime"),
        ("labels", "color"),
    ] {
        let s = spec(table).unwrap();
        let index = manifest
            .tables
            .iter()
            .position(|t| t.name == table)
            .unwrap();
        let original = s.get(&manifest.tables[index].rows[0], column).clone();
        s.set(&mut manifest.tables[index].rows[0], column, Value::Null);
        validate_manifest(&manifest).unwrap();
        write_manifest(&path, &manifest, &[(&blob, b"project archive attachment")]);
        assert!(
            import(&dest, &dest_store, &path, "owner").is_err(),
            "{table}.{column}"
        );
        s.set(&mut manifest.tables[index].rows[0], column, original);
        let conn = dest.read().unwrap();
        for table in [
            "projects",
            "audit_log",
            "attachments",
            "project_members",
            "search_index",
        ] {
            assert_eq!(
                conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            conn.query_row("SELECT value FROM sync_seq", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(
        serde_json::to_value(collect_manifest(&conn, "LIF").unwrap().tables).unwrap(),
        before
    );
}

#[test]
fn project_archive_audits_the_destination_lead_grant_without_changing_source_timestamps() {
    let (source, dir, store) = fixture();
    seed(&source, &store);
    {
        let conn = source.write().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let triggers = db::migrate::suspend_triggers(&tx).unwrap();
        tx.execute(
            "UPDATE projects SET updated_at='2020-01-01 00:00:00' WHERE identifier='LIF'",
            [],
        )
        .unwrap();
        for sql in triggers {
            tx.execute_batch(&sql).unwrap();
        }
        tx.commit().unwrap();
    }
    let path = dir.path().join("grant.tar.gz");
    export(&source, &store, "LIF", &path).unwrap();
    let imported_history = stage(&path).unwrap().manifest.rows("audit_log").len();
    let (dest, _dest_dir, dest_store) = fixture();
    import(&dest, &dest_store, &path, "owner").unwrap();
    let conn = dest.read().unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT updated_at FROM projects WHERE identifier='LIF'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "2020-01-01 00:00:00"
    );
    let grant: (Option<i64>, String, i64, String, String, String) = conn.query_row(
        "SELECT actor_user_id,transport,entity_id,entity_label,action,new_value FROM audit_log WHERE entity_type='member'",
        [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
    ).unwrap();
    assert_eq!(
        grant,
        (
            None,
            "cli".into(),
            1,
            "owner".into(),
            "create".into(),
            "lead".into()
        )
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM audit_log WHERE transport='imported'",
            [],
            |r| r.get::<_, usize>(0)
        )
        .unwrap(),
        imported_history
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM audit_log", [], |r| r
            .get::<_, usize>(0))
            .unwrap(),
        imported_history + 1
    );
    assert_eq!(
        conn.query_row("SELECT transport FROM _actor_state", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "system"
    );
}

// ── LIF-467: per-caller resource profiles ───────────────────────

// Every profile below is small enough that the ordinary seeded project trips
// the ceiling under test, so none of these need a large fixture.
#[test]
fn project_archive_limits_bound_metadata_rows_and_blobs_during_collection() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);

    // Rows: the seed has well over five.
    let profile = Limits {
        max_rows: 5,
        ..Limits::CLI
    };
    let error = export_with(
        &pool,
        &store,
        "LIF",
        &dir.path().join("rows.tar.gz"),
        profile,
        &|_| Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("too many rows"), "{error}");

    // Metadata: text columns are counted as they are read, not after the
    // manifest is serialized.
    let profile = Limits {
        max_metadata: 16,
        ..Limits::CLI
    };
    let error = export_with(
        &pool,
        &store,
        "LIF",
        &dir.path().join("metadata.tar.gz"),
        profile,
        &|_| Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("metadata exceeds"), "{error}");

    // One blob, and the combined blob budget.
    let profile = Limits {
        max_blob: 4,
        ..Limits::CLI
    };
    let error = export_with(
        &pool,
        &store,
        "LIF",
        &dir.path().join("blob.tar.gz"),
        profile,
        &|_| Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("blob"), "{error}");

    let profile = Limits {
        max_blobs: 0,
        ..Limits::CLI
    };
    let error = export_with(
        &pool,
        &store,
        "LIF",
        &dir.path().join("blobs.tar.gz"),
        profile,
        &|_| Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("too many blobs"), "{error}");

    // Nothing was left behind by any of the refusals.
    for name in ["rows", "metadata", "blob", "blobs"] {
        assert!(!dir.path().join(format!("{name}.tar.gz")).exists());
    }
}

#[test]
fn project_archive_compressed_ceiling_stops_the_export_while_it_writes() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let out = dir.path().join("compressed.tar.gz");
    let error = export_with(
        &pool,
        &store,
        "LIF",
        &out,
        Limits {
            max_compressed: 64,
            ..Limits::CLI
        },
        &|_| Ok(()),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("compressed archive exceeds"),
        "{error}"
    );
    assert!(!out.exists(), "a refused export leaves no file behind");
}

#[test]
fn project_archive_expanded_ceiling_stops_the_export_while_it_writes() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let out = dir.path().join("expanded.tar.gz");
    let error = export_with(
        &pool,
        &store,
        "LIF",
        &out,
        Limits {
            max_expanded: 128,
            ..Limits::CLI
        },
        &|_| Ok(()),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("archive contents exceed"),
        "{error}"
    );
    assert!(!out.exists());
}

#[test]
fn project_archive_limits_bound_a_decompressed_import_before_it_is_staged() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let path = dir.path().join("archive.tar.gz");
    export(&pool, &store, "LIF", &path).unwrap();
    let (dest, _dest_dir, dest_store) = fixture();

    // The compressed file is well under a kilobyte, so a byte-level
    // expansion ceiling is the only thing that can reject it here.
    let error = import_with(
        &dest,
        &dest_store,
        &path,
        Limits {
            max_expanded: 64,
            ..Limits::CLI
        },
        &|tx| cli_grant(tx, "owner"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("project archive"), "{error}");

    let error = import_with(
        &dest,
        &dest_store,
        &path,
        Limits {
            max_compressed: 8,
            ..Limits::CLI
        },
        &|tx| cli_grant(tx, "owner"),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("compressed archive exceeds"),
        "{error}"
    );

    let error = import_with(
        &dest,
        &dest_store,
        &path,
        Limits {
            max_metadata: 16,
            ..Limits::CLI
        },
        &|tx| cli_grant(tx, "owner"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("metadata exceeds"), "{error}");

    let error = import_with(
        &dest,
        &dest_store,
        &path,
        Limits {
            max_rows: 3,
            ..Limits::CLI
        },
        &|tx| cli_grant(tx, "owner"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("project archive"), "{error}");

    let conn = dest.read().unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0,
        "a refused import writes nothing"
    );
}

#[test]
fn project_archive_limits_do_not_leak_between_runs_on_one_thread() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let _ = export_with(
        &pool,
        &store,
        "LIF",
        &dir.path().join("tiny.tar.gz"),
        Limits {
            max_rows: 1,
            ..Limits::CLI
        },
        &|_| Ok(()),
    );
    // The CLI profile is restored, so the next export on this thread is
    // judged by its own limits and not the previous caller's.
    export(&pool, &store, "LIF", &dir.path().join("normal.tar.gz")).unwrap();
}

#[test]
fn project_archive_import_refuses_before_writing_when_the_hook_denies() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let path = dir.path().join("archive.tar.gz");
    export(&pool, &store, "LIF", &path).unwrap();
    let (dest, _dest_dir, dest_store) = fixture();
    let error = import_with(&dest, &dest_store, &path, Limits::WEB, &|_| {
        Err(LificError::Forbidden("no longer an admin".into()))
    })
    .unwrap_err();
    assert!(matches!(error, LificError::Forbidden(_)));
    let conn = dest.read().unwrap();
    for table in ["projects", "issues", "project_members"] {
        assert_eq!(
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
}

#[test]
fn project_archive_export_refuses_before_reading_when_the_hook_denies() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let out = dir.path().join("denied.tar.gz");
    let error = export_with(&pool, &store, "LIF", &out, Limits::WEB, &|_| {
        Err(LificError::Forbidden("lead was revoked".into()))
    })
    .unwrap_err();
    assert!(matches!(error, LificError::Forbidden(_)));
    assert!(!out.exists());
}

#[test]
fn project_archive_import_reports_the_committed_project_and_per_table_rows() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let path = dir.path().join("archive.tar.gz");
    export(&pool, &store, "LIF", &path).unwrap();
    let (dest, _dest_dir, dest_store) = fixture();
    {
        // An unrelated project already occupies the low ID range, so a
        // project resolved by name after the fact could not be assumed to be
        // this one.
        let conn = dest.write().unwrap();
        conn.execute_batch("INSERT INTO projects(id,name,identifier) VALUES(500,'Other','OTH');")
            .unwrap();
    }
    let outcome = import_with(&dest, &dest_store, &path, Limits::WEB, &|tx| {
        cli_grant(tx, "owner")
    })
    .unwrap();

    let conn = dest.read().unwrap();
    let actual: i64 = conn
        .query_row("SELECT id FROM projects WHERE identifier='LIF'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(outcome.project_id, actual);
    assert_eq!(outcome.report.project, "LIF");
    assert_eq!(outcome.rows_by_table["projects"], 1);
    assert_eq!(outcome.rows_by_table["issues"], 2);
    assert_eq!(outcome.rows_by_table["attachments"], 1);
    assert_eq!(
        outcome.rows_by_table.values().sum::<usize>(),
        outcome.report.rows
    );
    assert_eq!(
        outcome.external_reference_count,
        outcome.report.external_references.len()
    );
}

#[test]
fn project_archive_import_audits_the_lead_grant_with_the_caller_the_hook_names() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let path = dir.path().join("archive.tar.gz");
    export(&pool, &store, "LIF", &path).unwrap();
    let (dest, _dest_dir, dest_store) = fixture();
    let outcome = import_with(&dest, &dest_store, &path, Limits::WEB, &|_| {
        Ok(Grant {
            user_id: 1,
            transport: crate::actor::Transport::Web,
            actor_user_id: Some(1),
        })
    })
    .unwrap();

    let conn = dest.read().unwrap();
    let (transport, actor): (String, Option<i64>) = conn
        .query_row(
            "SELECT transport, actor_user_id FROM audit_log
             WHERE entity_type='member' OR action='member_added'
             ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(transport, "web");
    assert_eq!(actor, Some(1));
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM project_members WHERE project_id=?1 AND user_id=1 AND role='lead'",
            [outcome.project_id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

/// The identifier a caller names and the project a snapshot exports must be
/// tied together by the row ID. Resolving the label twice lets a rename plus a
/// reassignment hand the second lookup somebody else's project.
#[test]
fn project_archive_exports_the_row_id_it_was_given_after_an_identifier_swap() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    // The ID a caller was authorized for, captured before the swap.
    let authorized: i64 = {
        let c = pool.read().unwrap();
        resolve_project(&c, "LIF").unwrap()
    };
    {
        let c = pool.write().unwrap();
        c.execute_batch(
            "UPDATE projects SET identifier='OLD' WHERE id=10;
             UPDATE projects SET identifier='LIF' WHERE id=20;",
        )
        .unwrap();
    }

    let out = dir.path().join("swapped.tar.gz");
    let report =
        export_by_id_with(&pool, &store, authorized, &out, Limits::WEB, &|_| Ok(())).unwrap();
    assert_eq!(report.project, "OLD", "the exported project is the ID's");

    let staged = stage(&out).unwrap();
    let json = serde_json::to_string(&staged.manifest).unwrap();
    assert!(!json.contains("PRIVATE NEIGHBOR"));
    assert!(!json.contains("PRIVATE ISSUE"));
    assert!(json.contains("Portable issue"));

    // The identifier path is the one that follows the label, and now lands on
    // the other project. That is exactly why the web export does not use it.
    let other = dir.path().join("by-name.tar.gz");
    let report = export_with(&pool, &store, "LIF", &other, Limits::WEB, &|_| Ok(())).unwrap();
    assert_eq!(report.project, "LIF");
    assert_ne!(
        report.rows,
        std::fs::metadata(&out).unwrap().len() as usize,
        "the two exports are different projects"
    );
}

#[test]
fn project_archive_export_by_id_refuses_a_project_that_disappeared() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let error = export_by_id_with(
        &pool,
        &store,
        9_999,
        &dir.path().join("gone.tar.gz"),
        Limits::WEB,
        &|_| Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("project not found"), "{error}");
}

/// A ceiling is a resource answer (413); a broken archive is a validity answer
/// (400). The classification never comes from the archive author's text.
#[test]
fn project_archive_resource_ceilings_are_payload_too_large_and_invalid_data_is_not() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let path = dir.path().join("archive.tar.gz");
    export(&pool, &store, "LIF", &path).unwrap();
    let (dest, _dest_dir, dest_store) = fixture();

    let budget = [
        Limits {
            max_rows: 3,
            ..Limits::CLI
        },
        Limits {
            max_metadata: 16,
            ..Limits::CLI
        },
        Limits {
            max_blob: 4,
            ..Limits::CLI
        },
        Limits {
            max_blob_total: 4,
            ..Limits::CLI
        },
        Limits {
            max_blobs: 0,
            ..Limits::CLI
        },
        Limits {
            max_compressed: 8,
            ..Limits::CLI
        },
        Limits {
            max_expanded: 64,
            ..Limits::CLI
        },
    ];
    for profile in budget {
        let error = import_with(&dest, &dest_store, &path, profile, &|tx| {
            cli_grant(tx, "owner")
        })
        .unwrap_err();
        assert!(
            matches!(error, LificError::PayloadTooLarge(_)),
            "{profile:?} should be a resource answer, got {error}"
        );
        let error = export_with(
            &pool,
            &store,
            "LIF",
            &dir.path().join("budget.tar.gz"),
            profile,
            &|_| Ok(()),
        )
        .unwrap_err();
        assert!(
            matches!(error, LificError::PayloadTooLarge(_)),
            "{profile:?} on export should be a resource answer, got {error}"
        );
    }

    // Malformed input keeps its 400 whatever the profile is.
    let broken = dir.path().join("broken.tar.gz");
    std::fs::write(&broken, b"not a gzip stream").unwrap();
    assert!(matches!(
        import_with(&dest, &dest_store, &broken, Limits::WEB, &|tx| cli_grant(
            tx, "owner"
        ))
        .unwrap_err(),
        LificError::BadRequest(_)
    ));

    let mut m = {
        let c = pool.read().unwrap();
        collect_manifest(&c, "LIF").unwrap()
    };
    m.format_version = 999;
    assert!(matches!(
        validate_manifest(&m).unwrap_err(),
        LificError::BadRequest(_)
    ));
}

/// The parser cannot carry a typed error, so it raises a marker instead. A
/// hostile archive must not be able to pick its own status code by writing an
/// error-shaped string into its data.
#[test]
fn project_archive_parser_budget_trips_are_typed_by_marker_not_by_message() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let path = dir.path().join("archive.tar.gz");
    export(&pool, &store, "LIF", &path).unwrap();
    let (dest, _dest_dir, dest_store) = fixture();

    // Only the deserializer can reject this: the row cap is reached while the
    // manifest is being parsed, long before validate_manifest runs.
    let error = import_with(
        &dest,
        &dest_store,
        &path,
        Limits {
            max_rows: 1,
            ..Limits::CLI
        },
        &|tx| cli_grant(tx, "owner"),
    )
    .unwrap_err();
    assert!(matches!(error, LificError::PayloadTooLarge(_)), "{error}");

    // A project whose text says "too many rows" is still ordinary data, and a
    // structural fault in the same archive is still a 400.
    {
        let c = pool.write().unwrap();
        c.execute(
            "UPDATE issues SET title='too many rows: too many list entries' WHERE id=30",
            [],
        )
        .unwrap();
    }
    let claims = dir.path().join("claims.tar.gz");
    export(&pool, &store, "LIF", &claims).unwrap();
    let (second, _second_dir, second_store) = fixture();
    import_with(&second, &second_store, &claims, Limits::CLI, &|tx| {
        cli_grant(tx, "owner")
    })
    .unwrap();

    let mut m = {
        let c = pool.read().unwrap();
        collect_manifest(&c, "LIF").unwrap()
    };
    m.tables[0].rows[0].push("extra".into());
    assert!(matches!(
        validate_manifest(&m).unwrap_err(),
        LificError::BadRequest(_)
    ));
}

/// The expansion budget is a ceiling, not a strict bound: an archive whose
/// decompressed size is exactly the limit is legal. One byte more is a
/// resource answer, never a "this file is broken" answer.
#[test]
fn project_archive_accepts_an_archive_whose_expansion_is_exactly_the_limit() {
    let (pool, dir, store) = fixture();
    seed(&pool, &store);
    let path = dir.path().join("archive.tar.gz");
    export(&pool, &store, "LIF", &path).unwrap();

    // The real decompressed length of this archive, measured rather than
    // assumed, so the boundary under test is the actual one.
    let exact = {
        let mut bytes = Vec::new();
        flate2::bufread::GzDecoder::new(BufReader::new(File::open(&path).unwrap()))
            .read_to_end(&mut bytes)
            .unwrap();
        bytes.len() as u64
    };
    assert!(exact > 0);

    let (dest, _dest_dir, dest_store) = fixture();
    import_with(
        &dest,
        &dest_store,
        &path,
        Limits {
            max_expanded: exact,
            ..Limits::CLI
        },
        &|tx| cli_grant(tx, "owner"),
    )
    .expect("an archive that fits exactly is accepted");

    let (tight, _tight_dir, tight_store) = fixture();
    let error = import_with(
        &tight,
        &tight_store,
        &path,
        Limits {
            max_expanded: exact - 1,
            ..Limits::CLI
        },
        &|tx| cli_grant(tx, "owner"),
    )
    .unwrap_err();
    assert!(
        matches!(error, LificError::PayloadTooLarge(_)),
        "one byte over the budget is a resource answer, got {error}"
    );
    let conn = tight.read().unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
