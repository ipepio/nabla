// Embedded browser: a Tauri v2 **child webview** that the app owns and drives.
//
// Design notes:
// - **A real webview, never an iframe** (maintainer decision). X-Frame-Options
//   and CSP `frame-ancestors` make an iframe unusable for arbitrary sites, and
//   an external browser is not what this pane is for: the window keeps the app
//   UI, one child webview of that same window shows the page.
// - **The multiwebview API is Tauri's `unstable` feature.** In tauri 2.12.5
//   `Window::add_child` is `#[cfg(any(test, all(desktop, feature = "unstable")))]`
//   and `WebviewBuilder` is gated the same way, so `Cargo.toml` enables
//   `tauri = { features = ["unstable"] }`. Nothing else in this crate uses it.
// - **The registry is metadata; Tauri owns the webview.** This module keeps the
//   ids and the last url the navigation handler saw, which is what the tests
//   below exercise without a window. The `Webview` itself lives in Tauri's
//   manager and is resolved by label (`embed-<n>`) on every command.
// - **One lifetime per view.** A view is created inside the main window, moved
//   and sized by the app (the UI keeps it in sync with its panel rect), and
//   closed on request, on `embed_close_all`, or when the app exits.
//
// Platform notes:
// - **macOS (the path built and verified here).** `Window::add_child` maps to a
//   sibling `WKWebView` in the window's content view, which is a native,
//   first-class arrangement: several webviews per window, each with its own
//   position, size and JS context.
// - **Windows.** WebView2 also supports child webviews, but `add_child`
//   deadlocks when it is called from a synchronous command or an event handler
//   (tauri's documented Webview2 issue). Every command below is `async` and does
//   its blocking work on a worker thread, so the main thread is free to build
//   the child. `embed_eval` waits for a callback and therefore never runs on the
//   main thread either.
// - **Linux.** wry builds child webviews on webkit2gtk, but positioning and
//   transparency differ from macOS and were not exercised by this feature; a
//   GTK window may also need the child to be reparented on resize.
// - **iOS/Android.** `Window::add_child` does not exist there (it is
//   `cfg(desktop)`), so the registry still exists — `AppState` stays uniform —
//   and every webview operation answers [`EmbedError::Unsupported`].
//
// Payloads serialize camelCase, like every other payload of the seam: the
// `embed-nav` event is `{ viewId, url }`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tauri::{Emitter, Manager, Url};

#[cfg(desktop)]
use tauri::webview::WebviewBuilder;
#[cfg(desktop)]
use tauri::{AppHandle, LogicalPosition, LogicalSize, WebviewUrl};

/// How many embedded browsers may exist at once. The pane shows one panel; four
/// is room for a second look and a third without letting a stuck UI leak a
/// webview per click.
pub const MAX_VIEWS: usize = 4;

/// What a view shows before it is navigated anywhere.
pub const DEFAULT_URL: &str = "about:blank";

/// Every webview label, and every view id, starts with this.
pub const VIEW_PREFIX: &str = "embed-";

/// The window the child webviews are added to: the single window of
/// `tauri.conf.json`, whose label Tauri defaults to `main`.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Channel the navigation handler reports on, one-way, to the app UI.
pub const NAV_EVENT: &str = "embed-nav";

/// How long `embed_eval` waits for the page to answer. A page that is loading or
/// gone never answers, and a command must not hang the caller forever.
const EVAL_TIMEOUT: Duration = Duration::from_secs(5);

/// A webview cannot have a zero size; a collapsed panel reports one.
const MIN_EXTENT: f64 = 1.0;

/// One child webview in the registry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedView {
    /// Stable id, and the webview label itself: `embed-<n>`.
    pub id: String,
    /// Last url the navigation handler reported (requirement 7). It is the url
    /// the human is looking at, not the one that was last requested.
    pub url: String,
}

/// What the navigation handler emits on [`NAV_EVENT`]: `{ viewId, url }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedNavEvent {
    pub view_id: String,
    pub url: String,
}

/// Failures of the embedded browser, typed inside Rust and rendered as a
/// readable string when they cross the Tauri bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedError {
    /// [`MAX_VIEWS`] views are already open.
    TooMany(usize),
    /// The id is not in the registry (already closed, or never here).
    UnknownView(String),
    /// Only http, https and `about:blank` are ever requested by the app.
    InvalidUrl(String),
    /// The window or the webview could not be built.
    Create(String),
    /// Position or size was refused by the platform.
    Bounds(String),
    /// The navigation was refused by the platform.
    Navigate(String),
    /// The script could not be evaluated, or the page never answered.
    Eval(String),
    /// This platform has no multiwebview API at all. Only the mobile stubs
    /// below construct it.
    #[cfg_attr(desktop, allow(dead_code))]
    Unsupported,
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::TooMany(cap) => write!(
                f,
                "embed: {cap} embedded browsers are already open; close one before opening another"
            ),
            EmbedError::UnknownView(id) => write!(f, "embed: no embedded browser {id} is open"),
            EmbedError::InvalidUrl(raw) => write!(
                f,
                "embed: only http, https and about:blank urls can be opened, got \"{raw}\""
            ),
            EmbedError::Create(message) => {
                write!(f, "embed: could not create the embedded browser: {message}")
            }
            EmbedError::Bounds(message) => {
                write!(f, "embed: could not place the embedded browser: {message}")
            }
            EmbedError::Navigate(message) => {
                write!(f, "embed: could not navigate the embedded browser: {message}")
            }
            EmbedError::Eval(message) => write!(f, "embed: {message}"),
            EmbedError::Unsupported => write!(
                f,
                "embed: an embedded browser needs a desktop build (macOS, Windows or Linux)"
            ),
        }
    }
}

impl std::error::Error for EmbedError {}

// ── url policy ─────────────────────────────────────────────────────────────

/// Accepts what the app is allowed to ask a view to open, and answers the url to
/// use. Only `http`, `https` and `about:blank` pass: a local file, a
/// `javascript:` url or an empty string would hand the pane something the app
/// did not mean to show, so each of them is refused with the raw input in the
/// message.
pub fn validate_url(raw: &str) -> Result<String, EmbedError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(EmbedError::InvalidUrl(raw.to_string()));
    }
    // `about:blank` is the empty pane: the only `about:` page this app opens.
    if trimmed.eq_ignore_ascii_case(DEFAULT_URL) {
        return Ok(DEFAULT_URL.to_string());
    }
    match Url::parse(trimmed) {
        Ok(parsed)
            if matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some() =>
        {
            // The parsed form is what the webview gets: a bare host gains its
            // `/` and escapes are normalized once, here.
            Ok(parsed.to_string())
        }
        // Everything else — `file:`, `javascript:`, `data:`, a hostless url, a
        // string that is not a url at all — is the same refusal with the exact
        // input in it, so the UI can show what was rejected.
        _ => Err(EmbedError::InvalidUrl(raw.to_string())),
    }
}

/// What a *page* may navigate to. Only `file:` is refused — the local
/// filesystem is outside what this pane is for — so links, `data:`, `blob:` and
/// app-handled schemes keep working inside the child webview.
fn navigation_allowed(url: &str) -> bool {
    !url
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("file:")
}

/// A position or size the platform accepts: a collapsed panel reports 0.
fn extent(value: f64) -> f64 {
    if value.is_finite() && value >= MIN_EXTENT {
        value
    } else {
        MIN_EXTENT
    }
}

// ── registry ───────────────────────────────────────────────────────────────

/// The child webviews this app created, by id. Cloneable because the navigation
/// handler outlives the command that built it and needs the same registry.
#[derive(Clone)]
pub struct EmbedSessions {
    inner: Arc<Inner>,
}

struct Inner {
    /// Registered views, in creation order.
    views: Mutex<Vec<EmbeddedView>>,
    /// Counter behind `embed-<n>`. Monotonic, so a label that was closed is not
    /// handed out again while the app runs.
    next_index: AtomicUsize,
}

impl Default for EmbedSessions {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbedSessions {
    pub fn new() -> EmbedSessions {
        EmbedSessions {
            inner: Arc::new(Inner {
                views: Mutex::new(Vec::new()),
                next_index: AtomicUsize::new(0),
            }),
        }
    }

    /// Every registered view, in creation order. A copy, so a caller never holds
    /// the registry lock while it talks to a webview. Test-facing: the app
    /// addresses one view by id, never the whole list.
    #[cfg(test)]
    pub fn views(&self) -> Vec<EmbeddedView> {
        lock(&self.inner.views).clone()
    }

    /// How many views are registered.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        lock(&self.inner.views).len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One view by id.
    pub fn get(&self, view_id: &str) -> Option<EmbeddedView> {
        lock(&self.inner.views)
            .iter()
            .find(|view| view.id == view_id)
            .cloned()
    }

    /// Records the url the navigation handler just saw. `false` when the id was
    /// closed while the handler was in flight: a late event must never
    /// resurrect a view.
    pub fn set_url(&self, view_id: &str, url: &str) -> bool {
        let mut views = lock(&self.inner.views);
        match views.iter_mut().find(|view| view.id == view_id) {
            Some(view) => {
                view.url = url.to_string();
                true
            }
            None => false,
        }
    }

    /// Takes the next free label and registers the view before the webview
    /// exists, so the navigation handler always finds its own id. Refuses over
    /// the cap without allocating anything.
    fn reserve(&self, url: String) -> Result<EmbeddedView, EmbedError> {
        let mut views = lock(&self.inner.views);
        if views.len() >= MAX_VIEWS {
            return Err(EmbedError::TooMany(MAX_VIEWS));
        }
        // The counter is monotonic, so this loop only matters if a label could
        // ever come back: it keeps uniqueness independent of that assumption.
        let id = loop {
            let next = self.inner.next_index.fetch_add(1, Ordering::Relaxed) + 1;
            let candidate = format!("{VIEW_PREFIX}{next}");
            if !views.iter().any(|view| view.id == candidate) {
                break candidate;
            }
        };
        let view = EmbeddedView { id, url };
        views.push(view.clone());
        Ok(view)
    }

    /// Removes one view from the registry and answers it, so the caller can
    /// close the webview it describes.
    fn forget(&self, view_id: &str) -> Option<EmbeddedView> {
        let mut views = lock(&self.inner.views);
        let index = views.iter().position(|view| view.id == view_id)?;
        Some(views.remove(index))
    }

    /// Removes every view and answers them, for `close_all`.
    fn drain(&self) -> Vec<EmbeddedView> {
        std::mem::take(&mut *lock(&self.inner.views))
    }
}

/// One lock helper for the whole module, so a poisoned mutex is a panic here
/// instead of an `unwrap` on every call site (same shape as `browser.rs`).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

// ── webview operations (desktop only) ──────────────────────────────────────

#[cfg(desktop)]
impl EmbedSessions {
    /// Creates a child webview inside the main window, at a logical position and
    /// size, and answers its id.
    ///
    /// The registry entry exists before `add_child` runs: the navigation handler
    /// can fire while the webview is still being built, and it must find the id
    /// it is supposed to report on. A refused or failed build removes the entry
    /// again, so a failed call never uses up a slot.
    pub fn create(
        &self,
        app: &AppHandle,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        url: Option<String>,
    ) -> Result<String, EmbedError> {
        let initial = match url {
            Some(raw) => validate_url(&raw)?,
            None => DEFAULT_URL.to_string(),
        };
        let parsed = Url::parse(&initial).map_err(|err| EmbedError::InvalidUrl(err.to_string()))?;
        let window = app
            .get_window(MAIN_WINDOW_LABEL)
            .ok_or_else(|| EmbedError::Create(format!("no window labelled {MAIN_WINDOW_LABEL}")))?;
        // The entry exists before the webview does: the navigation handler can
        // fire while it is still being built and must find the id it reports on.
        // A failed build removes the entry again, so no slot leaks.
        let view = self.reserve(initial)?;
        let registry = self.clone();
        let emitter = app.clone();
        let navigated_id = view.id.clone();
        let builder = WebviewBuilder::new(view.id.clone(), WebviewUrl::External(parsed))
            .on_navigation(move |url| {
                // Only an accepted navigation is recorded and reported: a refused
                // one loaded nothing, so answering its url would misdescribe
                // what the pane shows.
                if !navigation_allowed(url.as_str()) {
                    return false;
                }
                let url = url.to_string();
                registry.set_url(&navigated_id, &url);
                let _ = emitter.emit(
                    NAV_EVENT,
                    EmbedNavEvent {
                        view_id: navigated_id.clone(),
                        url,
                    },
                );
                true
            });
        match window.add_child(
            builder,
            // A position may be negative (a panel scrolled partially off), only
            // the size has a floor the platform accepts.
            LogicalPosition::new(x, y),
            LogicalSize::new(extent(width), extent(height)),
        ) {
            Ok(_webview) => Ok(view.id),
            Err(err) => {
                self.forget(&view.id);
                Err(EmbedError::Create(err.to_string()))
            }
        }
    }

    /// Moves and resizes a view: the UI keeps it in sync with the panel rect and
    /// with every resize of that rect.
    pub fn set_bounds(
        &self,
        app: &AppHandle,
        view_id: &str,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), EmbedError> {
        let webview = self.webview(app, view_id)?;
        webview
            .set_position(LogicalPosition::new(x, y))
            .map_err(|err| EmbedError::Bounds(err.to_string()))?;
        webview
            .set_size(LogicalSize::new(extent(width), extent(height)))
            .map_err(|err| EmbedError::Bounds(err.to_string()))
    }

    /// Navigates a view. The url is validated with [`validate_url`] first, so an
    /// unsupported scheme is refused before the webview ever sees it.
    pub fn navigate(&self, app: &AppHandle, view_id: &str, url: &str) -> Result<(), EmbedError> {
        let target = validate_url(url)?;
        let parsed = Url::parse(&target).map_err(|err| EmbedError::InvalidUrl(err.to_string()))?;
        self.webview(app, view_id)?
            .navigate(parsed)
            .map_err(|err| EmbedError::Navigate(err.to_string()))
    }

    /// Evaluates `script` in the child webview and answers the result.
    ///
    /// `Webview::eval` returns `()`: it fires and forgets. `eval_with_callback`
    /// is the API that answers — the platform serializes the evaluation result
    /// into a JSON string and hands it to a callback. That callback arrives on
    /// the main thread, so this function waits on a channel from a worker thread
    /// (the commands do exactly that) and never from the main thread itself.
    pub fn eval(&self, app: &AppHandle, view_id: &str, script: &str) -> Result<String, EmbedError> {
        let webview = self.webview(app, view_id)?;
        let (sender, receiver) = std::sync::mpsc::channel::<String>();
        webview
            .eval_with_callback(script.to_string(), move |result| {
                // The receiver is gone when the wait already timed out; the
                // result of a script nobody waits for is dropped, not unwrapped.
                let _ = sender.send(result);
            })
            .map_err(|err| {
                EmbedError::Eval(format!("the script could not be evaluated: {err}"))
            })?;
        receiver.recv_timeout(EVAL_TIMEOUT).map_err(|_| {
            EmbedError::Eval(format!(
                "{view_id} did not answer within {}s",
                EVAL_TIMEOUT.as_secs()
            ))
        })
    }

    /// Closes one view and forgets it. Idempotent: an unknown id answers `false`
    /// rather than an error.
    pub fn close(&self, app: &AppHandle, view_id: &str) -> bool {
        // Forget first: a navigation handler firing during the close must find
        // nothing to update, and a second close answers `false`.
        let Some(view) = self.forget(view_id) else {
            return false;
        };
        if let Some(webview) = app.get_webview(&view.id) {
            let _ = webview.close();
        }
        true
    }

    /// Closes every view and answers how many were closed. Bound to the app's
    /// exit in `lib.rs`, so no webview outlives the app.
    pub fn close_all(&self, app: &AppHandle) -> usize {
        let views = self.drain();
        let closed = views.len();
        for view in views {
            if let Some(webview) = app.get_webview(&view.id) {
                let _ = webview.close();
            }
        }
        closed
    }

    /// The live webview of a registered view. The registry is the gate: an id
    /// nobody registered is never addressed, even if a webview with that label
    /// happened to exist.
    fn webview(&self, app: &AppHandle, view_id: &str) -> Result<tauri::Webview, EmbedError> {
        self.get(view_id)
            .ok_or_else(|| EmbedError::UnknownView(view_id.to_string()))?;
        app.get_webview(view_id)
            .ok_or_else(|| EmbedError::UnknownView(view_id.to_string()))
    }
}

#[cfg(not(desktop))]
impl EmbedSessions {
    // No `Window::add_child` on mobile: the registry exists so `AppState` stays
    // uniform, and every operation answers a readable refusal.
    pub fn create(
        &self,
        _app: &tauri::AppHandle,
        _x: f64,
        _y: f64,
        _width: f64,
        _height: f64,
        _url: Option<String>,
    ) -> Result<String, EmbedError> {
        Err(EmbedError::Unsupported)
    }

    pub fn set_bounds(
        &self,
        _app: &tauri::AppHandle,
        _view_id: &str,
        _x: f64,
        _y: f64,
        _width: f64,
        _height: f64,
    ) -> Result<(), EmbedError> {
        Err(EmbedError::Unsupported)
    }

    pub fn navigate(
        &self,
        _app: &tauri::AppHandle,
        _view_id: &str,
        _url: &str,
    ) -> Result<(), EmbedError> {
        Err(EmbedError::Unsupported)
    }

    pub fn eval(
        &self,
        _app: &tauri::AppHandle,
        _view_id: &str,
        _script: &str,
    ) -> Result<String, EmbedError> {
        Err(EmbedError::Unsupported)
    }

    pub fn close(&self, _app: &tauri::AppHandle, _view_id: &str) -> bool {
        false
    }

    pub fn close_all(&self, _app: &tauri::AppHandle) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── url validation ─────────────────────────────────────────────────────

    #[test]
    fn validate_url_accepts_http_and_https_and_normalizes_them() {
        assert_eq!(validate_url("https://example.com"), Ok("https://example.com/".into()));
        assert_eq!(
            validate_url("http://127.0.0.1:1420/app?x=1"),
            Ok("http://127.0.0.1:1420/app?x=1".into())
        );
        // Surrounding whitespace is trimmed, not rejected: a pasted url has it.
        assert_eq!(validate_url("  https://tauri.app/  "), Ok("https://tauri.app/".into()));
    }

    #[test]
    fn validate_url_accepts_about_blank_only() {
        assert_eq!(validate_url("about:blank"), Ok(DEFAULT_URL.to_string()));
        // Any other `about:` page is not what the app means to show.
        assert_eq!(
            validate_url("about:config"),
            Err(EmbedError::InvalidUrl("about:config".into()))
        );
    }

    #[test]
    fn validate_url_rejects_every_other_scheme_with_a_readable_message() {
        for raw in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<h1>x</h1>",
            "tauri://localhost/index.html",
        ] {
            let error = validate_url(raw).expect_err(raw);
            assert_eq!(error, EmbedError::InvalidUrl(raw.to_string()));
            let message = error.to_string();
            assert!(
                message.contains("only http, https and about:blank"),
                "unexpected message: {message}"
            );
            assert!(message.contains(raw), "the message names the input: {message}");
        }
    }

    #[test]
    fn validate_url_rejects_empty_and_hostless_urls() {
        for raw in ["", "   ", "https://", "http://:8080"] {
            assert_eq!(
                validate_url(raw),
                Err(EmbedError::InvalidUrl(raw.to_string())),
                "expected {raw:?} to be refused"
            );
        }
        // `http:///path` is not hostless to the URL parser: it normalizes to the
        // host `path`. Recorded here so the normalization is a decision, not a
        // surprise.
        assert_eq!(validate_url("http:///path"), Ok("http://path/".into()));
    }

    #[test]
    fn navigation_policy_only_refuses_the_local_filesystem() {
        assert!(navigation_allowed("https://example.com/docs"));
        assert!(navigation_allowed("http://127.0.0.1:1420/"));
        assert!(navigation_allowed("about:blank"));
        assert!(navigation_allowed("data:text/html,x"));
        assert!(!navigation_allowed("file:///etc/passwd"));
    }

    #[test]
    fn extent_never_hands_the_platform_a_zero_size() {
        assert_eq!(extent(320.0), 320.0);
        assert_eq!(extent(0.0), MIN_EXTENT);
        assert_eq!(extent(-40.0), MIN_EXTENT);
        assert_eq!(extent(f64::NAN), MIN_EXTENT);
        // A position may be negative: only size is clamped by the caller.
        assert_eq!(extent(-1.0), MIN_EXTENT);
    }

    // ── label allocation ───────────────────────────────────────────────────

    #[test]
    fn reserve_allocates_stable_unique_labels() {
        let sessions = EmbedSessions::new();
        let first = sessions.reserve(DEFAULT_URL.into()).expect("first");
        let second = sessions.reserve(DEFAULT_URL.into()).expect("second");
        assert_eq!(first.id, "embed-1");
        assert_eq!(second.id, "embed-2");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions.views(), vec![first, second]);
    }

    #[test]
    fn reserve_never_reuses_a_label_that_was_closed() {
        let sessions = EmbedSessions::new();
        let first = sessions.reserve(DEFAULT_URL.into()).expect("first");
        let second = sessions.reserve(DEFAULT_URL.into()).expect("second");
        assert_eq!(sessions.forget(&second.id), Some(second.clone()));
        let third = sessions.reserve(DEFAULT_URL.into()).expect("third");
        assert_ne!(third.id, first.id);
        assert_ne!(third.id, second.id);
        assert_eq!(third.id, "embed-3");
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn reserve_enforces_the_cap_and_frees_a_slot_on_close() {
        let sessions = EmbedSessions::new();
        for _ in 0..MAX_VIEWS {
            sessions.reserve(DEFAULT_URL.into()).expect("under the cap");
        }
        assert_eq!(
            sessions.reserve(DEFAULT_URL.into()),
            Err(EmbedError::TooMany(MAX_VIEWS)),
            "the fifth view is refused"
        );
        // A refused reservation leaves nothing behind.
        assert_eq!(sessions.len(), MAX_VIEWS);
        let closed = sessions.views()[0].clone();
        assert_eq!(sessions.forget(&closed.id), Some(closed));
        assert_eq!(sessions.len(), MAX_VIEWS - 1);
        sessions
            .reserve("https://example.com/".into())
            .expect("a closed view frees its slot");
        assert_eq!(sessions.len(), MAX_VIEWS);
    }

    #[test]
    fn too_many_carries_the_cap_in_its_message() {
        let message = EmbedError::TooMany(MAX_VIEWS).to_string();
        assert!(message.contains(&MAX_VIEWS.to_string()), "unexpected: {message}");
        assert!(message.contains("already open"), "unexpected: {message}");
    }

    // ── last known url ─────────────────────────────────────────────────────

    #[test]
    fn set_url_records_the_last_navigation() {
        let sessions = EmbedSessions::new();
        let view = sessions.reserve(DEFAULT_URL.into()).expect("view");
        assert_eq!(sessions.get(&view.id).map(|v| v.url), Some(DEFAULT_URL.into()));

        assert!(sessions.set_url(&view.id, "https://example.com/"));
        assert_eq!(
            sessions.get(&view.id).map(|v| v.url),
            Some("https://example.com/".into())
        );
        // The latest navigation wins: it is the page on screen.
        assert!(sessions.set_url(&view.id, "https://example.com/docs"));
        assert_eq!(
            sessions.get(&view.id).map(|v| v.url),
            Some("https://example.com/docs".into())
        );
    }

    #[test]
    fn set_url_ignores_a_view_that_was_already_closed() {
        let sessions = EmbedSessions::new();
        let view = sessions.reserve(DEFAULT_URL.into()).expect("view");
        assert!(sessions.forget(&view.id).is_some());
        // A navigation handler firing after the close must not resurrect it.
        assert!(!sessions.set_url(&view.id, "https://example.com/"));
        assert_eq!(sessions.get(&view.id), None);
        assert!(sessions.is_empty());
    }

    #[test]
    fn unknown_ids_are_reported_by_id() {
        let sessions = EmbedSessions::new();
        assert_eq!(sessions.get("embed-99"), None);
        assert_eq!(sessions.forget("embed-99"), None);
        let message = EmbedError::UnknownView("embed-99".into()).to_string();
        assert!(message.contains("embed-99"), "unexpected: {message}");
    }

    #[test]
    fn drain_closes_the_registry_in_one_step() {
        let sessions = EmbedSessions::new();
        sessions.reserve(DEFAULT_URL.into()).expect("first");
        sessions.reserve("https://example.com/".into()).expect("second");
        let drained = sessions.drain();
        assert_eq!(drained.len(), 2);
        assert!(sessions.is_empty(), "close_all leaves no view behind");
        assert!(sessions.drain().is_empty(), "draining twice is empty, not a panic");
    }

    // Nothing below runs without a real window: `embed_create`, `set_bounds`,
    // `navigate`, `eval`, `close` and `close_all` need Tauri's event loop, a
    // WKWebView and a main thread, so they are manually verified in
    // `npm run tauri dev`. What is left here is every decision that does not need
    // the platform: url validation, label allocation, the cap and the url
    // bookkeeping the navigation handler performs.
}
