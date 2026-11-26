use super::*;
use http_body_util::BodyExt;

fn reads() -> AnonymousReads {
    AnonymousReads::new(Arc::from(Vec::<IpNetwork>::new()))
}

// ── The allowlist ───────────────────────────────────────────────

#[test]
fn published_reads_are_allowed_without_a_credential() {
    for path in [
        "/api/projects",
        "/api/projects/3",
        "/api/projects/3/index",
        "/api/issues",
        "/api/issues/12",
        "/api/issues/resolve/LIF-1",
        "/api/issues/12/comments",
        "/api/issues/12/waits",
        "/api/pages/resolve/LIF-DOC-1",
        "/api/plans/resolve/LIF-PLAN-1",
        "/api/plans/4/activity",
        "/api/attachments/9",
        "/api/attachments/9/thumbnail",
        "/api/search",
    ] {
        assert!(is_anonymous_read(&Method::GET, path), "{path}");
    }
}

#[test]
fn only_get_is_ever_anonymous() {
    for method in [
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::HEAD,
        Method::OPTIONS,
    ] {
        for path in ["/api/issues", "/api/issues/12", "/api/issues/12/comments"] {
            assert!(!is_anonymous_read(&method, path), "{method} {path}");
        }
    }
}

#[test]
fn rosters_accounts_settings_and_exports_stay_closed() {
    for path in [
        "/api/projects/3/members",
        "/api/projects/3/mention-candidates",
        "/api/projects/3/views",
        "/api/users",
        "/api/auth/me",
        "/api/auth/keys",
        "/api/settings",
        "/api/home",
        "/api/inbox",
        "/api/export/project/LIF",
        "/api/events/ws",
        "/mcp",
    ] {
        assert!(!is_anonymous_read(&Method::GET, path), "{path}");
    }
}

#[test]
fn a_pattern_matches_whole_segments_only() {
    assert!(!is_anonymous_read(&Method::GET, "/api/issues/abc"));
    assert!(!is_anonymous_read(&Method::GET, "/api/issues/12/"));
    assert!(!is_anonymous_read(&Method::GET, "/api/issues//comments"));
    assert!(!is_anonymous_read(
        &Method::GET,
        "/api/issues/12/comments/extra"
    ));
    assert!(!is_anonymous_read(&Method::GET, "/api/issues/resolve/"));
    assert!(!is_anonymous_read(&Method::GET, "/api/projectsX"));
    assert!(!is_anonymous_read(&Method::GET, "/api"));
}

// ── Budgets ─────────────────────────────────────────────────────

#[test]
fn one_client_is_rate_limited_and_another_is_not() {
    let reads = AnonymousReads::with_limits(Arc::from(Vec::<IpNetwork>::new()), 2, 4, 4, 1);
    let first: SocketAddr = "203.0.113.1:1000".parse().unwrap();
    let second: SocketAddr = "203.0.113.2:1000".parse().unwrap();
    let headers = HeaderMap::new();
    assert!(reads.admit(Some(first), &headers).is_ok());
    assert!(reads.admit(Some(first), &headers).is_ok());
    let refused = reads.admit(Some(first), &headers).unwrap_err();
    assert!(matches!(refused, LificError::TooManyRequests(_)));
    let response = refused.into_response();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().get(header::RETRY_AFTER).is_some());
    assert!(reads.admit(Some(second), &headers).is_ok());
}

/// A forwarding header from an untrusted peer is ignored, so a client
/// cannot mint a fresh bucket per request.
#[test]
fn a_forged_forwarding_header_does_not_buy_a_new_bucket() {
    let reads = AnonymousReads::with_limits(Arc::from(Vec::<IpNetwork>::new()), 1, 4, 4, 1);
    let peer: SocketAddr = "203.0.113.1:1000".parse().unwrap();
    let mut headers = HeaderMap::new();
    assert!(reads.admit(Some(peer), &headers).is_ok());
    headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.7"));
    assert!(reads.admit(Some(peer), &headers).is_err());
}

#[tokio::test]
async fn a_full_instance_answers_503_with_retry_after() {
    let reads = AnonymousReads::with_limits(Arc::from(Vec::<IpNetwork>::new()), 240, 1, 4, 1);
    let held = acquire_read_slot(&reads).await.unwrap();
    let refused = acquire_read_slot(&reads).await.unwrap_err();
    let response = refused.into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(response.headers().get(header::RETRY_AFTER).is_some());
    drop(held);
    assert!(acquire_read_slot(&reads).await.is_ok());
}

#[test]
fn production_budgets_match_the_documented_limits() {
    let reads = reads();
    assert_eq!(READS_PER_MINUTE, 240);
    assert_eq!(reads.concurrency().available_permits(), 4);
    assert_eq!(reads.downloads().available_permits(), 4);
    assert_eq!(reads.derivations().available_permits(), 1);
    assert_eq!(DERIVE_MAX_BYTES, 32 * 1024 * 1024);
}

fn attachment(size_bytes: i64) -> Attachment {
    serde_json::from_value(serde_json::json!({
        "id": 1,
        "sha256": "0".repeat(64),
        "filename": "a.png",
        "mime": "image/png",
        "size_bytes": size_bytes,
        "uploader_id": null,
        "created_at": "2026-01-01T00:00:00Z",
    }))
    .expect("attachment fixture")
}

#[test]
fn derived_views_are_refused_above_the_size_ceiling() {
    let reads = reads();
    let refused = derivation_permit(&reads, &attachment(DERIVE_MAX_BYTES + 1)).unwrap_err();
    assert!(matches!(refused, LificError::NotFound(_)));
    assert!(derivation_permit(&reads, &attachment(DERIVE_MAX_BYTES)).is_ok());
}

#[test]
fn derived_views_are_refused_while_the_budget_is_busy() {
    let reads = reads();
    let held = derivation_permit(&reads, &attachment(10)).unwrap();
    let refused = derivation_permit(&reads, &attachment(10)).unwrap_err();
    assert!(matches!(refused, LificError::Unavailable(_)));
    drop(held);
    assert!(derivation_permit(&reads, &attachment(10)).is_ok());
}

#[tokio::test]
async fn a_cancelled_derivation_keeps_its_permit_until_the_work_stops() {
    let reads = reads();
    let permit = derivation_permit(&reads, &attachment(10)).unwrap();
    let (started, has_started) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let job = tokio::spawn(run_derivation(permit, move || {
        let _ = started.send(());
        let _ = wait.recv();
        Ok(())
    }));
    has_started.await.unwrap();
    job.abort();
    let _ = job.await;
    assert_eq!(reads.derivations().available_permits(), 0);
    release.send(()).unwrap();
    let _permit = tokio::time::timeout(
        Duration::from_secs(2),
        Arc::clone(reads.derivations()).acquire_owned(),
    )
    .await
    .expect("the permit comes back once the blocking work ends")
    .unwrap();
}

// ── Concealment and headers ─────────────────────────────────────

#[tokio::test]
async fn a_refusal_reads_exactly_like_absence() {
    let forbidden =
        conceal_refusal(LificError::Forbidden("needs viewer on PRIV".into()).into_response());
    let missing = conceal_refusal(LificError::NotFound("issue 9 not found".into()).into_response());
    assert_eq!(forbidden.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let body = |r: Response<Body>| async { r.into_body().collect().await.unwrap().to_bytes() };
    let (forbidden, missing) = (body(forbidden).await, body(missing).await);
    assert_eq!(forbidden, missing);
    assert!(!String::from_utf8_lossy(&forbidden).contains("PRIV"));
}

#[test]
fn every_anonymous_response_is_uncacheable_and_hardened() {
    let mut response = Response::new(Body::empty());
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    let response = harden(response);
    let get = |name: &str| response.headers().get(name).unwrap().to_str().unwrap();
    assert_eq!(get("cache-control"), "no-store, no-cache, must-revalidate");
    assert_eq!(get("x-content-type-options"), "nosniff");
    assert_eq!(get("referrer-policy"), "no-referrer");
    assert_eq!(get("x-frame-options"), "DENY");
    assert_eq!(get("cross-origin-resource-policy"), "same-origin");
}

#[test]
fn download_filenames_cannot_break_out_of_the_header() {
    assert_eq!(header_safe("a\"b\\c\r\nd.png"), "abcd.png");
    assert_eq!(header_safe(&"x".repeat(500)).len(), 200);
}

// ── Streaming downloads ─────────────────────────────────────────

fn test_download(size: usize, idle_ms: u64, max_ms: u64) -> (Body, Arc<Semaphore>) {
    let file = tempfile::tempfile().unwrap();
    file.set_len(size as u64).unwrap();
    let semaphore = Arc::new(Semaphore::new(1));
    let permit = Arc::clone(&semaphore).try_acquire_owned().unwrap();
    let body = download_body(
        file,
        permit,
        Duration::from_millis(idle_ms),
        Duration::from_millis(max_ms),
    );
    (body, semaphore)
}

async fn wait_for_download_release(semaphore: &Arc<Semaphore>) {
    let permit = tokio::time::timeout(
        Duration::from_secs(2),
        Arc::clone(semaphore).acquire_owned(),
    )
    .await
    .expect("producer must release its permit without body polling")
    .unwrap();
    drop(permit);
}

#[tokio::test]
async fn an_unpolled_download_times_out_idle_and_reports_truncation() {
    let (mut body, semaphore) = test_download(DOWNLOAD_CHUNK_BYTES * 4, 20, 5_000);
    wait_for_download_release(&semaphore).await;
    let queued = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(queued.len(), DOWNLOAD_CHUNK_BYTES);
    let error = body.frame().await.unwrap().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("anonymous download idle timeout"),
        "{error}"
    );
    assert!(body.frame().await.is_none());
    assert!(
        body.frame().await.is_none(),
        "terminal polling must remain safe"
    );
}

#[tokio::test]
async fn a_partly_read_download_hits_its_deadline_without_further_polling() {
    let (mut body, semaphore) = test_download(DOWNLOAD_CHUNK_BYTES * 100, 5_000, 200);
    for _ in 0..2 {
        let frame = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert_eq!(frame.len(), DOWNLOAD_CHUNK_BYTES);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(semaphore.available_permits(), 0);
    wait_for_download_release(&semaphore).await;
    let error = body.collect().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("anonymous download deadline exceeded"),
        "{error}"
    );
}

#[tokio::test]
async fn a_fully_buffered_download_releases_its_permit_before_body_consumption() {
    let (body, semaphore) = test_download(17, 5_000, 5_000);
    wait_for_download_release(&semaphore).await;
    assert_eq!(body.collect().await.unwrap().to_bytes().as_ref(), &[0; 17]);
}

#[tokio::test]
async fn dropping_a_download_releases_its_producer_permit() {
    let (mut body, semaphore) = test_download(DOWNLOAD_CHUNK_BYTES * 4, 5_000, 5_000);
    assert!(body.frame().await.unwrap().unwrap().is_data());
    drop(body);
    wait_for_download_release(&semaphore).await;
}
