use super::*;
use crate::api::test_helpers::*;
use crate::db::models::AuthUser;
use crate::storage::fixtures::png_image;
use axum::{Router, body::Body, http::StatusCode, routing::get};
use std::sync::Mutex;

struct Source {
    addresses: Vec<IpAddr>,
    bytes: Vec<u8>,
    mime: Option<String>,
    fetched: Mutex<Vec<SocketAddr>>,
    stall: bool,
    resolve_error: bool,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            addresses: vec!["93.184.216.34".parse().unwrap()],
            bytes: png_image(2, 3),
            mime: Some("text/html".into()),
            fetched: Mutex::new(Vec::new()),
            stall: false,
            resolve_error: false,
        }
    }
}

impl ImageSource for Source {
    fn resolve(&self, _: String, _: u16) -> BoxFuture<'_, Result<Vec<IpAddr>, String>> {
        Box::pin(async move {
            if self.stall {
                std::future::pending::<()>().await;
            }
            if self.resolve_error {
                return Err("internal resolver diagnostic".into());
            }
            Ok(self.addresses.clone())
        })
    }

    fn fetch(
        &self,
        _: reqwest::Url,
        address: SocketAddr,
        _: usize,
    ) -> BoxFuture<'_, Result<ImageBody, String>> {
        self.fetched.lock().unwrap().push(address);
        Box::pin(std::future::ready(Ok((
            self.bytes.clone(),
            self.mime.clone(),
        ))))
    }
}

const URL: &str = "https://images.example/screen.png?size=large";

#[tokio::test]
async fn unresolved_and_non_public_hosts_have_the_same_error() {
    let mut source = Source {
        resolve_error: true,
        ..Source::default()
    };
    assert_eq!(
        load(&source, URL, 1024).await.unwrap_err(),
        IMAGE_HOST_UNAVAILABLE
    );
    source.resolve_error = false;
    for addresses in [vec![], vec!["10.0.0.1".parse().unwrap()]] {
        source.addresses = addresses;
        assert_eq!(
            load(&source, URL, 1024).await.unwrap_err(),
            IMAGE_HOST_UNAVAILABLE
        );
    }
    assert_eq!(
        load(&source, "http://127.0.0.1/a.png", 1024)
            .await
            .unwrap_err(),
        IMAGE_HOST_UNAVAILABLE
    );
    assert!(source.fetched.lock().unwrap().is_empty());
}

#[tokio::test]
async fn public_image_uses_checked_address_and_byte_detected_mime() {
    let source = Source::default();
    let image = load(&source, URL, 1024).await.unwrap();
    assert_eq!(image.mime, "image/png");
    assert_eq!(image.filename, "screen.png");
    assert_eq!(image.bytes, source.bytes);
    assert_eq!(
        *source.fetched.lock().unwrap(),
        ["93.184.216.34:443".parse().unwrap()]
    );
}

#[tokio::test]
async fn private_literals_and_dns_answers_never_reach_fetch() {
    let mut source = Source::default();
    for host in [
        "127.0.0.1",
        "127.1",
        "2130706433",
        "0x7f000001",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.169.254",
        "0.0.0.0",
        "100.64.0.1",
        "[::1]",
        "[::ffff:127.0.0.1]",
        "[fc00::1]",
        "[fe80::1]",
        "[64:ff9b::a00:1]",
        "[2002:a00:1::]",
        "[2001::1]",
    ] {
        assert!(
            load(&source, &format!("http://{host}/image.png"), 1024)
                .await
                .is_err(),
            "{host}"
        );
    }
    for ips in [
        vec![],
        vec!["127.0.0.1"],
        vec!["10.0.0.1"],
        vec!["169.254.169.254"],
        vec!["93.184.216.34", "::1"],
        vec!["93.184.216.34", "192.168.0.1"],
    ] {
        source.addresses = ips.iter().map(|ip| ip.parse().unwrap()).collect();
        assert!(load(&source, URL, 1024).await.is_err(), "{ips:?}");
    }
    assert!(source.fetched.lock().unwrap().is_empty());
}

#[tokio::test]
async fn invalid_urls_and_credentials_are_refused() {
    let source = Source::default();
    for url in [
        "file:///etc/passwd",
        "ftp://example.com/a.png",
        "not a url",
        "https://user:secret@example.com/a.png",
        "https://example.com@127.0.0.1/a.png",
    ] {
        assert!(load(&source, url, 1024).await.is_err(), "{url}");
    }
    assert!(
        load(
            &source,
            &format!("https://example.com/{}", "a".repeat(8192)),
            1024
        )
        .await
        .is_err()
    );
    assert!(source.fetched.lock().unwrap().is_empty());
}

#[tokio::test]
async fn oversize_empty_and_non_images_are_refused_despite_image_headers() {
    let mut source = Source {
        mime: Some("image/png".into()),
        ..Source::default()
    };
    assert!(
        load(&source, URL, 2)
            .await
            .unwrap_err()
            .contains("too large")
    );
    for bytes in [
        vec![],
        b"<html>not an image</html>".to_vec(),
        b"%PDF-1.7 document".to_vec(),
        b"\x89PNG\r\n\x1a\nbroken".to_vec(),
    ] {
        source.bytes = bytes;
        assert!(load(&source, URL, 1024).await.is_err());
    }
    source.mime = Some("image/svg+xml".into());
    source.bytes = br#"<?xml version="1.0"?><document>not an image</document>"#.to_vec();
    assert!(load(&source, URL, 1024).await.is_err());
    source.bytes =
        br#"<?xml version="1.0"?><!-- image --><svg xmlns="http://www.w3.org/2000/svg"/>"#.to_vec();
    assert_eq!(
        load(&source, URL, 1024).await.unwrap().mime,
        "image/svg+xml"
    );
}

#[tokio::test(start_paused = true)]
async fn deadline_includes_dns_resolution() {
    let source = Source {
        stall: true,
        ..Source::default()
    };
    assert!(
        load(&source, URL, 1024)
            .await
            .unwrap_err()
            .contains("timed out")
    );
    assert!(source.fetched.lock().unwrap().is_empty());
}

async fn serve(
    router: Router,
) -> (
    SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });
    (addr, tx, task)
}

#[tokio::test]
async fn transport_refuses_all_redirects_without_contacting_target() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let target = Router::new().route(
        "/private.png",
        get(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            std::future::ready("private data")
        }),
    );
    let (private, stop_private, private_task) = serve(target).await;
    for status in [301, 302, 303, 307, 308] {
        let location = format!("http://{private}/private.png");
        let router = Router::new().route(
            "/image.png",
            get(move || {
                std::future::ready((
                    StatusCode::from_u16(status).unwrap(),
                    [("location", location.clone())],
                ))
            }),
        );
        let (addr, stop, task) = serve(router).await;
        let url = format!("http://images.example:{}/image.png", addr.port())
            .parse()
            .unwrap();
        let result = fetch_image(url, addr, 1024, FETCH_TIMEOUT).await;
        assert!(result.unwrap_err().contains("redirects"));
        stop.send(()).unwrap();
        task.await.unwrap();
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    stop_private.send(()).unwrap();
    private_task.await.unwrap();
}

#[tokio::test]
async fn transport_caps_declared_and_streamed_size_and_stalled_body() {
    use futures_util::{StreamExt, stream};
    let router = Router::new()
        .route("/length", get(|| std::future::ready(vec![1u8; 2048])))
        .route(
            "/chunked",
            get(|| {
                std::future::ready(Body::from_stream(stream::iter([
                    Ok::<_, std::io::Error>(vec![1u8; 600]),
                    Ok(vec![2u8; 600]),
                ])))
            }),
        )
        .route(
            "/stall",
            get(|| {
                std::future::ready(Body::from_stream(
                    stream::once(std::future::ready(Ok::<_, std::io::Error>(vec![1u8])))
                        .chain(stream::pending()),
                ))
            }),
        );
    let (addr, stop, task) = serve(router).await;
    for path in ["length", "chunked", "stall"] {
        let url = format!("http://images.example:{}/{path}", addr.port())
            .parse()
            .unwrap();
        let result = fetch_image(url, addr, 1024, Duration::from_millis(150)).await;
        let error = result.unwrap_err();
        assert!(
            error.contains(if path == "stall" {
                "timed out"
            } else {
                "too large"
            }),
            "{error}"
        );
    }
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn api_refuses_private_urls_and_malformed_link_pairs() {
    let app = test_app();
    for body in [
        serde_json::json!({"url": "http://127.0.0.1/image.png"}),
        serde_json::json!({"url": "http://169.254.169.254/image.png"}),
        serde_json::json!({"url": URL, "entity_type": "issue"}),
        serde_json::json!({"url": URL, "entity_id": 1}),
    ] {
        let response = json_post(&app, "/api/attachments/import", body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn import_stores_bytes_metadata_and_entity_link_and_checks_permissions_first() {
    let db = crate::db::open_memory().unwrap();
    let (store, _tmp) = test_attachment_store();
    let identity = {
        let conn = db.write().unwrap();
        conn.execute(
            "INSERT INTO users (id, username, email, password_hash, display_name, is_admin)
            VALUES (1, 'u', 'u@example.com', 'x', 'U', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO users (id, username, email, password_hash, display_name, is_admin)
             VALUES (99, 'outsider', 'outsider@example.com', 'x', 'Outsider', 0)",
            [],
        )
        .unwrap();
        Some(ResolvedIdentity {
            user: AuthUser {
                id: 1,
                username: "u".into(),
                display_name: "U".into(),
                is_admin: true,
            },
            transport: crate::actor::Transport::Web,
        })
    };
    let realtime = RealtimeHub::new();
    let app = with_attachment_layers_store(crate::api::router(db.clone(), &[]), store.clone())
        .layer(Extension(identity.clone()))
        .layer(Extension(realtime.clone()));
    let (project, _) = seed_project(&app).await;
    let issue = parse_json(
        json_post(
            &app,
            "/api/issues",
            serde_json::json!({
                "project_id": project, "title": "Imported screenshot"
            }),
        )
        .await,
    )
    .await;
    let issue_id = issue["id"].as_i64().unwrap();
    let source = Source::default();
    let config = AttachmentConfig::default();
    let limiter = AttachmentUploadLimiter(crate::ratelimit::RateLimiter::new(10, FETCH_TIMEOUT));
    let body = || ImportBody {
        url: URL.into(),
        entity_type: Some(AttachmentEntity::Issue),
        entity_id: Some(issue_id),
    };
    let result = import_from(
        &source,
        &db,
        &identity,
        &realtime,
        &store,
        &config,
        &limiter,
        body(),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(result.mime, "image/png");
    assert_eq!((result.width, result.height), (Some(2), Some(3)));
    let row =
        crate::db::queries::attachments::get_attachment(&db.read().unwrap(), result.id).unwrap();
    assert_eq!(store.read(&row.sha256).unwrap(), source.bytes);
    assert_eq!(row.uploader_id, Some(1));
    assert_eq!(
        crate::db::queries::attachments::list_for_entity(
            &db.read().unwrap(),
            AttachmentEntity::Issue,
            issue_id
        )
        .unwrap()
        .len(),
        1
    );
    assert_eq!(result.url, format!("/api/attachments/{}", result.id));
    source.fetched.lock().unwrap().clear();
    assert!(
        import_from(
            &source,
            &db,
            &None,
            &realtime,
            &store,
            &config,
            &limiter,
            body()
        )
        .await
        .is_err()
    );
    let mut outsider = identity.clone().unwrap();
    outsider.user.id = 99;
    outsider.user.is_admin = false;
    let outsider = Some(outsider);
    assert!(
        import_from(
            &source,
            &db,
            &outsider,
            &realtime,
            &store,
            &config,
            &limiter,
            body()
        )
        .await
        .is_err()
    );
    let exhausted = AttachmentUploadLimiter(crate::ratelimit::RateLimiter::new(0, FETCH_TIMEOUT));
    assert!(
        import_from(
            &source,
            &db,
            &identity,
            &realtime,
            &store,
            &config,
            &exhausted,
            body()
        )
        .await
        .is_err()
    );
    assert!(source.fetched.lock().unwrap().is_empty());
    let unlinked = import_from(
        &source,
        &db,
        &outsider,
        &realtime,
        &store,
        &config,
        &limiter,
        ImportBody {
            url: URL.into(),
            entity_type: None,
            entity_id: None,
        },
    )
    .await
    .unwrap()
    .0;
    let row =
        crate::db::queries::attachments::get_attachment(&db.read().unwrap(), unlinked.id).unwrap();
    assert_eq!(row.uploader_id, Some(99));
}
