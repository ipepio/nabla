//! `web_search`: the agent's web search, backed by a SearXNG instance.
//!
//! **Nabla hosts no SearXNG.** The app is a client of one: the instance the human
//! names in Settings, or — when they name none — any public instance from the
//! best-effort list below (`web-search.md`, S1-S4).
//!
//! Two seams live here, and both exist so the dispatcher in `mcp_server.rs` can be
//! tested without a socket:
//!
//! ```text
//! SearchGateway ── what the human configured, and the only place a real
//!                  HTTP searcher is built
//!      └─ Searcher ── one instance, one query in, hits out
//! ```
//!
//! A test implements both with fakes and no request is ever made. The real
//! implementation of `Searcher`, [`SearxngSearcher`], is the only thing in this
//! file that touches the network, and nothing constructs it but
//! [`SettingsSearch::searcher`].
//!
//! **Credentials.** An instance behind nginx Basic auth answers `401` to
//! every anonymous request, so the human may store one optional secret beside the
//! instance (`webSearchCredentials`). It becomes exactly one header
//! ([`authorization_header`]) and it only ever travels to the instance the human
//! configured ([`credentials_for`]): the public fallback walks servers they never
//! named, and a header sent to one of those would hand the secret to a third
//! party.

use std::path::PathBuf;
use std::time::Duration;

use base64::prelude::{BASE64_STANDARD, Engine as _};

/// How long one search may take. Short on purpose: a tool call that hangs is
/// worse than one that answers "the instance did not respond", and the next
/// candidate (or the human's next wording) is a better use of the turn.
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(8);

/// The most hits one call may ask for (S3: `limit` is clamped to 1..=10). Ten
/// blocks of title, url and snippet is already a large answer for a model, and a
/// larger cap only invites a page of noise.
pub const MAX_LIMIT: usize = 10;

/// What one call asks for when the model omits `limit`.
pub const DEFAULT_LIMIT: usize = 5;

/// One result: what the model reads and what `web_search` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// One instance, one query in, hits out.
///
/// `Err` is a message already written for the model: what failed, at which
/// instance, and — where it is knowable — what would fix it. The dispatcher adds
/// the context only it has (a configured instance vs. the public list).
pub trait Searcher: Send + Sync {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, String>;
}

/// What the dispatcher needs from the running app.
///
/// Kept as a trait for the same reason as [`Searcher`]: the setting and the
/// searcher are the two things a test must be able to fake, and neither a
/// `settings.json` nor an HTTP client belongs in a unit test.
pub trait SearchGateway: Send + Sync + 'static {
    /// S1: whether the human turned web search on. Read per call, so a flip in
    /// Settings reaches the very next tool call instead of the next relaunch.
    fn enabled(&self) -> bool;

    /// S1: the configured instance, trimmed; empty means "any public instance".
    fn instance(&self) -> String;

    /// A searcher for one instance. **The only seam a real HTTP client is built
    /// behind**, which is what lets every test above this line run offline.
    fn searcher(&self, instance: &str) -> Box<dyn Searcher>;
}

/// The real gateway: `settings.json` plus [`SearxngSearcher`].
///
/// Holds the resolved preferences path, not the settings: every call re-reads the
/// file, so the toggle in the dialog applies to the next tool call. A file that
/// cannot be read answers **off**: a network capability must never come on
/// because of a parse error.
pub struct SettingsSearch {
    path: PathBuf,
}

impl SettingsSearch {
    /// `config_dir` is the app config directory, and the path is the one
    /// `commands` already resolves — one definition of where the file lives.
    pub fn new(config_dir: &std::path::Path) -> SettingsSearch {
        SettingsSearch {
            path: crate::commands::settings_file(config_dir),
        }
    }
}

impl SearchGateway for SettingsSearch {
    fn enabled(&self) -> bool {
        match crate::settings::load(&self.path) {
            Ok(settings) => settings.web_search_enabled,
            Err(_) => false,
        }
    }

    fn instance(&self) -> String {
        match crate::settings::load(&self.path) {
            Ok(settings) => settings.web_search_instance.trim().to_string(),
            Err(_) => String::new(),
        }
    }

    fn searcher(&self, instance: &str) -> Box<dyn Searcher> {
        // Credentials: they live in the same file as the instance, so the gateway
        // reads them here instead of widening `SearchGateway` — the dispatcher's seam
        // keeps its shape, and a searcher for a public candidate is built with no
        // secret at all ([`credentials_for`]). A file that cannot be read sends
        // nothing, like every other read in this module.
        let credentials = match crate::settings::load(&self.path) {
            Ok(settings) => credentials_for(
                &settings.web_search_instance,
                instance,
                &settings.web_search_credentials,
            ),
            Err(_) => String::new(),
        };
        Box::new(SearxngSearcher::with_credentials(instance, &credentials))
    }
}

/// Credentials: the `Authorization` header one configured value is worth, or
/// `None`.
///
/// The rule is the shape of what the human typed, because that is the only signal
/// there is and it is the one every tool uses:
///
/// - `user:password` (it contains a colon) → `Basic base64(user:password)`, which is
///   what an nginx in front of a SearXNG instance asks for;
/// - anything else → `Bearer <value>`, for an instance behind a token;
/// - empty (or only spaces) → `None`: no header at all, never an empty one, so an
///   anonymous instance sees exactly the request it saw before this field existed.
///
/// Only the surrounding spaces are trimmed; the value itself is data, so a password
/// keeps its colons and a token keeps its characters and its case.
pub fn authorization_header(credentials: &str) -> Option<String> {
    let trimmed = credentials.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains(':') {
        // The whole `user:password` is what Basic auth encodes, so a password with a
        // colon in it keeps working.
        Some(format!("Basic {}", BASE64_STANDARD.encode(trimmed)))
    } else {
        Some(format!("Bearer {trimmed}"))
    }
}

/// The credentials, but only for the instance the human configured.
///
/// Both sides are compared normalized, because `searx.example`, `searx.example/`
/// and `https://searx.example` are one instance to [`normalize_instance`]. Every
/// other case answers an empty string, which [`authorization_header`] then turns
/// into no header: no instance configured, a public fallback candidate, or nothing
/// stored at all.
pub fn credentials_for(configured: &str, instance: &str, credentials: &str) -> String {
    let trimmed = credentials.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let configured = normalize_instance(configured);
    if configured.is_empty() || configured != normalize_instance(instance) {
        return String::new();
    }
    trimmed.to_string()
}

/// Well-known public SearXNG endpoints, tried in this order when the human
/// configured none.
///
/// **Best effort, by nature, not by accident.** A public instance owes nobody a
/// JSON API: most of them rate-limit it, some disable it outright, and any of
/// them can be down, renamed or start refusing this app's user agent tomorrow.
/// That is why an empty setting is the convenience path and a configured instance
/// is the reliable one — and why the error the model reads says exactly that
/// instead of pretending the search failed for another reason.
pub fn public_instances() -> &'static [&'static str] {
    &[
        "https://searx.be",
        "https://search.bus-hit.me",
        "https://priv.au",
        "https://searx.work",
    ]
}

/// S3: `limit` clamped into 1..=10. A model that asks for 0 wants "a few", not
/// nothing; a model that asks for 500 gets the same short answer as one that
/// asked for ten.
pub fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_LIMIT)
}

/// S5: what the model reads for one search. One block per hit — `title`, `url`,
/// `snippet` — separated by a blank line, which is compact enough to read and
/// regular enough to quote from.
///
/// A hit with no snippet keeps two lines instead of three: SearXNG does not always
/// have one, and an empty third line turns one block into two.
pub fn format_hits(query: &str, hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return format!(
            "no results for \"{query}\": the instance answered with an empty result list. \
             Try different words, or a narrower query."
        );
    }
    let blocks: Vec<String> = hits
        .iter()
        .map(|hit| {
            let mut block = format!("{}\n{}", hit.title.trim(), hit.url.trim());
            let snippet = hit.snippet.trim();
            if !snippet.is_empty() {
                block.push('\n');
                block.push_str(snippet);
            }
            block
        })
        .collect();
    blocks.join("\n\n")
}

/// Percent-encodes one query for the `q` parameter of a url.
///
/// Hand-written on purpose: `url` and `percent-encoding` are only **transitive**
/// dependencies (they arrive with `reqwest`), and this crate does not depend on
/// them directly — a direct pin for one `encode` call is not worth it. The set
/// kept raw is RFC 3986's unreserved set, so everything else (`&`, `=`, `+`, `#`,
/// `?`, `/`, spaces, every non-ASCII byte) becomes `%XX`. That is all a query
/// string needs, and there is no parsing here that could be got wrong.
pub fn encode_query(raw: &str) -> String {
    let mut encoded = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// A configured instance, normalized enough to build a url from: trimmed, no
/// trailing slash, `https://` added when the human typed a bare host (what
/// people do: `searx.be`).
pub fn normalize_instance(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

/// S2: the real searcher — `GET {instance}/search?q=…&format=json`.
///
/// A struct per instance, built by [`SettingsSearch::searcher`]: the instance and
/// the one header derived from the configured credentials are the whole state, so
/// two instances can be in flight without sharing anything.
pub struct SearxngSearcher {
    instance: String,
    /// The `Authorization` header already built, or `None` for an anonymous
    /// instance. Built once, in [`SearxngSearcher::with_credentials`], so a request
    /// can never re-encode a secret differently from the one next to it.
    authorization: Option<String>,
}

impl SearxngSearcher {
    /// The searcher for an instance, carrying the header its credentials are
    /// worth (none for an empty value, which is the anonymous case).
    pub fn with_credentials(instance: &str, credentials: &str) -> SearxngSearcher {
        SearxngSearcher {
            instance: normalize_instance(instance),
            authorization: authorization_header(credentials),
        }
    }

    /// The url one query is asked at. `format=json` is SearXNG's own switch: an
    /// instance that has the JSON API disabled answers HTML, which
    /// [`parse_results`] then reports honestly instead of quoting markup at the
    /// model.
    pub fn endpoint(&self, query: &str) -> String {
        format!(
            "{}/search?q={}&format=json",
            self.instance,
            encode_query(query)
        )
    }
}

impl Searcher for SearxngSearcher {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, String> {
        if self.instance.is_empty() {
            return Err("no instance was configured".to_string());
        }
        let endpoint = self.endpoint(query);
        let body = fetch(&endpoint, self.authorization.as_deref())?;
        parse_results(&body, clamp_limit(limit))
    }
}

/// One GET, run on a thread of its own.
///
/// The dispatcher runs inside the MCP server's single-thread tokio runtime, where
/// `Runtime::block_on` panics (a runtime cannot be driven from inside a runtime)
/// and where a blocking HTTP call would stop the endpoint from answering anything
/// else. A short-lived OS thread owns its own current-thread runtime instead, so
/// the search blocks one thread that exists only for it. `reqwest`'s blocking
/// client is not an option: this crate enables no `blocking` feature for it.
///
/// `authorization` is the header already built for this instance, moved into the
/// thread because that is what it ends up as: a borrowed string could not outlive
/// the call.
fn fetch(endpoint: &str, authorization: Option<&str>) -> Result<String, String> {
    let url = endpoint.to_string();
    let authorization = authorization.map(str::to_string);
    let handle = std::thread::Builder::new()
        .name("nabla-web-search".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("could not start the search runtime: {error}"))?;
            runtime.block_on(async move { get_json(&url, authorization.as_deref()).await })
        })
        .map_err(|error| format!("could not start the search thread: {error}"))?;
    handle
        .join()
        .map_err(|_| "the search thread stopped without an answer".to_string())?
}

/// The request itself: one JSON GET with [`SEARCH_TIMEOUT`] as the whole budget,
/// and the `Authorization` header when one was configured.
async fn get_json(url: &str, authorization: Option<&str>) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(SEARCH_TIMEOUT)
        .build()
        .map_err(|error| format!("could not build the search client: {error}"))?;
    let mut request = client
        .get(url)
        .header("Accept", "application/json")
        .header("User-Agent", concat!("nabla/", env!("CARGO_PKG_VERSION")));
    if let Some(authorization) = authorization {
        request = request.header("Authorization", authorization);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(http_status_error(status.as_u16(), authorization.is_some()));
    }
    Ok(body)
}

/// S3: a refusal, said plainly. The two that matter are the ones a public
/// instance usually answers with, and they are the reason the fix is "configure
/// your own instance" rather than "try again".
///
/// `401`/`403` mean two different things depending on what was sent: with
/// credentials the instance rejected them — an expired token, a wrong password, a
/// `user:password` shape where it wanted a token — and without them it refused the
/// anonymous JSON request, which is the truth for a public instance that blocks the
/// API. Saying "rejected the credentials" when none were sent would send the human
/// hunting for a field that is already empty.
fn http_status_error(code: u16, credentials_sent: bool) -> String {
    match code {
        401 | 403 if credentials_sent => format!(
            "HTTP {code}: the instance rejected the credentials from Settings > Web search \
             (check the user:password or the token, and that this instance is the one \
             expecting them)"
        ),
        401 | 403 => format!(
            "HTTP {code}: the instance refuses the JSON API, and may require credentials \
             — set them in Settings > Web search if it does"
        ),
        429 => format!("HTTP {code}: the instance is rate-limiting requests"),
        _ => format!("HTTP {code}"),
    }
}

/// S2: the hits in one SearXNG answer — `results[].{title,url,content}`, capped at
/// `limit`. A row without a url is dropped (there is nothing to cite), and a
/// missing title falls back to the url.
///
/// An answer that is not JSON is reported as such: a public instance with the
/// JSON API disabled answers its HTML page, and "HTTP 200, unparseable" is exactly
/// the failure a human needs to hear about.
pub fn parse_results(body: &str, limit: usize) -> Result<Vec<SearchHit>, String> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|error| {
        format!(
            "the answer is not JSON ({error}): the instance may have the JSON API disabled, \
             which public instances often do"
        )
    })?;
    let rows = value
        .get("results")
        .and_then(|results| results.as_array())
        .ok_or_else(|| "the answer has no `results` array".to_string())?;

    Ok(rows
        .iter()
        .filter_map(|row| {
            let url = row.get("url").and_then(|value| value.as_str())?.trim();
            if url.is_empty() {
                return None;
            }
            Some(SearchHit {
                title: text_field(row, "title").unwrap_or_else(|| url.to_string()),
                url: url.to_string(),
                snippet: text_field(row, "content").unwrap_or_default(),
            })
        })
        .take(limit)
        .collect())
}

/// One optional string field of a result row, trimmed; empty reads as absent so a
/// title of spaces falls back like a missing one.
fn text_field(row: &serde_json::Value, key: &str) -> Option<String> {
    let value = row.get(key).and_then(|value| value.as_str())?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// A gateway that answers "off" and builds nothing: the stand-in the dispatcher's
/// view-tool tests use, so no test can reach the network by accident. Test-only
/// because the app always wires the real one (`SettingsSearch`); a build that used
/// it would search nothing while claiming to be wired.
#[cfg(test)]
pub struct NoSearch;

#[cfg(test)]
impl SearchGateway for NoSearch {
    fn enabled(&self) -> bool {
        false
    }

    fn instance(&self) -> String {
        String::new()
    }

    fn searcher(&self, _instance: &str) -> Box<dyn Searcher> {
        Box::new(UnreachableSearcher)
    }
}

/// The searcher [`NoSearch`] hands out. Reaching it is a bug in the dispatcher
/// (a disabled gateway was asked to search), so it says that instead of guessing
/// an answer.
#[cfg(test)]
struct UnreachableSearcher;

#[cfg(test)]
impl Searcher for UnreachableSearcher {
    fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SearchHit>, String> {
        Err("no search gateway is wired to this server".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(title: &str, url: &str, snippet: &str) -> SearchHit {
        SearchHit {
            title: title.to_string(),
            url: url.to_string(),
            snippet: snippet.to_string(),
        }
    }

    // S2: the query is data, not url syntax. `&`, `=`, `+`, `#`, a space and a
    // non-ASCII byte must all be escaped, or `q=a&format=html` would rewrite the
    // request the model asked for.
    #[test]
    fn a_query_is_percent_encoded() {
        assert_eq!(encode_query("rust lang"), "rust%20lang");
        assert_eq!(encode_query("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode_query("c++ #1"), "c%2B%2B%20%231");
        assert_eq!(encode_query("ñandú"), "%C3%B1and%C3%BA");
        // The unreserved set is left alone, so a plain query stays readable.
        assert_eq!(encode_query("Rust-lang_1.0~x"), "Rust-lang_1.0~x");
    }

    // S2: the url one instance is asked at, from what the human typed.
    #[test]
    fn the_endpoint_is_built_from_the_normalized_instance() {
        let bare = SearxngSearcher::with_credentials("  searx.be/  ", "");
        assert_eq!(
            bare.endpoint("a&b"),
            "https://searx.be/search?q=a%26b&format=json"
        );
        let full = SearxngSearcher::with_credentials("https://searx.example/base/", "");
        assert_eq!(
            full.endpoint("rust"),
            "https://searx.example/base/search?q=rust&format=json"
        );
        // Empty stays empty: a searcher with no instance must not invent one.
        assert_eq!(normalize_instance("   "), "");
        assert_eq!(normalize_instance("/"), "");
    }

    // S2: `results[]` is what SearXNG documents, and only the three fields the
    // model reads are kept.
    #[test]
    fn results_are_parsed_into_hits() {
        let body = r#"{
            "query": "rust",
            "results": [
                { "title": "Rust", "url": "https://rust-lang.org/", "content": "A language." },
                { "title": "Docs", "url": "https://doc.rust-lang.org/", "content": "The book." },
                { "title": "no url", "content": "dropped" },
                { "title": "  ", "url": "https://a.example/", "content": "" }
            ]
        }"#;
        let hits = parse_results(body, MAX_LIMIT).expect("parse");
        assert_eq!(
            hits,
            vec![
                hit("Rust", "https://rust-lang.org/", "A language."),
                hit("Docs", "https://doc.rust-lang.org/", "The book."),
                // No title, no snippet: the url is the honest stand-in for one and
                // an empty snippet stays empty instead of being invented.
                hit("https://a.example/", "https://a.example/", ""),
            ]
        );
    }

    // S3: `limit` is the cap, not a request for more: the parser never returns more
    // than it was asked for.
    #[test]
    fn results_are_capped_at_the_limit() {
        let body = r#"{"results":[
            {"title":"a","url":"https://a/","content":""},
            {"title":"b","url":"https://b/","content":""},
            {"title":"c","url":"https://c/","content":""}
        ]}"#;
        assert_eq!(parse_results(body, 2).expect("parse").len(), 2);
        assert_eq!(parse_results(body, 1).expect("parse").len(), 1);
        let empty = parse_results(r#"{"results":[]}"#, 5).expect("parse");
        assert!(empty.is_empty());
    }

    // S3: a body that is not JSON is the honest failure of an instance with the
    // JSON API disabled — HTML is what it answers instead.
    #[test]
    fn an_html_answer_is_reported_as_a_disabled_json_api() {
        let reason = parse_results("<!doctype html><h1>Search</h1>", 5).expect_err("not JSON");
        assert!(reason.contains("not JSON"), "{reason}");
        assert!(reason.contains("JSON API"), "{reason}");

        let no_results = parse_results(r#"{"error":"nope"}"#, 5).expect_err("no results array");
        assert!(no_results.contains("results"), "{no_results}");
    }

    // S3: what the model reads. One block per hit, blank line between blocks, and
    // exactly the three fields — no markup, no numbering to quote by accident.
    #[test]
    fn format_hits_writes_one_block_per_result() {
        let hits = vec![
            hit("Rust", "https://rust-lang.org/", "A language."),
            hit("Docs", "https://doc.rust-lang.org/", "The book."),
        ];
        assert_eq!(
            format_hits("rust", &hits),
            "Rust\nhttps://rust-lang.org/\nA language.\n\nDocs\nhttps://doc.rust-lang.org/\nThe book."
        );

        // A hit with no snippet keeps two lines, so a block is never half empty.
        assert_eq!(
            format_hits("rust", &[hit("Only", "https://only.example/", "")]),
            "Only\nhttps://only.example/"
        );
    }

    // S5: no hits is an answer, not an error, and it names the query it searched.
    #[test]
    fn format_hits_says_when_there_were_no_results() {
        let text = format_hits("ñandú zzz", &[]);
        assert!(text.contains("no results"), "{text}");
        assert!(text.contains("\"ñandú zzz\""), "names the query: {text}");
    }

    // S3: the clamp, in both directions and at both edges.
    #[test]
    fn the_limit_is_clamped_to_one_and_ten() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(1), 1);
        assert_eq!(clamp_limit(7), 7);
        assert_eq!(clamp_limit(10), 10);
        assert_eq!(clamp_limit(11), 10);
        assert_eq!(clamp_limit(usize::MAX), MAX_LIMIT);
        assert_eq!(DEFAULT_LIMIT.clamp(1, MAX_LIMIT), DEFAULT_LIMIT);
    }

    // ── configured credentials ───────────────────────────────────────────────

    // The credentials the human configured become exactly one header. A value
    // with a colon is `user:password` — the Basic auth an nginx in front of
    // SearXNG asks for — and anything else is a bearer token. Nothing configured
    // is no header at all, never an empty one.
    #[test]
    fn credentials_become_one_authorization_header() {
        assert_eq!(authorization_header(""), None);
        assert_eq!(authorization_header("   \n\t"), None);
        assert_eq!(authorization_header("u:s").as_deref(), Some("Basic dTpz"));
        // Split at the first colon only: a password may contain one, and the whole
        // value is what Basic auth encodes.
        assert_eq!(
            authorization_header("user:pa:ss").as_deref(),
            Some("Basic dXNlcjpwYTpzcw==")
        );
        assert_eq!(
            authorization_header("tok123").as_deref(),
            Some("Bearer tok123")
        );
        // Surrounding spaces are dropped; nothing else is touched.
        assert_eq!(
            authorization_header("  u:s  ").as_deref(),
            Some("Basic dTpz")
        );
        assert_eq!(
            authorization_header("  a b  ").as_deref(),
            Some("Bearer a b")
        );
    }

    // The leak rule: the credentials belong to the instance the human named.
    // The public fallback walks instances they never named, and a header sent to
    // one of them would hand the secret to a third party.
    #[test]
    fn credentials_only_travel_to_the_configured_instance() {
        assert_eq!(
            credentials_for("https://searx.example", "https://searx.example/", " u:p "),
            "u:p",
            "a trailing slash is the same instance"
        );
        assert_eq!(
            credentials_for("searx.example", "https://searx.example", "u:p"),
            "u:p",
            "a bare host and its url are the same instance"
        );
        assert_eq!(
            credentials_for("", "https://searx.be", "u:p"),
            "",
            "no configured instance means no instance may receive the secret"
        );
        assert_eq!(
            credentials_for("https://searx.example", "https://searx.be", "u:p"),
            "",
            "a public candidate gets nothing"
        );
        assert_eq!(
            credentials_for("https://searx.example", "https://searx.example", "  "),
            "",
            "nothing configured is nothing sent"
        );
    }

    // A 401/403 means the instance rejected what was sent. With credentials the
    // message says so; without them it says the JSON API was refused, because
    // that is the truthful reading of an anonymous refusal.
    #[test]
    fn a_401_names_the_credentials_when_some_were_sent() {
        let rejected = http_status_error(401, true);
        assert!(rejected.contains("401"), "{rejected}");
        assert!(rejected.contains("rejected the credentials"), "{rejected}");

        let anonymous = http_status_error(403, false);
        assert!(anonymous.contains("403"), "{anonymous}");
        assert!(anonymous.contains("JSON API"), "{anonymous}");
        assert!(
            !anonymous.contains("rejected the credentials"),
            "{anonymous}"
        );

        // A rate limit is about volume, not about who is asking.
        assert_eq!(http_status_error(429, true), http_status_error(429, false));
        assert!(http_status_error(429, false).contains("rate-limiting"));
        assert_eq!(http_status_error(500, true), "HTTP 500");
    }

    /// A one-request HTTP server on loopback, for the probes that must observe the
    /// wire. It answers `answer` to whatever it is asked and hands the request head
    /// back through the channel, so a test can read the headers that were really
    /// sent. Nothing leaves the machine and no port is fixed.
    fn serve_once(answer: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("one connection");
            let mut head: Vec<u8> = Vec::new();
            let mut buffer = [0u8; 1024];
            // The request head ends at the blank line; the body of a GET is empty.
            while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => head.extend_from_slice(&buffer[..read]),
                }
            }
            let _ = sender.send(String::from_utf8_lossy(&head).to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
        (format!("http://{addr}"), receiver)
    }

    // The header really reaches the instance. Both shapes are observed on the
    // wire — through the searcher the dispatcher would build, against a fake
    // server — not only in the helper's return value. A searcher with no
    // credentials sends no `Authorization` header at all.
    #[test]
    fn the_authorization_header_reaches_the_instance() {
        const ANSWER: &str =
            r#"{"results":[{"title":"Rust","url":"https://rust-lang.org/","content":""}]}"#;

        let (base, seen) = serve_once(ANSWER);
        let basic = SearxngSearcher::with_credentials(&format!("{base}/"), "u:s");
        assert_eq!(basic.search("rust", 5).expect("the fake answers").len(), 1);
        let request = seen.recv().expect("one request");
        // Header names travel lowercase on the wire (http::HeaderName).
        assert!(
            request.to_lowercase().contains("authorization: basic dtpz"),
            "{request}"
        );
        assert!(
            request.starts_with("GET /search?q=rust&format=json HTTP/1.1"),
            "{request}"
        );

        let (base, seen) = serve_once(ANSWER);
        let bearer = SearxngSearcher::with_credentials(&base, "tok123");
        assert_eq!(bearer.search("rust", 5).expect("the fake answers").len(), 1);
        let request = seen.recv().expect("one request");
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer tok123"),
            "{request}"
        );

        let (base, seen) = serve_once(ANSWER);
        assert_eq!(
            SearxngSearcher::with_credentials(&base, "")
                .search("rust", 5)
                .expect("the fake answers")
                .len(),
            1
        );
        let request = seen.recv().expect("one request");
        assert!(
            !request.to_lowercase().contains("authorization"),
            "no credentials, no header: {request}"
        );
    }

    // S2/L4: the public list is the best-effort fallback, and it holds real
    // endpoints (no placeholders a reader would have to decode).
    #[test]
    fn the_public_list_holds_https_endpoints() {
        let instances = public_instances();
        assert!(instances.len() >= 3, "{instances:?}");
        for instance in instances {
            assert!(instance.starts_with("https://"), "{instance}");
            assert!(
                !instance.ends_with('/'),
                "a slash would double up: {instance}"
            );
        }
    }
}
