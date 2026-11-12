//! OAuth Client ID Metadata Documents (MCP 2025-11-25,
//! draft-ietf-oauth-client-id-metadata-document).
//!
//! A client may use an HTTPS URL as its `client_id` instead of registering.
//! The URL serves the client's metadata as JSON. Lific fetches it when a
//! signed-in user opens the consent page, validates it, and keeps it as an
//! ordinary `oauth_clients` row, so the rest of the authorization-code flow
//! (consent, code, token) is unchanged. The row is refetched once it is older
//! than [`CACHE_SECONDS`].
//!
//! Fetching a URL a stranger chose is a server-side request forgery risk, so
//! the host must resolve only to public addresses, the connection is pinned
//! to the address that was checked (no second DNS lookup to rebind), redirects
//! are not followed, and the response is bounded in time and size.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;

/// How long a fetched document stands in for the URL.
pub(crate) const CACHE_SECONDS: i64 = 60 * 60;
/// Largest metadata document accepted, in bytes.
pub(crate) const MAX_DOCUMENT_BYTES: usize = 16 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether `client_id` names a metadata document rather than a registration.
pub(crate) fn is_document_client_id(client_id: &str) -> bool {
    client_id.starts_with("https://")
}

/// The host that published a document client's metadata, which the consent
/// page names next to the self-declared client name.
pub(crate) fn document_host(client_id: &str) -> Option<String> {
    if !is_document_client_id(client_id) {
        return None;
    }
    reqwest::Url::parse(client_id)
        .ok()?
        .host_str()
        .map(str::to_owned)
}

/// How a document is fetched. Production resolves with the system resolver
/// and fetches over HTTPS; tests substitute both.
pub(crate) trait DocumentSource: Send + Sync {
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<IpAddr>, String>> + Send + '_>>;

    /// GET `url` from `address` without following redirects, returning at
    /// most `MAX_DOCUMENT_BYTES + 1` bytes of a 200 JSON body.
    fn fetch(
        &self,
        url: &reqwest::Url,
        address: SocketAddr,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + Send + '_>>;
}

pub(crate) type Source = Arc<dyn DocumentSource>;

/// The real source: system DNS, then one pinned HTTPS GET.
pub(crate) struct HttpsSource;

impl DocumentSource for HttpsSource {
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<IpAddr>, String>> + Send + '_>>
    {
        let target = format!("{host}:{port}");
        Box::pin(async move {
            let addresses = tokio::time::timeout(FETCH_TIMEOUT, tokio::net::lookup_host(target))
                .await
                .map_err(|_| "DNS lookup timed out".to_owned())?
                .map_err(|error| format!("DNS lookup failed: {error}"))?;
            Ok(addresses.map(|address| address.ip()).collect())
        })
    }

    fn fetch(
        &self,
        url: &reqwest::Url,
        address: SocketAddr,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + Send + '_>>
    {
        let url = url.clone();
        Box::pin(async move {
            let host = url.host_str().unwrap_or_default().to_owned();
            let client = reqwest::Client::builder()
                .resolve(&host, address)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(FETCH_TIMEOUT)
                .connect_timeout(FETCH_TIMEOUT)
                .build()
                .map_err(|error| format!("client setup failed: {error}"))?;
            let mut response = client
                .get(url)
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await
                .map_err(|error| format!("fetch failed: {error}"))?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(format!("answered HTTP {}", response.status()));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| format!("read failed: {error}"))?
            {
                body.extend_from_slice(&chunk);
                if body.len() > MAX_DOCUMENT_BYTES {
                    break;
                }
            }
            Ok(body)
        })
    }
}

/// Check the URL's own shape: HTTPS, a host, a path, and none of the parts
/// the specification forbids.
fn parse_client_id(client_id: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(client_id).map_err(|_| "client_id is not a valid URL")?;
    if url.scheme() != "https" {
        return Err("a client_id URL must use https".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("a client_id URL needs a host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("a client_id URL must not carry credentials".into());
    }
    if url.fragment().is_some() || client_id.contains('#') {
        return Err("a client_id URL must not have a fragment".into());
    }
    if url.path() == "/" || url.path().is_empty() {
        return Err("a client_id URL must have a path".into());
    }
    // The parser normalizes dot segments away, so compare against the raw
    // string: the document must name exactly the URL the client sent.
    if url.as_str() != client_id
        || client_id
            .split(['/', '?'])
            .any(|segment| segment == "." || segment == "..")
    {
        return Err("a client_id URL must be in normal form, without dot segments".into());
    }
    Ok(url)
}

/// Addresses a fetch may never reach: loopback, private, link-local, shared,
/// multicast, documentation and other special-purpose ranges.
pub(crate) fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let first = v6.segments()[0];
            !(
                v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00 // unique local
                || (first & 0xffc0) == 0xfe80 // link local
                || (first & 0xffc0) == 0xfec0 // site local (deprecated)
                || first == 0x2001 && v6.segments()[1] == 0x0db8 // documentation
                || first == 0x0064 && v6.segments()[1] == 0xff9b // NAT64
                || first == 0x2002 // 6to4 embeds an IPv4 address
                || first == 0
                // ::/16 holds compatible and other special forms
            )
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // shared address space
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || a >= 240)
}

#[derive(Deserialize)]
struct Document {
    client_id: String,
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    grant_types: Option<Vec<String>>,
    #[serde(default)]
    client_secret: Option<serde_json::Value>,
}

/// A validated document, ready to store.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ClientMetadata {
    pub(crate) client_name: String,
    pub(crate) redirect_uris: Vec<String>,
}

fn validate_document(client_id: &str, host: &str, body: &[u8]) -> Result<ClientMetadata, String> {
    if body.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "the metadata document is over {MAX_DOCUMENT_BYTES} bytes"
        ));
    }
    let document: Document = serde_json::from_slice(body)
        .map_err(|_| "the metadata document is not a JSON client metadata object")?;
    if document.client_id != client_id {
        return Err("the metadata document's client_id does not match its URL".into());
    }
    if document.client_secret.is_some() {
        return Err("a metadata document must not contain a client_secret".into());
    }
    if document
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|method| method != "none")
    {
        return Err("only public clients (token_endpoint_auth_method none) are supported".into());
    }
    if document
        .grant_types
        .as_ref()
        .is_some_and(|grants| !grants.iter().any(|grant| grant == "authorization_code"))
    {
        return Err("the metadata document does not allow the authorization_code grant".into());
    }
    if document.redirect_uris.is_empty() || document.redirect_uris.len() > super::MAX_REDIRECT_URIS
    {
        return Err(format!(
            "the metadata document needs 1 to {} redirect_uris",
            super::MAX_REDIRECT_URIS
        ));
    }
    for uri in &document.redirect_uris {
        if uri.len() > super::MAX_REDIRECT_URI_BYTES {
            return Err("a redirect_uri in the metadata document is too long".into());
        }
        super::validate_redirect_uri(uri).map_err(str::to_owned)?;
    }
    let client_name = document
        .client_name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| host.to_owned());
    if client_name.len() > super::MAX_CLIENT_NAME_BYTES || client_name.chars().any(char::is_control)
    {
        return Err(
            "the metadata document's client_name is too long or has control characters".into(),
        );
    }
    Ok(ClientMetadata {
        client_name,
        redirect_uris: document.redirect_uris,
    })
}

/// Fetch and validate the document behind `client_id`. Nothing is fetched
/// unless every address the host resolves to is public.
pub(crate) async fn load(
    source: &dyn DocumentSource,
    client_id: &str,
) -> Result<ClientMetadata, String> {
    let url = parse_client_id(client_id)?;
    let host = url.host_str().unwrap_or_default().to_owned();
    let port = url.port_or_known_default().unwrap_or(443);
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    let addresses = match literal.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => source.resolve(&host, port).await?,
    };
    if addresses.is_empty() {
        return Err(format!("{host} does not resolve"));
    }
    if let Some(blocked) = addresses.iter().find(|ip| !is_public_address(**ip)) {
        return Err(format!(
            "{host} resolves to a non-public address ({blocked}); refusing to fetch it"
        ));
    }
    let body = source
        .fetch(&url, SocketAddr::new(addresses[0], port))
        .await
        .map_err(|error| format!("could not fetch the client metadata document: {error}"))?;
    validate_document(client_id, &host, &body)
}

/// A cached document row younger than [`CACHE_SECONDS`], if any.
pub(crate) fn is_fresh(conn: &Connection, client_id: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row(
            "SELECT created_at > datetime('now', ?2) FROM oauth_clients WHERE client_id = ?1",
            params![client_id, format!("-{CACHE_SECONDS} seconds")],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

/// Store `metadata` as the client's row. For a document client `created_at`
/// is the time it was last fetched, which is what the cache and the unused
/// client sweep both want.
pub(crate) fn store(
    conn: &Connection,
    client_id: &str,
    metadata: &ClientMetadata,
) -> rusqlite::Result<()> {
    let redirect_uris =
        serde_json::to_string(&metadata.redirect_uris).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO oauth_clients (client_id, client_name, redirect_uris, created_at)
         VALUES (?1, ?2, ?3, datetime('now'))
         ON CONFLICT(client_id) DO UPDATE SET
             client_name = excluded.client_name,
             redirect_uris = excluded.redirect_uris,
             created_at = excluded.created_at",
        params![client_id, metadata.client_name, redirect_uris],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A source with canned DNS answers and documents, recording fetches.
    #[derive(Default)]
    pub(crate) struct FakeSource {
        pub(crate) dns: HashMap<String, Vec<IpAddr>>,
        pub(crate) documents: HashMap<String, Vec<u8>>,
        pub(crate) fetched: Mutex<Vec<String>>,
    }

    impl DocumentSource for FakeSource {
        fn resolve(
            &self,
            host: &str,
            _port: u16,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Vec<IpAddr>, String>> + Send + '_>,
        > {
            let answer = self
                .dns
                .get(host)
                .cloned()
                .ok_or_else(|| format!("no DNS answer for {host}"));
            Box::pin(async move { answer })
        }

        fn fetch(
            &self,
            url: &reqwest::Url,
            _address: SocketAddr,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + Send + '_>>
        {
            self.fetched.lock().unwrap().push(url.to_string());
            let body = self
                .documents
                .get(url.as_str())
                .cloned()
                .ok_or_else(|| "answered HTTP 404 Not Found".to_owned());
            Box::pin(async move { body })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeSource;
    use super::*;

    const ID: &str = "https://client.example/oauth/metadata.json";

    fn source(document: serde_json::Value) -> FakeSource {
        let mut source = FakeSource::default();
        source.dns.insert(
            "client.example".into(),
            vec!["93.184.216.34".parse().unwrap()],
        );
        source
            .documents
            .insert(ID.into(), serde_json::to_vec(&document).unwrap());
        source
    }

    #[tokio::test]
    async fn a_valid_document_loads() {
        let source = source(serde_json::json!({
            "client_id": ID,
            "client_name": "Example Client",
            "redirect_uris": ["http://127.0.0.1:7777/callback"],
            "token_endpoint_auth_method": "none"
        }));
        assert_eq!(
            load(&source, ID).await.unwrap(),
            ClientMetadata {
                client_name: "Example Client".into(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".into()],
            }
        );
    }

    #[tokio::test]
    async fn documents_that_misdescribe_themselves_are_refused() {
        for (document, expected) in [
            (
                serde_json::json!({"client_id": "https://other.example/x", "redirect_uris": ["https://a.example/cb"]}),
                "does not match",
            ),
            (
                serde_json::json!({"client_id": ID, "redirect_uris": []}),
                "redirect_uris",
            ),
            (
                serde_json::json!({"client_id": ID, "redirect_uris": ["https://a.example/cb"], "client_secret": "s"}),
                "client_secret",
            ),
            (
                serde_json::json!({"client_id": ID, "redirect_uris": ["https://a.example/cb"], "token_endpoint_auth_method": "private_key_jwt"}),
                "public clients",
            ),
            (
                serde_json::json!({"client_id": ID, "redirect_uris": ["javascript:alert(1)"]}),
                "http or https",
            ),
        ] {
            let error = load(&source(document), ID).await.unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[tokio::test]
    async fn private_and_malformed_client_ids_are_refused_without_a_fetch() {
        let mut source = FakeSource::default();
        source
            .dns
            .insert("internal.example".into(), vec!["10.0.0.5".parse().unwrap()]);
        source.dns.insert(
            "mixed.example".into(),
            vec!["93.184.216.34".parse().unwrap(), "::1".parse().unwrap()],
        );
        for client_id in [
            "https://internal.example/meta.json",
            "https://mixed.example/meta.json",
            "https://127.0.0.1/meta.json",
            "https://[::ffff:192.168.1.1]/meta.json",
            "https://169.254.169.254/latest/meta-data",
            "http://client.example/meta.json",
            "https://client.example/",
            "https://client.example/a/../meta.json",
            "https://user:pw@client.example/meta.json",
            "https://client.example/meta.json#x",
        ] {
            let error = load(&source, client_id).await.unwrap_err();
            assert!(!error.is_empty(), "{client_id}");
        }
        assert!(source.fetched.lock().unwrap().is_empty());
    }

    #[test]
    fn public_address_classification() {
        for public in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public_address(public.parse().unwrap()), "{public}");
        }
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "2001:db8::1",
        ] {
            assert!(!is_public_address(private.parse().unwrap()), "{private}");
        }
    }
}
