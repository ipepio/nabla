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
// - **Hidden is not closed.** `hide`/`show` are the platform's own visibility
//   switch: the view keeps its document, its history and its JS context, so the
//   page a human was reading survives the drawer collapsing. That is what makes
//   a closed drawer possible at all — the view floats above the HTML, so CSS
//   cannot hide it — and it is why the pane hides instead of closing.
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
// - **Linux.** `tauri.conf.json` creates `main` as a `WebviewWindow` whose
//   primary WebKitGTK view lives inside `window.default_vbox()` (a vertical
//   `gtk::Box`). `Window::add_child` packs child webviews into that same box,
//   splitting the window vertically and ignoring bounds; `linux_gtk` wraps the
//   main webview in a `gtk::Overlay` and floats each `embed-*` child at its
//   exact measured rect via `connect_get_child_position`.
// - **iOS/Android.** `Window::add_child` does not exist there (it is
//   `cfg(desktop)`), so the registry still exists — `AppState` stays uniform —
//   and every webview operation answers [`EmbedError::Unsupported`].
//
// Payloads serialize camelCase, like every other payload of the seam: the
// `embed-nav` event is `{ viewId, url }`.
//
// The other event this module owns carries no payload: [`NEED_VIEW_EVENT`] is a
// one-way request — "a tool needs a view" — that the MCP bridge emits and the app
// UI answers by creating the pane, with its own rect (`auto-open-pane.md`, S3).
// The wait for that answer is [`EmbedSessions::wait_for_first_view`].
//
// Popups (`drawer-responsiveness-oauth.md`, S3/L3) — a page that calls
// `window.open` or clicks `target=_blank` gets a tab INSIDE the browser pane, not
// a drawer tab and not a window of its own:
// - `on_new_window` intercepts the request on the page view. The response cannot
//   be a child webview, so the shape is two steps: build a `WebviewWindow` with
//   `window_features` (macOS needs the opener's own `WKWebViewConfiguration`, or
//   WebKit refuses the webview and there is no `window.opener`), then
//   `Webview::reparent` it into the main window. `tests/browser-popup-probe/`
//   proves both steps, `window.opener`/`postMessage` in both directions, and that
//   closing the emptied host window leaves the reparented page running.
// - The popup is a registered view like any other, with one extra field: the id of
//   the view that opened it. That link is what the UI turns into a strip tab,
//   what a close follows (a page takes its popups with it), and what keeps the
//   popup out of `ready_views` — the list the MCP bridge gives the agent.
// - **The popup cannot close itself.** macOS never tells wry that a page called
//   `window.close()` (no `webViewDidClose:` in `wry-0.57.2`), so there is no native
//   signal to react to; the human closes the tab, and that is the documented end of
//   a popup's life (`docs/ai/browser-popups.md`).
// - **Nothing here logs a url.** A popup request is where a provider's redirect
//   lands, and a redirect can carry an authorization code: every line below names
//   ids and reasons only, never a url.
// - `NewWindowResponse::Allow` is never returned: wry's own window panics on the
//   first `tauri://` navigation, and it gives the app no handle to manage. A
//   request that fails or is refused answers `Deny`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager, Url};

#[cfg(desktop)]
use tauri::webview::{NewWindowFeatures, NewWindowResponse, WebviewBuilder, WebviewWindowBuilder};
#[cfg(desktop)]
use tauri::{AppHandle, LogicalPosition, LogicalSize, WebviewUrl};
#[cfg(all(desktop, not(target_os = "linux")))]
use tauri::Rect;

/// One line on the dev terminal: `pnpm tauri dev` forwards this process's stderr,
/// which is the only place a native action can be watched from (the webview's own
/// console belongs to the DevTools pane).
///
/// Debug-only: in a release build `cfg!` is a compile-time `false`, so nothing is
/// printed and the format arguments are never evaluated. Nothing logged here
/// carries a prompt, a url or a token — only view ids, rects and booleans.
macro_rules! debug_line {
    ($($arg:tt)*) => {
        if cfg!(debug_assertions) {
            eprintln!("[nabla:embed] {}", format!($($arg)*));
        }
    };
}

/// How many embedded browsers may exist at once, popups included. Eight leaves
/// room for several pages plus chained OAuth/2FA popups without leaking webviews.
pub const MAX_VIEWS: usize = 8;

/// Standard desktop User-Agent applied to the embedded browser (and its popups)
/// so OAuth providers (Google, Microsoft, GitHub) do not reject it as an
/// unsupported embedded webview (`disallowed_useragent`).
#[cfg(target_os = "macos")]
const DESKTOP_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.3 Safari/605.1.15";
#[cfg(target_os = "windows")]
const DESKTOP_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36 Edg/134.0.0.0";
#[cfg(all(desktop, not(target_os = "macos"), not(target_os = "windows")))]
const DESKTOP_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.3 Safari/605.1.15";

/// Persistent `WKWebsiteDataStore` identifier on macOS 14+ so cookies,
/// `localStorage`, and sign-in sessions survive app restarts.
#[cfg(target_os = "macos")]
const BROWSER_DATA_STORE_ID: [u8; 16] = *b"nabla-browser-01";

/// What a view shows before it is navigated anywhere.
pub const DEFAULT_URL: &str = "about:blank";

/// Every webview label, and every view id, starts with this.
pub const VIEW_PREFIX: &str = "embed-";

/// The window the child webviews are added to: the single window of
/// `tauri.conf.json`, whose label Tauri defaults to `main`.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Channel the navigation handler reports on, one-way, to the app UI.
pub const NAV_EVENT: &str = "embed-nav";

/// Channel a tool uses to ask the app UI for a view (`auto-open-pane.md`, S3):
/// emitted with `()` and answered by a `reserve` — the UI creates the pane, and
/// the view inside it, with the rect only the UI can measure.
pub const NEED_VIEW_EVENT: &str = "embed-need-view";

/// Channel the new-window handler reports on: a page asked for a new window, and
/// the popup is now a hidden child webview the pane can adopt as a tab. Payload is
/// a [`PopupTab`] — ids only, never a url.
pub const POPUP_EVENT: &str = "embed-popup";

/// The title of the invisible window a popup is handed back in. It is never on
/// screen (it exists for as long as it takes to move the webview into the main
/// window) and it names no page.
#[cfg(desktop)]
pub const POPUP_WINDOW_TITLE: &str = "Nabla popup";

/// How many new-window requests the handler has seen. Only the dev line uses it:
/// it separates "the page asked for nothing" from "the request was refused".
static POPUP_REQUESTS: AtomicUsize = AtomicUsize::new(0);

/// How long `embed_eval` waits for the page to answer. A page that is loading or
/// gone never answers, and a command must not hang the caller forever.
const EVAL_TIMEOUT: Duration = Duration::from_secs(5);

/// A webview cannot have a zero size; a collapsed panel reports one.
const MIN_EXTENT: f64 = 1.0;

/// A placement in LOGICAL pixels, normalized once, as the platform receives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One child webview in the registry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedView {
    /// Stable id, and the webview label itself: `embed-<n>`.
    pub id: String,
    /// Last url the navigation handler reported (requirement 7). It is the url
    /// the human is looking at, not the one that was last requested.
    pub url: String,
    /// The view that opened this one, when this is a **popup**: the page called
    /// `window.open` or clicked `target=_blank`. `None` is a page view — the one
    /// the human drives and the agent may target.
    pub opener: Option<String>,
}

impl EmbeddedView {
    /// Whether this view came from a page opening a new window.
    pub fn is_popup(&self) -> bool {
        self.opener.is_some()
    }
}

/// What the app UI needs to adopt a popup as a tab of its pane: the view, and the
/// view that opened it. Ids only — a popup's url is a redirect url, and a redirect
/// url can carry an authorization code, so it is never part of this payload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PopupTab {
    pub view_id: String,
    pub opener_id: String,
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
    /// The platform refused to show or hide the view.
    Visibility(String),
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
            EmbedError::Visibility(message) => {
                write!(
                    f,
                    "embed: could not show or hide the embedded browser: {message}"
                )
            }
            EmbedError::Navigate(message) => {
                write!(
                    f,
                    "embed: could not navigate the embedded browser: {message}"
                )
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
    !url.trim_start().to_ascii_lowercase().starts_with("file:")
}

/// Which new-window requests are adopted as a tab of the pane.
///
/// Same policy as [`validate_url`] and for the same reason: what this pane shows is
/// a page on the web. `about:blank` is the exception it must keep — providers open
/// an empty window and then navigate it themselves. Everything else is refused
/// before any webview exists, and `tauri://` is refused hardest: a popup webview
/// that loads an app url is the one shape that aborts this process (wry's own
/// window panics on it), so it never reaches the platform.
fn popup_allowed(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case(DEFAULT_URL) {
        return true;
    }
    match Url::parse(trimmed) {
        Ok(parsed) => matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
        Err(_) => false,
    }
}

/// A position or size the platform accepts: a collapsed panel reports 0.
fn extent(value: f64) -> f64 {
    if value.is_finite() && value >= MIN_EXTENT {
        value
    } else {
        MIN_EXTENT
    }
}

/// A coordinate rounded to the whole logical pixel the platform places the child
/// webview on. A value that is not a finite number becomes 0 instead of reaching
/// the platform as `NaN`, which it refuses as a whole placement.
fn whole(value: f64) -> f64 {
    if value.is_finite() {
        value.round()
    } else {
        0.0
    }
}

/// The rect a placement is actually made with: whole logical pixels and a size the
/// platform accepts.
///
/// Rounding happens HERE, once, and not in the UI: the view is placed by a single
/// call, and a fractional coordinate that one layer truncates while another rounds
/// is a rect that lands a pixel away from the panel it reserves — and stays there,
/// because the pane only re-sends a rect that CHANGED. A position may be negative
/// (a panel scrolled partly off screen); only the size has a floor, which is what
/// [`extent`] guarantees.
pub fn place(x: f64, y: f64, width: f64, height: f64) -> Placed {
    Placed {
        x: whole(x),
        y: whole(y),
        width: extent(whole(width)),
        height: extent(whole(height)),
    }
}

// ── registry ───────────────────────────────────────────────────────────────

/// The child webviews this app created, by id. Cloneable because the navigation
/// handler outlives the command that built it and needs the same registry.
///
/// Two states per entry, and the difference is what a tool waits on. **Reserved**
/// is a label the registry holds before the platform has built the webview
/// (`create` reserves first, so the navigation handler already finds its id), and
/// **ready** is that same entry once `Window::add_child` answered. Only a ready
/// view is driveable: a tool that acted on a reserved one reached the platform
/// before its webview existed, which is how the FIRST `navigate` after an
/// auto-open used to fail.
#[derive(Clone)]
pub struct EmbedSessions {
    inner: Arc<Inner>,
}

struct Inner {
    /// Registered views, in creation order, each with whether the platform has
    /// finished building it.
    views: Mutex<Vec<Registered>>,
    /// Counter behind `embed-<n>`. Monotonic, so a label that was closed is not
    /// handed out again while the app runs.
    next_index: AtomicUsize,
    /// How many views left the registry. A close is the "no" a tool waiting for a
    /// pane has to respect, and it is the only reason this counter exists.
    closed: AtomicUsize,
    /// Signalled on every registry change, under `views`: a view was registered
    /// (wake a waiter) or removed (cancel it).
    changed: Condvar,
    /// Persistent profile directory for cookies/localStorage (`<data_dir>/browser-profile`).
    profile_dir: Option<std::path::PathBuf>,
}

impl Default for EmbedSessions {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbedSessions {
    pub fn new() -> EmbedSessions {
        Self::build_registry(None)
    }

    pub fn with_profile_dir(profile_dir: std::path::PathBuf) -> EmbedSessions {
        Self::build_registry(Some(profile_dir))
    }

    fn build_registry(profile_dir: Option<std::path::PathBuf>) -> EmbedSessions {
        EmbedSessions {
            inner: Arc::new(Inner {
                views: Mutex::new(Vec::new()),
                next_index: AtomicUsize::new(0),
                closed: AtomicUsize::new(0),
                changed: Condvar::new(),
                profile_dir,
            }),
        }
    }

    /// Every registered view, in creation order — reserved ones included. A copy,
    /// so a caller never holds the registry lock while it talks to a webview.
    ///
    /// This is the metadata list, not the drivable one: a label reserved for a
    /// webview the platform is still building names nothing yet. Callers that
    /// drive a view ask [`Self::ready_views`], which is why this one is only read
    /// by the tests that assert the difference.
    #[cfg(test)]
    pub fn views(&self) -> Vec<EmbeddedView> {
        lock(&self.inner.views)
            .iter()
            .map(|entry| entry.view.clone())
            .collect()
    }

    /// The views the platform has finished building, in creation order.
    ///
    /// The MCP server's list is this one, because it is the only list a tool can
    /// act on: a view that is merely reserved has no webview to navigate or
    /// evaluate in. **Popups are not in it**: a popup carries the human's live
    /// provider session (`drawer-responsiveness-oauth.md`, S3: protect human auth
    /// and agent target), so it is never offered to a tool — the page that opened
    /// it is the agent's target, and stays the only one.
    pub fn ready_views(&self) -> Vec<EmbeddedView> {
        lock(&self.inner.views)
            .iter()
            .filter(|entry| entry.ready && !entry.view.is_popup())
            .map(|entry| entry.view.clone())
            .collect()
    }

    /// Whether one id names a view a tool may drive: a PAGE the platform has
    /// finished building.
    ///
    /// A popup answers `false` even when its webview exists (see
    /// [`Self::ready_views`]): the MCP driver checks this before every act, so a
    /// guessed popup id is refused instead of driving a page the human is signing
    /// in on.
    pub fn is_ready(&self, view_id: &str) -> bool {
        lock(&self.inner.views)
            .iter()
            .find(|entry| entry.view.id == view_id)
            .is_some_and(|entry| entry.ready && !entry.view.is_popup())
    }

    /// The ready popups, with the view that opened each one.
    ///
    /// The pane adopts these on mount as well as from [`POPUP_EVENT`]: the event
    /// can fire while the UI is not listening (a reload of the dev shell, a pane
    /// that is being rebuilt), and a popup nobody adopted would stay a hidden
    /// webview with no tab.
    pub fn open_popup_tabs(&self) -> Vec<PopupTab> {
        lock(&self.inner.views)
            .iter()
            .filter(|entry| entry.ready)
            .filter_map(|entry| {
                entry.view.opener.as_ref().map(|opener| PopupTab {
                    view_id: entry.view.id.clone(),
                    opener_id: opener.clone(),
                })
            })
            .collect()
    }

    /// The popups one view opened, in creation order.
    #[cfg(test)]
    fn popups_of(&self, opener_id: &str) -> Vec<EmbeddedView> {
        lock(&self.inner.views)
            .iter()
            .filter(|entry| entry.view.opener.as_deref() == Some(opener_id))
            .map(|entry| entry.view.clone())
            .collect()
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
            .find(|entry| entry.view.id == view_id)
            .map(|entry| entry.view.clone())
    }

    /// Records the url the navigation handler just saw. `false` when the id was
    /// closed while the handler was in flight: a late event must never
    /// resurrect a view.
    pub fn set_url(&self, view_id: &str, url: &str) -> bool {
        let mut views = lock(&self.inner.views);
        match views.iter_mut().find(|entry| entry.view.id == view_id) {
            Some(entry) => {
                entry.view.url = url.to_string();
                true
            }
            None => false,
        }
    }

    /// Waits at most `timeout` for a READY view, and answers whether one is there.
    ///
    /// This is the tool-facing half of `auto-open-pane.md` (S1, S3, S4): the MCP
    /// bridge emits [`NEED_VIEW_EVENT`] and waits here while the app UI creates the
    /// pane — and therefore the view, whose registry entry `reserve` writes before
    /// the webview exists and [`Self::mark_ready`] flips once the platform has it.
    /// Nothing here invents geometry: the answer is only whether a view the tool
    /// can actually drive exists.
    ///
    /// A close wins over a view that appeared. `forget`/`drain` (and therefore
    /// `close`/`close_all`) is the human saying the pane is not there, so it
    /// cancels the wait instead of leaving it to run out the clock (S4,
    /// respect-close). The signal is snapshotted when the wait starts, so a close
    /// from before it is not its business.
    pub fn wait_for_first_view(&self, timeout: Duration) -> bool {
        let started = Instant::now();
        let closed_before = self.inner.closed.load(Ordering::SeqCst);
        let mut views = lock(&self.inner.views);
        loop {
            // Before the views on purpose: a registration and the close that
            // follows it can be two halves of "the pane came and went", and then
            // the human's no is the honest answer.
            if self.inner.closed.load(Ordering::SeqCst) != closed_before {
                return false;
            }
            if views
                .iter()
                .any(|entry| entry.ready && !entry.view.is_popup())
            {
                return true;
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                return false;
            }
            // Spurious wake-ups loop back here, so the elapsed time bounds the
            // wait, never a single `wait_timeout` call.
            let (next, _) = self
                .inner
                .changed
                .wait_timeout(views, timeout - elapsed)
                .unwrap_or_else(|poison| poison.into_inner());
            views = next;
        }
    }

    /// Takes the next free label and registers the view before the webview
    /// exists, so the navigation handler always finds its own id. Refuses over
    /// the cap without allocating anything.
    fn reserve(&self, url: String) -> Result<EmbeddedView, EmbedError> {
        self.reserve_with(url, None)
    }

    /// The popup half of [`Self::reserve`]: same cap, same monotonic label, and the
    /// opener recorded — which is what the pane turns into a tab next to its page,
    /// what a close follows, and what keeps the popup out of the agent's list.
    fn reserve_popup(&self, url: String, opener_id: &str) -> Result<EmbeddedView, EmbedError> {
        self.reserve_with(url, Some(opener_id.to_string()))
    }

    fn reserve_with(
        &self,
        url: String,
        opener: Option<String>,
    ) -> Result<EmbeddedView, EmbedError> {
        let mut views = lock(&self.inner.views);
        if views.len() >= MAX_VIEWS {
            return Err(EmbedError::TooMany(MAX_VIEWS));
        }
        // The counter is monotonic, so this loop only matters if a label could
        // ever come back: it keeps uniqueness independent of that assumption.
        let id = loop {
            let next = self.inner.next_index.fetch_add(1, Ordering::Relaxed) + 1;
            let candidate = format!("{VIEW_PREFIX}{next}");
            if !views.iter().any(|entry| entry.view.id == candidate) {
                break candidate;
            }
        };
        let view = EmbeddedView { id, url, opener };
        views.push(Registered {
            view: view.clone(),
            // The platform has not built anything yet: the entry exists for the
            // navigation handler and for the id, not for a tool to drive.
            ready: false,
        });
        // Woken while the lock is still held: a waiter cannot miss this
        // notification, because it re-checks the registry under the lock. A
        // waiter looks for a READY entry, so this wake-up is a re-check, not an
        // answer.
        self.inner.changed.notify_all();
        Ok(view)
    }

    /// Marks a reserved view as built, which is what answers a tool waiting for a
    /// pane. `false` when the id is no longer registered: a late readiness report
    /// of a view that was closed while the platform built it must never resurrect
    /// the entry.
    pub fn mark_ready(&self, view_id: &str) -> bool {
        let mut views = lock(&self.inner.views);
        let ready = match views.iter_mut().find(|entry| entry.view.id == view_id) {
            Some(entry) => {
                entry.ready = true;
                true
            }
            None => false,
        };
        if ready {
            self.inner.changed.notify_all();
        }
        ready
    }

    /// Removes one view from the registry and answers it, so the caller can
    /// close the webview it describes.
    ///
    /// A close signals even when the id is already gone: for a tool waiting on a
    /// pane, "this view is not here" and "nothing is here" are the same no.
    fn forget(&self, view_id: &str) -> Option<EmbeddedView> {
        let mut views = lock(&self.inner.views);
        self.inner.closed.fetch_add(1, Ordering::SeqCst);
        // Woken while the lock is still held, and even when nothing is removed:
        // the waiter re-checks the registry and the close counter under the lock.
        self.inner.changed.notify_all();
        take(&mut views, view_id)
    }

    /// Removes every popup opened by `roots`, transitively, and answers them.
    ///
    /// The registry half of a close: a page and the tabs it opened are one thing to
    /// the human, so closing the page closes its popups instead of leaving hidden
    /// webviews holding a provider's live session that nobody can reach. Transitive
    /// because a popup may itself have opened one.
    fn take_popups_of(&self, roots: &[String]) -> Vec<EmbeddedView> {
        let mut views = lock(&self.inner.views);
        let mut openers: Vec<String> = roots.to_vec();
        let mut taken: Vec<EmbeddedView> = Vec::new();
        loop {
            let index = views.iter().position(|entry| {
                entry
                    .view
                    .opener
                    .as_ref()
                    .is_some_and(|opener| openers.iter().any(|id| id == opener))
            });
            let Some(index) = index else { break };
            let entry = views.remove(index);
            openers.push(entry.view.id.clone());
            taken.push(entry.view);
        }
        if !taken.is_empty() {
            // Closing something is the human's no, exactly as `forget` reports it:
            // a tool waiting for a pane must not keep waiting on it.
            self.inner.closed.fetch_add(1, Ordering::SeqCst);
            self.inner.changed.notify_all();
        }
        taken
    }

    /// Removes and answers the popups whose opener is no longer registered.
    ///
    /// Such a popup is unreachable: the pane has no tab to return to, because the
    /// tab that opened it is gone. The sweep runs when a page view is created, which
    /// is the moment a stale popup would otherwise stay hidden forever (the MCP
    /// driver can evict a page, and a close can race a popup being built).
    fn reap_orphans(&self) -> Vec<EmbeddedView> {
        let mut views = lock(&self.inner.views);
        let mut removed: Vec<EmbeddedView> = Vec::new();
        loop {
            let live: Vec<String> = views.iter().map(|entry| entry.view.id.clone()).collect();
            let index = views.iter().position(|entry| match &entry.view.opener {
                Some(opener) => !live.iter().any(|id| id == opener),
                None => false,
            });
            let Some(index) = index else { break };
            removed.push(views.remove(index).view);
        }
        if !removed.is_empty() {
            self.inner.changed.notify_all();
        }
        removed
    }

    /// Drops a registry entry whose webview no longer exists, and answers what was
    /// dropped.
    ///
    /// The one caller is `mcp_server::EmbedDriver`: a tool call found the entry in
    /// this registry but no webview behind it (a creation that failed halfway, or
    /// a close that raced the call). The **registry** is what is corrected here,
    /// not the webview — the webview is already gone, that is the whole reason the
    /// driver is here — and correcting it is what keeps the same dead entry from
    /// being listed, resolved or driven again.
    ///
    /// Deliberately not a [`Self::forget`]: nobody said the pane is not there, so
    /// the close counter a tool waiting for a pane reads is left alone. Waiters are
    /// still woken, because the list they check changed.
    pub fn evict(&self, view_id: &str) -> Option<EmbeddedView> {
        let mut views = lock(&self.inner.views);
        self.inner.changed.notify_all();
        take(&mut views, view_id)
    }

    /// Removes every view and answers them, for `close_all`.
    fn drain(&self) -> Vec<EmbeddedView> {
        let mut views = lock(&self.inner.views);
        let drained = std::mem::take(&mut *views);
        self.inner.closed.fetch_add(1, Ordering::SeqCst);
        self.inner.changed.notify_all();
        drained.into_iter().map(|entry| entry.view).collect()
    }
}

/// One registry entry, and whether the platform has finished building it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Registered {
    view: EmbeddedView,
    /// `Window::add_child` answered: the label names a live child webview.
    ready: bool,
}

/// Closes the child webview a label names, and answers whether one was there.
///
/// A label with no webview behind it is not a failure: the id was already gone,
/// which is the state a close wants.
#[cfg(desktop)]
fn close_label(app: &AppHandle, view_id: &str) -> bool {
    match app.get_webview(view_id) {
        Some(webview) => webview.close().is_ok(),
        None => false,
    }
}

/// Closes the invisible window a popup was handed back in, if it is still there.
///
/// A popup exists as a top-level window for a moment, because that is the only
/// thing WebKit accepts from a new-window request; the webview is then moved into
/// the main window, so what is left is an EMPTY window. Closing it destroys nothing
/// of the popup (proven: the reparented page kept evaluating after the host window
/// raised `CloseRequested` and `Destroyed`), and it keeps the app from accumulating
/// one hidden window per OAuth flow. A page view has no window of this name, so the
/// same call is a no-op for it.
#[cfg(desktop)]
fn close_popup_window(app: &AppHandle, view_id: &str) {
    if let Some(window) = app.get_window(view_id) {
        let _ = window.close();
    }
}

/// Questions a page's new-window request and answers WebKit.
///
/// Called ON the main thread, inside WebKit's own delegate for
/// `webView:createWebViewWithConfiguration:…`, so two things shape it: it must
/// return a webview synchronously (that is the whole contract), and it must not
/// wait for the event loop (which is what `reparent` does). So the window is built
/// here — the documented shape does not deadlock; the probe measured 9 ms — and the
/// move into the drawer happens on a thread of its own, which then tells the UI.
///
/// `Allow` is never returned: wry's own native window panics on the first
/// `tauri://` navigation and gives the app no handle at all, so a request that is
/// refused or fails answers `Deny` — `window.open` then returns `null`, which every
/// provider page already handles.
#[cfg(desktop)]
fn popup_request(
    app: &AppHandle,
    embeds: &EmbedSessions,
    opener_id: &str,
    url: Url,
    features: NewWindowFeatures,
) -> NewWindowResponse<tauri::Wry> {
    let request = POPUP_REQUESTS.fetch_add(1, Ordering::Relaxed) + 1;
    // Ids and reasons only: this is where a provider's redirect lands, and a
    // redirect can carry an authorization code.
    if !popup_allowed(url.as_str()) {
        debug_line!("popup request {request} from {opener_id} refused by the url policy");
        return NewWindowResponse::Deny;
    }
    let Ok(view) = embeds.reserve_popup(DEFAULT_URL.to_string(), opener_id) else {
        debug_line!("popup request {request} from {opener_id} refused: no free view slot");
        return NewWindowResponse::Deny;
    };
    let label = view.id.clone();
    // `about:blank` and not the requested url: WebKit performs that navigation
    // itself the moment it has the webview back, and loading the url here too would
    // load the provider page twice.
    let Ok(started) = Url::parse(DEFAULT_URL) else {
        embeds.forget(&view.id);
        return NewWindowResponse::Deny;
    };
    let registry = embeds.clone();
    let emitter = app.clone();
    let navigated_id = label.clone();
    let nested_popup_app = app.clone();
    let nested_popup_registry = embeds.clone();
    let nested_opener_id = opener_id.to_string();
    let built = WebviewWindowBuilder::new(app, label.clone(), WebviewUrl::External(started))
        // The opener's own webview configuration travels here. On macOS WebKit
        // refuses a webview built with anything else, and a popup without it has no
        // `window.opener` — which is the half of S3 the provider page needs.
        .window_features(features)
        .user_agent(DESKTOP_USER_AGENT)
        // Invisible: this window exists for the microsecond it takes WebKit to hand
        // the webview back, and the webview is moved into the main window right
        // after. The pane shows it only once it has placed it on a measured rect.
        .visible(false)
        .title(POPUP_WINDOW_TITLE)
        // A popup navigates on its own: the provider's redirect arrives here, so the
        // url bookkeeping is the same as a page view's (recorded, reported, never
        // logged).
        .on_navigation(move |url| {
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
        })
        // Allow chained popups (e.g. OAuth -> SSO / 2FA) to open as managed tabs
        // under the same page opener instead of being silently blocked.
        .on_new_window(move |url, nested_features| {
            popup_request(
                &nested_popup_app,
                &nested_popup_registry,
                &nested_opener_id,
                url,
                nested_features,
            )
        })
        .build();
    match built {
        Ok(window) => {
            let app = app.clone();
            let embeds = embeds.clone();
            // Off the main thread on purpose: `reparent` sends a message to the
            // event loop and waits for it, so waiting for it HERE — inside the
            // delegate that runs on that thread — would deadlock the app.
            std::thread::spawn(move || adopt_popup(app, embeds, label));
            NewWindowResponse::Create { window }
        }
        Err(err) => {
            embeds.forget(&view.id);
            debug_line!("popup request {request} from {opener_id} could not be built: {err}");
            NewWindowResponse::Deny
        }
    }
}

/// The second half of a popup: move the webview into the main window, keep it
/// hidden, drop the window that only existed to hand it to WebKit, and tell the
/// pane that a tab is waiting for it.
///
/// The order is the point. Hiding BEFORE the move keeps a page nobody has placed yet
/// from painting over the whole app; the placeholder rect keeps the platform from
/// holding a rect the pane never measured; `mark_ready` before the event means the
/// pane is never handed an id whose webview is not there yet.
#[cfg(desktop)]
fn adopt_popup(app: AppHandle, embeds: EmbedSessions, view_id: String) {
    let give_up = |reason: &str| {
        debug_line!("popup {view_id} abandoned: {reason}");
        embeds.forget(&view_id);
        close_label(&app, &view_id);
        close_popup_window(&app, &view_id);
    };
    let Some(window) = app.get_window(MAIN_WINDOW_LABEL) else {
        return give_up("no main window");
    };
    let Some(webview) = app.get_webview(&view_id) else {
        return give_up("no webview behind the popup window");
    };
    if let Err(err) = webview.hide() {
        return give_up(&format!("could not hide it: {err}"));
    }
    if let Err(err) = webview.reparent(&window) {
        return give_up(&format!("could not move it into the main window: {err}"));
    }
    // One pixel, at the origin, hidden: the pane places the tab on the rect it
    // measured before it shows anything. Until then the platform has a placement it
    // asked for, so no layer is left guessing.
    #[cfg(target_os = "linux")]
    linux_gtk::attach_to_overlay(
        &window,
        &webview,
        &view_id,
        place(0.0, 0.0, MIN_EXTENT, MIN_EXTENT),
    );
    #[cfg(not(target_os = "linux"))]
    let _ = webview.set_bounds(Rect {
        position: LogicalPosition::new(0.0, 0.0).into(),
        size: LogicalSize::new(MIN_EXTENT, MIN_EXTENT).into(),
    });
    close_popup_window(&app, &view_id);
    if !embeds.mark_ready(&view_id) {
        // Closed while the platform was building it: a late readiness report must
        // not resurrect the entry, and nothing may stay on screen either.
        close_label(&app, &view_id);
        return;
    }
    let Some(opener_id) = embeds.get(&view_id).and_then(|view| view.opener) else {
        // Evicted or reaped between the reservation and here: nothing to adopt.
        return give_up("no opener is registered");
    };
    debug_line!("popup {view_id} adopted by {opener_id}");
    let _ = app.emit(POPUP_EVENT, PopupTab { view_id, opener_id });
}

/// The one removal step [`EmbedSessions::forget`] and [`EmbedSessions::evict`]
/// share: taken by id, under the registry lock.
fn take(views: &mut Vec<Registered>, view_id: &str) -> Option<EmbeddedView> {
    let index = views.iter().position(|entry| entry.view.id == view_id)?;
    Some(views.remove(index).view)
}

/// Whether a webview label is one this app created (`embed-<n>`).
///
/// Used to sweep orphans, so it must be true for every label [`EmbedSessions`]
/// hands out and false for the window's own webview (`main`): a sweep that
/// matched it would close the app.
pub fn is_view_label(label: &str) -> bool {
    label
        .strip_prefix(VIEW_PREFIX)
        .is_some_and(|rest| !rest.is_empty())
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
    ///
    /// The webview also answers `window.open` and `target=_blank`
    /// ([`popup_request`]): a second window is not a new pane, it is a tab of this
    /// one (`drawer-responsiveness-oauth.md`, S3/L3).
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
        // The placement is normalized once, by the same rule every later move uses
        // (`place`): a creation rect that is one pixel off the panel it reserves
        // is not corrected by any later resize, because the pane only re-sends a
        // rect that changed.
        let placed = place(x, y, width, height);
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
        let popup_app = app.clone();
        let popup_registry = self.clone();
        let opener_id = view.id.clone();
        let mut builder = WebviewBuilder::new(view.id.clone(), WebviewUrl::External(parsed))
            .incognito(false)
            .user_agent(DESKTOP_USER_AGENT)
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
            })
            // The popup half of S3: the page asks for a window and gets a tab of
            // this pane instead. The handler runs on WebKit's delegate thread, so
            // it only builds the window and returns it.
            .on_new_window(move |url, features| {
                popup_request(&popup_app, &popup_registry, &opener_id, url, features)
            });
        if let Some(dir) = &self.inner.profile_dir {
            let _ = std::fs::create_dir_all(dir);
            builder = builder.data_directory(dir.clone());
        }
        #[cfg(target_os = "macos")]
        {
            builder = builder.data_store_identifier(BROWSER_DATA_STORE_ID);
        }
        match window.add_child(
            builder,
            // A position may be negative (a panel scrolled partially off), only
            // the size has a floor the platform accepts.
            LogicalPosition::new(placed.x, placed.y),
            LogicalSize::new(placed.width, placed.height),
        ) {
            Ok(webview) => {
                // On Linux/GTK, `add_child` packs the child webview into the
                // window's vertical `default_vbox`, splitting the main UI in
                // half vertically instead of floating over the pane rect. Move
                // it into a `gtk::Overlay` positioned by exact coordinates.
                #[cfg(target_os = "linux")]
                linux_gtk::attach_to_overlay(&window, &webview, &view.id, placed);
                // Ready HERE, not when it was reserved: a tool that waited for a
                // pane must never act on a label the platform has not built yet.
                if !self.mark_ready(&view.id) {
                    webview
                        .close()
                        .map_err(|err| EmbedError::Create(err.to_string()))?;
                    return Err(EmbedError::UnknownView(view.id));
                }
                // A popup whose opener is gone is unreachable: no tab can return to
                // it, so it would stay a hidden webview holding somebody's session.
                // A new page view is the moment to sweep those.
                for orphan in self.reap_orphans() {
                    debug_line!("reaping popup {} whose opener is gone", orphan.id);
                    close_label(app, &orphan.id);
                    close_popup_window(app, &orphan.id);
                }
                debug_line!(
                    "create {} ready at {}x{} @{},{} (was reserved before the platform)",
                    view.id,
                    placed.width,
                    placed.height,
                    placed.x,
                    placed.y
                );
                Ok(view.id)
            }
            Err(err) => {
                debug_line!("create {} failed", view.id);
                self.forget(&view.id);
                Err(EmbedError::Create(err.to_string()))
            }
        }
    }

    /// Moves and resizes a view: the UI keeps it in sync with the panel rect and
    /// with every resize of that rect.
    ///
    /// ONE placement, not two: `Webview::set_position` and `Webview::set_size` are
    /// two separate messages to the event loop, so every resize frame left the
    /// platform holding the new position with the previous size — a child webview
    /// whose frame is half of the new rect, which is exactly what WebKit paints as
    /// an unpainted (black) area until the second message lands. `set_bounds`
    /// places position and size together, and it is also the only call that keeps
    /// the framework's own record of the rect (its auto-resize rates) in step with
    /// the rect the pane measured.
    ///
    /// The rect is normalized once, in [`place`]: whole logical pixels, a size the
    /// platform accepts, and `NaN`/infinite inputs kept away from it.
    pub fn set_bounds(
        &self,
        app: &AppHandle,
        view_id: &str,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), EmbedError> {
        let placed = place(x, y, width, height);
        let webview = self.webview(app, view_id)?;
        #[cfg(target_os = "linux")]
        {
            linux_gtk::update_overlay_bounds(app, &webview, view_id, placed);
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        webview
            .set_bounds(Rect {
                position: LogicalPosition::new(placed.x, placed.y).into(),
                size: LogicalSize::new(placed.width, placed.height).into(),
            })
            .map_err(|err| EmbedError::Bounds(err.to_string()))
    }

    /// Shows or hides a view without closing it. Both directions are idempotent:
    /// asking twice is the same as asking once.
    ///
    /// Why this exists: the view floats ABOVE the HTML, so `display: none` on the
    /// panel that reserves its rect would leave the page painted over the rest of
    /// the app. `Webview::hide`/`show` are the platform's own switch (wry maps
    /// them to `NSView.hidden`, `ShowWindow` and GTK show/hide), and — unlike a
    /// close — they keep the document, the history and the JS context, so the
    /// same page comes back instead of a reload.
    pub fn set_visible(
        &self,
        app: &AppHandle,
        view_id: &str,
        visible: bool,
    ) -> Result<(), EmbedError> {
        let webview = self.webview(app, view_id)?;
        let answer = if visible {
            let res = webview.show();
            #[cfg(target_os = "linux")]
            if res.is_ok() {
                linux_gtk::refresh_overlay(&webview);
            }
            res
        } else {
            webview.hide()
        };
        answer.map_err(|err| EmbedError::Visibility(err.to_string()))
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
            .map_err(|err| EmbedError::Eval(format!("the script could not be evaluated: {err}")))?;
        receiver.recv_timeout(EVAL_TIMEOUT).map_err(|_| {
            EmbedError::Eval(format!(
                "{view_id} did not answer within {}s",
                EVAL_TIMEOUT.as_secs()
            ))
        })
    }

    /// Closes one view and forgets it. Idempotent: a view that is already gone
    /// answers `false` rather than an error.
    ///
    /// The LABEL closes the child webview, and the registry is not the only way
    /// one can exist: the MCP driver evicts an entry whose webview it believes is
    /// gone, so a live webview can outlive the entry that described it. Such an
    /// orphan floats ABOVE the HTML and keeps painting over the app — the opaque
    /// rectangle — and an entry-only close could never reach it. So the close is
    /// by label, and the entry is merely forgotten first.
    pub fn close(&self, app: &AppHandle, view_id: &str) -> bool {
        // Forget first: a navigation handler firing during the close must find
        // nothing to update, and a second close answers `false`.
        let known = self.forget(view_id).is_some();
        #[cfg(target_os = "linux")]
        linux_gtk::forget_bounds(view_id);
        let mut closed = close_label(app, view_id);
        // The page and the tabs it opened are one thing to the human: closing the
        // page closes its popups, instead of leaving hidden webviews that hold a
        // provider's live session nobody can reach any more.
        for popup in self.take_popups_of(&[view_id.to_string()]) {
            #[cfg(target_os = "linux")]
            linux_gtk::forget_bounds(&popup.id);
            closed |= close_label(app, &popup.id);
            close_popup_window(app, &popup.id);
        }
        debug_line!(
            "close {view_id}: entry={known} webview={closed}{}",
            if known {
                ""
            } else {
                " (orphan, closed by label)"
            }
        );
        known || closed
    }

    /// Closes every view and answers how many were closed. Bound to the app's
    /// exit in `lib.rs`, so no webview outlives the app.
    ///
    /// Two sources, because a label is not the same thing as a registry entry: the
    /// entries this registry holds, and any child webview of ours the registry does
    /// NOT hold. The second sweep is what stops an evicted-but-alive view from
    /// keeping the app covered after a quit path started from a clean registry.
    pub fn close_all(&self, app: &AppHandle) -> usize {
        let views = self.drain();
        #[cfg(target_os = "linux")]
        linux_gtk::clear_bounds();
        let mut closed = 0;
        for view in &views {
            if close_label(app, &view.id) {
                closed += 1;
            }
        }
        let mut orphans = 0;
        for (label, webview) in app.webviews() {
            if !is_view_label(&label) {
                continue;
            }
            if views.iter().any(|view| view.id == label) {
                continue;
            }
            if webview.close().is_ok() {
                orphans += 1;
            }
        }
        // A popup's host window is named after its view, so the same prefix sweeps
        // the invisible windows a reparent left behind (a close that failed halfway,
        // or a popup nobody adopted). Closing such a window destroys an empty one —
        // its webview was moved into the main window.
        let mut windows = 0;
        for (label, window) in app.windows() {
            if !is_view_label(&label) {
                continue;
            }
            if window.close().is_ok() {
                windows += 1;
            }
        }
        closed += orphans;
        debug_line!(
            "close_all: entries={} closed={closed} orphans={orphans} popup windows={windows}",
            views.len()
        );
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

/// Linux/GTK overlay host for embedded child webviews.
///
/// On Linux, `tauri.conf.json` creates `main` as a `WebviewWindow` whose main
/// `WebKitWebView` is packed directly into `window.default_vbox()` (a vertical
/// `gtk::Box`). Calling `Window::add_child` adds the child webview into that
/// same vertical box, splitting the window 50/50 vertically and ignoring
/// logical bounds. Wrapping the main webview in a `gtk::Overlay` keeps the main
/// UI at 100% of the window while floating each `embed-*` child at its exact
/// measured `(x, y, width, height)` via `connect_get_child_position`.
#[cfg(target_os = "linux")]
mod linux_gtk {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    use gtk::prelude::*;
    use tauri::{AppHandle, Manager};

    use super::{MAIN_WINDOW_LABEL, Placed};

    const OVERLAY_WIDGET_NAME: &str = "nabla-embed-overlay";

    static BOUNDS: OnceLock<Mutex<HashMap<String, Placed>>> = OnceLock::new();

    fn bounds_map() -> &'static Mutex<HashMap<String, Placed>> {
        BOUNDS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn set_bounds_record(view_id: &str, placed: Placed) {
        bounds_map()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(view_id.to_string(), placed);
    }

    pub fn forget_bounds(view_id: &str) {
        bounds_map()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(view_id);
    }

    pub fn clear_bounds() {
        bounds_map()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    fn ensure_overlay(vbox: &gtk::Box, child_widget: &gtk::Widget) -> gtk::Overlay {
        for existing in vbox.children() {
            if let Ok(overlay) = existing.downcast::<gtk::Overlay>()
                && overlay.widget_name().as_str() == OVERLAY_WIDGET_NAME
            {
                return overlay;
            }
        }

        let overlay = gtk::Overlay::new();
        overlay.set_widget_name(OVERLAY_WIDGET_NAME);
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);

        overlay.connect_get_child_position(|_overlay, widget| {
            let name = widget.widget_name();
            let placed = bounds_map()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(name.as_str())
                .copied()?;
            Some(gtk::Allocation::new(
                placed.x.round() as i32,
                placed.y.round() as i32,
                (placed.width.round() as i32).max(1),
                (placed.height.round() as i32).max(1),
            ))
        });

        for existing in vbox.children() {
            if existing == *child_widget {
                continue;
            }
            if existing.downcast_ref::<gtk::MenuBar>().is_some() {
                continue;
            }
            vbox.remove(&existing);
            existing.set_hexpand(true);
            existing.set_vexpand(true);
            overlay.add(&existing);
            break;
        }

        vbox.pack_start(&overlay, true, true, 0);
        // `show()` instead of `show_all()` so hidden overlay children (e.g. popups)
        // stay hidden until the browser pane explicitly shows them.
        overlay.show();
        overlay
    }

    fn attach_widget_to_overlay(
        window: &tauri::Window,
        child_widget: &gtk::Widget,
        view_id: &str,
    ) {
        let Ok(vbox) = window.default_vbox() else {
            return;
        };

        if let Some(parent) = child_widget.parent() {
            let already_in_overlay = parent
                .downcast_ref::<gtk::Overlay>()
                .is_some_and(|o| o.widget_name().as_str() == OVERLAY_WIDGET_NAME);
            if already_in_overlay {
                child_widget.set_widget_name(view_id);
                parent.queue_resize();
                child_widget.queue_resize();
                return;
            }
            if let Ok(container) = parent.clone().downcast::<gtk::Container>() {
                container.remove(child_widget);
                // If wry created a helper `gtk::Fixed` inside `default_vbox` that is
                // now empty, remove it so it does not claim half of the vbox height.
                if container.downcast_ref::<gtk::Fixed>().is_some()
                    && container.children().is_empty()
                    && let Some(grandparent) = container
                        .parent()
                        .and_then(|w| w.downcast::<gtk::Container>().ok())
                {
                    grandparent.remove(&container);
                }
            }
        }

        let overlay = ensure_overlay(&vbox, child_widget);
        child_widget.set_widget_name(view_id);
        // Keep minimum size request at 1x1 so an overlay child never inflates the
        // window's minimum size; `connect_get_child_position` supplies the exact rect.
        child_widget.set_size_request(1, 1);
        overlay.add_overlay(child_widget);
        overlay.queue_resize();
        child_widget.queue_resize();
    }

    pub fn attach_to_overlay(
        window: &tauri::Window,
        webview: &tauri::Webview,
        view_id: &str,
        placed: Placed,
    ) {
        set_bounds_record(view_id, placed);
        let win = window.clone();
        let id = view_id.to_string();
        let _ = webview.with_webview(move |platform_wv| {
            let child_widget: gtk::Widget = platform_wv.inner().upcast_ref::<gtk::Widget>().clone();
            attach_widget_to_overlay(&win, &child_widget, &id);
        });
    }

    pub fn update_overlay_bounds(
        app: &AppHandle,
        webview: &tauri::Webview,
        view_id: &str,
        placed: Placed,
    ) {
        set_bounds_record(view_id, placed);
        let app = app.clone();
        let id = view_id.to_string();
        let _ = webview.with_webview(move |platform_wv| {
            let child_widget: gtk::Widget = platform_wv.inner().upcast_ref::<gtk::Widget>().clone();
            if let Some(parent) = child_widget.parent()
                && parent
                    .downcast_ref::<gtk::Overlay>()
                    .is_some_and(|o| o.widget_name().as_str() == OVERLAY_WIDGET_NAME)
            {
                child_widget.set_widget_name(&id);
                parent.queue_resize();
                child_widget.queue_resize();
            } else if let Some(win) = app.get_window(MAIN_WINDOW_LABEL) {
                attach_widget_to_overlay(&win, &child_widget, &id);
            }
        });
    }

    pub fn refresh_overlay(webview: &tauri::Webview) {
        let _ = webview.with_webview(|platform_wv| {
            let child_widget: gtk::Widget = platform_wv.inner().upcast_ref::<gtk::Widget>().clone();
            if let Some(parent) = child_widget.parent() {
                parent.queue_resize();
            }
            child_widget.queue_resize();
        });
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

    pub fn set_visible(
        &self,
        _app: &tauri::AppHandle,
        _view_id: &str,
        _visible: bool,
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
        assert_eq!(
            validate_url("https://example.com"),
            Ok("https://example.com/".into())
        );
        assert_eq!(
            validate_url("http://127.0.0.1:1420/app?x=1"),
            Ok("http://127.0.0.1:1420/app?x=1".into())
        );
        // Surrounding whitespace is trimmed, not rejected: a pasted url has it.
        assert_eq!(
            validate_url("  https://tauri.app/  "),
            Ok("https://tauri.app/".into())
        );
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
            assert!(
                message.contains(raw),
                "the message names the input: {message}"
            );
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

    // ── placement ──────────────────────────────────────────────────────────
    //
    // What the platform is told, decided once: the UI measures a CSS rect, and a
    // fractional one that one layer truncates while another rounds is a view that
    // lands a pixel off the panel and STAYS there, because the pane only re-sends a
    // rect that changed.

    #[test]
    fn place_rounds_a_fractional_rect_to_whole_logical_pixels() {
        let placed = place(240.4, 88.5, 519.6, 402.5);
        assert_eq!(placed.x, 240.0);
        assert_eq!(placed.y, 89.0);
        assert_eq!(placed.width, 520.0);
        assert_eq!(placed.height, 403.0);
        // A rect that is already whole is handed through untouched.
        assert_eq!(place(0.0, 0.0, 800.0, 600.0).width, 800.0);
    }

    #[test]
    fn place_never_hands_the_platform_a_size_below_one_pixel() {
        // A collapsed panel (a hidden drawer, a zero-height body) reports 0 or a
        // sub-pixel remainder; both become the one pixel the platform accepts.
        for (width, height) in [(0.0, 0.0), (0.2, 0.4), (-40.0, -1.0)] {
            let placed = place(10.0, 10.0, width, height);
            assert_eq!(placed.width, MIN_EXTENT, "width from {width}");
            assert_eq!(placed.height, MIN_EXTENT, "height from {height}");
        }
    }

    #[test]
    fn place_keeps_a_negative_position_and_answers_zero_for_a_non_finite_one() {
        // A panel scrolled partly off the window keeps its negative origin: the
        // platform clips it, and moving it to 0 would slide the page into view.
        assert_eq!(place(-12.0, -40.0, 300.0, 200.0).x, -12.0);
        assert_eq!(place(-12.0, -40.0, 300.0, 200.0).y, -40.0);

        // `NaN` is not a placement: 0 is a coordinate the platform accepts, and the
        // size floor still applies, so the view exists rather than vanishing.
        let placed = place(f64::NAN, f64::INFINITY, f64::NAN, f64::NAN);
        assert_eq!((placed.x, placed.y), (0.0, 0.0));
        assert_eq!((placed.width, placed.height), (MIN_EXTENT, MIN_EXTENT));
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

    // The drawer hides a view natively instead of closing it, so a refused
    // `hide`/`show` is a failure the human can read: it names the action and
    // carries the platform's own reason.
    #[test]
    fn visibility_failures_name_the_action_and_the_platform_reason() {
        let message = EmbedError::Visibility("the platform refused".into()).to_string();
        assert!(message.contains("show or hide"), "unexpected: {message}");
        assert!(
            message.contains("the platform refused"),
            "the platform's reason travels: {message}"
        );
    }

    #[test]
    fn too_many_carries_the_cap_in_its_message() {
        let message = EmbedError::TooMany(MAX_VIEWS).to_string();
        assert!(
            message.contains(&MAX_VIEWS.to_string()),
            "unexpected: {message}"
        );
        assert!(message.contains("already open"), "unexpected: {message}");
    }

    // ── last known url ─────────────────────────────────────────────────────

    #[test]
    fn set_url_records_the_last_navigation() {
        let sessions = EmbedSessions::new();
        let view = sessions.reserve(DEFAULT_URL.into()).expect("view");
        assert_eq!(
            sessions.get(&view.id).map(|v| v.url),
            Some(DEFAULT_URL.into())
        );

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

    // An eviction corrects the registry when a webview vanished under it: the
    // entry goes away, so nothing lists, resolves or drives it again.
    #[test]
    fn evict_removes_the_entry_from_the_registry() {
        let sessions = EmbedSessions::new();
        let dead = sessions.reserve(DEFAULT_URL.into()).expect("dead");
        let live = sessions
            .reserve("https://example.com/".into())
            .expect("live");

        assert_eq!(sessions.evict(&dead.id), Some(dead.clone()));
        assert_eq!(sessions.get(&dead.id), None, "the dead entry is gone");
        // A second look at the registry no longer lists it.
        assert_eq!(sessions.views(), vec![live]);
        // The navigation handler of a webview that fired late cannot resurrect it.
        assert!(!sessions.set_url(&dead.id, "https://example.com/"));
        assert_eq!(
            sessions.evict(&dead.id),
            None,
            "evicting twice is empty, not a panic"
        );
    }

    // An eviction is not the human's close: a tool waiting for a pane keeps
    // waiting instead of reading the registry correction as "the pane is not
    // there". The evicted id is one the registry never held — the second of two
    // tool calls that both found the same dead view — because that is the only
    // way an eviction can race a wait: a waiter only sleeps on an empty registry.
    #[test]
    fn evicting_is_not_a_close_and_does_not_cancel_a_wait() {
        let sessions = EmbedSessions::new();
        let waiting = {
            let sessions = sessions.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                let opened = sessions.wait_for_first_view(Duration::from_millis(300));
                (opened, started.elapsed())
            })
        };
        // The waiter is asleep by now: the eviction below is what it wakes on.
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(sessions.evict("embed-99"), None);

        let (opened, waited) = waiting.join().expect("the waiting thread panicked");
        assert!(!opened, "no view appeared, so the answer is still no");
        assert!(
            waited >= Duration::from_millis(300),
            "the wait ran out instead of being cancelled as a close would, took {waited:?}"
        );
    }

    #[test]
    fn drain_closes_the_registry_in_one_step() {
        let sessions = EmbedSessions::new();
        sessions.reserve(DEFAULT_URL.into()).expect("first");
        sessions
            .reserve("https://example.com/".into())
            .expect("second");
        let drained = sessions.drain();
        assert_eq!(drained.len(), 2);
        assert!(sessions.is_empty(), "close_all leaves no view behind");
        assert!(
            sessions.drain().is_empty(),
            "draining twice is empty, not a panic"
        );
    }

    // ── waiting for a pane (`auto-open-pane.md`, S1/S3/S4) ───────────────────
    //
    // The MCP bridge asks the UI for a view and waits here. What is verified is
    // every decision that does not need a window: a view that appears answers,
    // silence is bounded, and a close is a no rather than a longer wait.

    #[test]
    fn waiting_for_first_view_answers_at_once_when_one_is_open() {
        let sessions = EmbedSessions::new();
        let view = sessions.reserve(DEFAULT_URL.into()).expect("an open view");
        assert!(sessions.mark_ready(&view.id), "the platform built it");

        let started = Instant::now();
        assert!(sessions.wait_for_first_view(Duration::from_secs(10)));
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "an open view is not a wait, took {:?}",
            started.elapsed()
        );
    }

    // The reservation is not the view. `create` registers the label BEFORE
    // `add_child` runs (so the navigation handler finds its id), and a tool that
    // acted on that reservation reached the platform before the webview existed —
    // the first `navigate` after an auto-open failed, the registry then dropped the
    // entry as dead, and the webview that DID get built stayed on screen with
    // nobody able to close it: the opaque rectangle over the app.
    #[test]
    fn a_reserved_view_does_not_answer_the_wait_for_a_pane() {
        let sessions = EmbedSessions::new();
        sessions.reserve(DEFAULT_URL.into()).expect("reserved");

        let started = Instant::now();
        let opened = sessions.wait_for_first_view(Duration::from_millis(120));
        let waited = started.elapsed();

        assert!(
            !opened,
            "a label the platform has not built is not a view a tool can drive"
        );
        assert!(
            waited >= Duration::from_millis(120),
            "the wait runs to its bound instead of believing the reservation, took {waited:?}"
        );
    }

    #[test]
    fn mark_ready_answers_the_wait_a_reservation_only_woke() {
        let sessions = EmbedSessions::new();
        let creating = sessions.clone();
        let ui = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            // What the pane's `embed_create` does first: the view exists in the
            // registry before the webview does.
            let reserved = creating
                .reserve(DEFAULT_URL.into())
                .expect("the pane's create");
            assert!(!creating.is_ready(&reserved.id), "reserved first");
            std::thread::sleep(Duration::from_millis(50));
            // ... and this is `add_child` answering.
            assert!(creating.mark_ready(&reserved.id));
            reserved
        });

        let started = Instant::now();
        let opened = sessions.wait_for_first_view(Duration::from_secs(10));
        let waited = started.elapsed();
        ui.join().expect("the creating thread panicked");

        assert!(opened, "the built view answers the wait");
        assert!(
            waited >= Duration::from_millis(90),
            "it waited for the platform, not for the label, returned after {waited:?}"
        );
    }

    // The MCP driver lists and drives ready views only: a reserved label names no
    // webview yet, so listing it would hand the model an id that answers
    // "no embedded browser" the moment it is used.
    #[test]
    fn ready_views_lists_only_what_the_platform_has_built() {
        let sessions = EmbedSessions::new();
        let reserved = sessions.reserve(DEFAULT_URL.into()).expect("reserved");
        let built = sessions
            .reserve("https://example.com/".into())
            .expect("built");
        assert!(sessions.mark_ready(&built.id));

        assert_eq!(sessions.views(), vec![reserved.clone(), built.clone()]);
        assert_eq!(sessions.ready_views(), vec![built.clone()]);
        assert!(sessions.is_ready(&built.id));
        assert!(!sessions.is_ready(&reserved.id));
        assert!(
            !sessions.is_ready("embed-99"),
            "an unknown id is never ready"
        );
    }

    // A creation racing a close: the platform answers after the pane is gone. The
    // late report must not resurrect the entry, or the next tool would resolve a
    // view the human already closed.
    #[test]
    fn a_late_readiness_report_does_not_resurrect_a_closed_view() {
        let sessions = EmbedSessions::new();
        let view = sessions.reserve(DEFAULT_URL.into()).expect("reserved");
        assert!(sessions.forget(&view.id).is_some());

        assert!(!sessions.mark_ready(&view.id), "nothing to mark");
        assert_eq!(sessions.get(&view.id), None);
        assert!(sessions.ready_views().is_empty());
    }

    // Only labels `create` hands out are ours: the sweep `close_all` runs closes
    // every `embed-` webview it finds, so matching the window's own webview would
    // close the app.
    #[test]
    fn only_embed_labels_belong_to_this_app() {
        assert!(is_view_label("embed-1"));
        assert!(is_view_label("embed-42"));
        assert!(!is_view_label(MAIN_WINDOW_LABEL));
        assert!(!is_view_label("embed"));
        assert!(!is_view_label("embed-"));
        assert!(!is_view_label(""));
        assert!(!is_view_label("my-embed-1"));
    }

    #[test]
    fn waiting_for_first_view_times_out_when_nothing_is_registered() {
        let sessions = EmbedSessions::new();

        let started = Instant::now();
        let opened = sessions.wait_for_first_view(Duration::from_millis(80));
        let waited = started.elapsed();

        assert!(!opened, "silence is a no, not an invented view");
        assert!(
            waited >= Duration::from_millis(80),
            "the wait is the timeout it was given"
        );
        assert!(
            waited < Duration::from_secs(5),
            "and no longer than that, took {waited:?}"
        );
    }

    /// Runs `close` on another thread once the waiter is inside its wait, so the
    /// close is what the wait has to observe.
    fn abort_with(closing: impl FnOnce(EmbedSessions) + Send + 'static) -> bool {
        let sessions = EmbedSessions::new();
        let waiting = {
            let sessions = sessions.clone();
            std::thread::spawn(move || sessions.wait_for_first_view(Duration::from_secs(10)))
        };
        // The waiter is asleep in its wait by now: nothing registers in these
        // tests, so the close cannot race with a view appearing.
        std::thread::sleep(Duration::from_millis(80));
        closing(sessions.clone());
        let opened = waiting.join().expect("the waiting thread panicked");
        assert!(sessions.is_empty(), "the helper never registers a view");
        opened
    }

    // S4 (respect-close): the human closing the pane while the tool waits is a
    // no, answered now instead of after the whole timeout.
    #[test]
    fn a_close_during_the_wait_aborts_it() {
        let started = Instant::now();
        let opened = abort_with(|sessions| {
            // A close names a view; the registry still answers it as the human's
            // "the pane is not there" even when that view is already gone.
            assert_eq!(sessions.forget("embed-1"), None);
        });

        assert!(!opened, "a close means no view, not a longer wait");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "it aborted instead of running out the timeout, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_close_all_during_the_wait_aborts_it() {
        let started = Instant::now();
        let opened = abort_with(|sessions| {
            // `close_all` with nothing registered still says the panes are gone.
            assert!(sessions.drain().is_empty());
        });

        assert!(!opened, "close_all means no view, not a longer wait");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "it aborted instead of running out the timeout, took {:?}",
            started.elapsed()
        );
    }

    // A close is only the wait's business while the wait runs: the snapshot is
    // taken when it starts, so an earlier close cannot cancel a later wait.
    #[test]
    fn a_close_before_the_wait_is_not_its_business() {
        let sessions = EmbedSessions::new();
        let gone = sessions.reserve(DEFAULT_URL.into()).expect("view");
        assert!(sessions.forget(&gone.id).is_some());

        let creating = sessions.clone();
        let ui = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            let view = creating.reserve(DEFAULT_URL.into()).expect("a later view");
            assert!(creating.mark_ready(&view.id));
        });

        assert!(sessions.wait_for_first_view(Duration::from_secs(10)));
        ui.join().expect("the creating thread panicked");
    }

    // ── popups: tabs inside the pane (`drawer-responsiveness-oauth.md`, S3/L3) ──
    //
    // A page that calls `window.open` or clicks `target=_blank` gets a child
    // webview of the main window like any other view, with the one difference that
    // decides everything else: it is registered with the view that opened it. That
    // link is what the pane turns into a strip tab, what a close follows, and what
    // keeps the popup out of the agent's list. Nothing below needs a window; the
    // native half (create, reparent, hide, close) was proven by the probe in
    // `tests/browser-popup-probe/` and is manually verified in `npm run tauri dev`
    // (`docs/ai/browser-popups.md`).

    #[test]
    fn a_popup_is_registered_with_the_view_that_opened_it() {
        let sessions = EmbedSessions::new();
        let page = sessions.reserve(DEFAULT_URL.into()).expect("page");
        let popup = sessions
            .reserve_popup(DEFAULT_URL.into(), &page.id)
            .expect("popup");

        assert_eq!(popup.opener.as_deref(), Some(page.id.as_str()));
        assert!(popup.is_popup());
        assert!(!page.is_popup(), "the page the human drives is not a popup");
        assert_eq!(sessions.get(&popup.id), Some(popup.clone()));
        // A popup is a view like any other for the close sweep: its label is what
        // `close_all` matches and what the pane calls `embed_close` with.
        assert!(is_view_label(&popup.id));
        assert_ne!(popup.id, page.id, "a popup has a label of its own");
    }

    // S3's last clause: the popup carries the human's live provider session, so it
    // must never become an agent target. `ready_views` is the list the MCP bridge
    // gives a tool, and `is_ready` is what the driver checks before acting.
    #[test]
    fn a_ready_popup_is_neither_listed_nor_driveable() {
        let sessions = EmbedSessions::new();
        let popup = sessions
            .reserve_popup("https://accounts.example.com/".into(), "embed-404")
            .expect("popup");
        assert!(sessions.mark_ready(&popup.id), "the platform built it");

        assert!(sessions.ready_views().is_empty(), "the agent never sees it");
        assert!(!sessions.is_ready(&popup.id), "and never drives it");

        // A tool waiting for a pane is not answered by a popup either: the wait is
        // about a view the agent can act on.
        let started = Instant::now();
        assert!(!sessions.wait_for_first_view(Duration::from_millis(60)));
        assert!(started.elapsed() >= Duration::from_millis(60));

        // The page view the human does drive is still listed after a popup exists.
        let page = sessions.reserve(DEFAULT_URL.into()).expect("page");
        assert!(sessions.mark_ready(&page.id));
        assert_eq!(sessions.ready_views(), vec![page.clone()]);
        assert!(sessions.is_ready(&page.id));
    }

    #[test]
    fn closing_a_page_takes_the_popups_it_opened() {
        let sessions = EmbedSessions::new();
        let page = sessions.reserve(DEFAULT_URL.into()).expect("page");
        let first = sessions
            .reserve_popup(DEFAULT_URL.into(), &page.id)
            .expect("first");
        let second = sessions
            .reserve_popup(DEFAULT_URL.into(), &page.id)
            .expect("second");
        let nested = sessions
            .reserve_popup(DEFAULT_URL.into(), &first.id)
            .expect("nested");

        let mut closed: Vec<String> = sessions
            .take_popups_of(std::slice::from_ref(&page.id))
            .into_iter()
            .map(|view| view.id)
            .collect();
        closed.sort();
        let mut expected = vec![first.id.clone(), second.id.clone(), nested.id.clone()];
        expected.sort();
        assert_eq!(closed, expected, "the page and its popups are one thing");
        assert_eq!(
            sessions.get(&page.id),
            Some(page),
            "the page itself is not touched"
        );
        assert_eq!(sessions.len(), 1, "nothing else was registered");
    }

    // Which popups belong to which view: the pane draws a tab under the tab that
    // opened it, so "the popups of this view" has to be exactly that view's.
    #[test]
    fn popups_of_names_only_the_popups_of_its_opener() {
        let sessions = EmbedSessions::new();
        let page = sessions.reserve(DEFAULT_URL.into()).expect("page");
        let first = sessions
            .reserve_popup(DEFAULT_URL.into(), &page.id)
            .expect("first");
        let second = sessions
            .reserve_popup(DEFAULT_URL.into(), &page.id)
            .expect("second");
        let nested = sessions
            .reserve_popup(DEFAULT_URL.into(), &first.id)
            .expect("nested");

        assert_eq!(
            sessions.popups_of(&page.id),
            vec![first.clone(), second.clone()]
        );
        assert_eq!(sessions.popups_of(&first.id), vec![nested]);
        assert!(sessions.popups_of(&second.id).is_empty());
        assert!(sessions.popups_of("embed-99").is_empty());
    }

    #[test]
    fn closing_a_popup_takes_the_popups_it_opened() {
        let sessions = EmbedSessions::new();
        let page = sessions.reserve(DEFAULT_URL.into()).expect("page");
        let popup = sessions
            .reserve_popup(DEFAULT_URL.into(), &page.id)
            .expect("popup");
        let nested = sessions
            .reserve_popup(DEFAULT_URL.into(), &popup.id)
            .expect("nested");
        let sibling = sessions
            .reserve_popup(DEFAULT_URL.into(), &page.id)
            .expect("sibling");

        let closed: Vec<String> = sessions
            .take_popups_of(std::slice::from_ref(&popup.id))
            .into_iter()
            .map(|view| view.id)
            .collect();
        assert_eq!(
            closed,
            vec![nested.id.clone()],
            "a popup's own popups go with it"
        );
        assert_eq!(
            sessions.get(&sibling.id),
            Some(sibling),
            "a sibling popup is not a child"
        );
        assert_eq!(sessions.get(&popup.id), Some(popup));
    }

    #[test]
    fn open_popup_tabs_lists_only_the_popups_the_platform_built() {
        let sessions = EmbedSessions::new();
        let page = sessions.reserve(DEFAULT_URL.into()).expect("page");
        assert!(sessions.mark_ready(&page.id));
        let popup = sessions
            .reserve_popup(DEFAULT_URL.into(), &page.id)
            .expect("popup");

        // Reserved is not built: the pane could not place a view that has no
        // webview yet, so it is not adopted early.
        assert!(sessions.open_popup_tabs().is_empty());

        assert!(sessions.mark_ready(&popup.id));
        assert_eq!(
            sessions.open_popup_tabs(),
            vec![PopupTab {
                view_id: popup.id.clone(),
                opener_id: page.id.clone(),
            }]
        );
        // The payload carries ids only: a popup's url is a redirect url, and a
        // redirect url can carry a code.
        assert_eq!(
            serde_json::to_string(&sessions.open_popup_tabs()[0]).expect("json"),
            format!(
                "{{\"viewId\":\"{}\",\"openerId\":\"{}\"}}",
                popup.id, page.id
            )
        );
    }

    // A popup can outlive the entry that opened it: the MCP driver evicts a view
    // whose webview it believes is gone, and a page can be closed by hand while its
    // popup was being built. Such a popup is unreachable — the pane has no tab to
    // return to — so the next creation reaps it instead of leaving a hidden webview
    // holding somebody's session.
    #[test]
    fn a_popup_whose_opener_left_is_reaped() {
        let sessions = EmbedSessions::new();
        let page = sessions.reserve(DEFAULT_URL.into()).expect("page");
        let popup = sessions
            .reserve_popup(DEFAULT_URL.into(), &page.id)
            .expect("popup");
        let nested = sessions
            .reserve_popup(DEFAULT_URL.into(), &popup.id)
            .expect("nested");
        let other_page = sessions.reserve(DEFAULT_URL.into()).expect("other page");

        assert_eq!(sessions.forget(&page.id), Some(page));
        let mut reaped: Vec<String> = sessions
            .reap_orphans()
            .into_iter()
            .map(|view| view.id)
            .collect();
        reaped.sort();
        let mut expected = vec![popup.id.clone(), nested.id.clone()];
        expected.sort();
        assert_eq!(reaped, expected, "the child of an orphan is an orphan too");
        assert_eq!(
            sessions.views(),
            vec![other_page],
            "what still has its opener stays"
        );
        assert!(sessions.reap_orphans().is_empty(), "nothing left to reap");
    }

    #[test]
    fn popups_count_against_the_view_cap() {
        let sessions = EmbedSessions::new();
        let page = sessions.reserve(DEFAULT_URL.into()).expect("page");
        for _ in 1..MAX_VIEWS {
            sessions
                .reserve_popup(DEFAULT_URL.into(), &page.id)
                .expect("room for one more");
        }
        assert_eq!(
            sessions.reserve_popup(DEFAULT_URL.into(), &page.id),
            Err(EmbedError::TooMany(MAX_VIEWS)),
            "a popup is a webview: the cap is the cap"
        );
        assert_eq!(sessions.len(), MAX_VIEWS, "a refused popup leaves nothing");
    }

    // The url policy of a popup request. `tauri://` is the one that matters most:
    // a popup that loads an app url is the shape that aborts this process, so it is
    // refused before any webview exists rather than trusted.
    #[test]
    fn only_web_popups_are_adopted() {
        for allowed in [
            "https://accounts.example.com/authorize?client_id=abc",
            "http://127.0.0.1:8080/callback",
            "about:blank",
            "  about:blank  ",
        ] {
            assert!(popup_allowed(allowed), "expected {allowed:?} to be adopted");
        }
        for denied in [
            "tauri://localhost/index.html",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<h1>x</h1>",
            "about:config",
            "https://",
            "",
        ] {
            assert!(!popup_allowed(denied), "expected {denied:?} to be refused");
        }
    }

    // A late navigation of a popup that is gone must not resurrect it, exactly as
    // for a page: the navigation handler of a closed webview can still fire.
    #[test]
    fn a_closed_popup_is_not_resurrected_by_a_late_navigation() {
        let sessions = EmbedSessions::new();
        let page = sessions.reserve(DEFAULT_URL.into()).expect("page");
        let popup = sessions
            .reserve_popup(DEFAULT_URL.into(), &page.id)
            .expect("popup");
        let closed: Vec<String> = sessions
            .take_popups_of(std::slice::from_ref(&page.id))
            .into_iter()
            .map(|view| view.id)
            .collect();
        assert_eq!(closed, vec![popup.id.clone()]);
        assert!(!sessions.set_url(&popup.id, "https://example.com/"));
        assert_eq!(sessions.get(&popup.id), None);
        assert!(sessions.open_popup_tabs().is_empty());
    }

    // Nothing below runs without a real window: `embed_create`, `set_bounds`,
    // `navigate`, `eval`, `close` and `close_all` need Tauri's event loop, a
    // WKWebView and a main thread, so they are manually verified in
    // `npm run tauri dev`. What is left here is every decision that does not need
    // the platform: url validation, label allocation, the cap and the url
    // bookkeeping the navigation handler performs.
}
