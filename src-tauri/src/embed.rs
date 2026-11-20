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
// - **Linux.** wry builds child webviews on webkit2gtk, but positioning and
//   transparency differ from macOS and were not exercised by this feature; a
//   GTK window may also need the child to be reparented on resize.
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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager, Url};

#[cfg(desktop)]
use tauri::webview::WebviewBuilder;
#[cfg(desktop)]
use tauri::{AppHandle, LogicalPosition, LogicalSize, Rect, WebviewUrl};

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

/// Channel a tool uses to ask the app UI for a view (`auto-open-pane.md`, S3):
/// emitted with `()` and answered by a `reserve` — the UI creates the pane, and
/// the view inside it, with the rect only the UI can measure.
pub const NEED_VIEW_EVENT: &str = "embed-need-view";

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
                write!(f, "embed: could not show or hide the embedded browser: {message}")
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
    /// How many views left the registry. A close is the "no" a tool waiting for a
    /// pane has to respect, and it is the only reason this counter exists.
    closed: AtomicUsize,
    /// Signalled on every registry change, under `views`: a view was registered
    /// (wake a waiter) or removed (cancel it).
    changed: Condvar,
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
                closed: AtomicUsize::new(0),
                changed: Condvar::new(),
            }),
        }
    }

    /// Every registered view, in creation order. A copy, so a caller never holds
    /// the registry lock while it talks to a webview.
    ///
    /// The app itself addresses one view by id; the caller that needs the whole
    /// list is the MCP server, which has to tell the agent which views exist
    /// before it can drive one (`webview-control.md` T1).
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

    /// Waits at most `timeout` for a view to be registered, and answers whether
    /// one is there.
    ///
    /// This is the tool-facing half of `auto-open-pane.md` (S1, S3, S4): the MCP
    /// bridge emits [`NEED_VIEW_EVENT`] and waits here while the app UI creates the
    /// pane — and therefore the view, whose registry entry `reserve` writes before
    /// the webview exists. Nothing here invents geometry: the answer is only
    /// whether a view exists.
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
            if !views.is_empty() {
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
        // Woken while the lock is still held: a waiter cannot miss this
        // notification, because it re-checks the registry under the lock.
        self.inner.changed.notify_all();
        Ok(view)
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
        drained
    }
}

/// The one removal step [`EmbedSessions::forget`] and [`EmbedSessions::evict`]
/// share: taken by id, under the registry lock.
fn take(views: &mut Vec<EmbeddedView>, view_id: &str) -> Option<EmbeddedView> {
    let index = views.iter().position(|view| view.id == view_id)?;
    Some(views.remove(index))
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
            LogicalPosition::new(placed.x, placed.y),
            LogicalSize::new(placed.width, placed.height),
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
            webview.show()
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

    // An eviction corrects the registry when a webview vanished under it: the
    // entry goes away, so nothing lists, resolves or drives it again.
    #[test]
    fn evict_removes_the_entry_from_the_registry() {
        let sessions = EmbedSessions::new();
        let dead = sessions.reserve(DEFAULT_URL.into()).expect("dead");
        let live = sessions.reserve("https://example.com/".into()).expect("live");

        assert_eq!(sessions.evict(&dead.id), Some(dead.clone()));
        assert_eq!(sessions.get(&dead.id), None, "the dead entry is gone");
        // A second look at the registry no longer lists it.
        assert_eq!(sessions.views(), vec![live]);
        // The navigation handler of a webview that fired late cannot resurrect it.
        assert!(!sessions.set_url(&dead.id, "https://example.com/"));
        assert_eq!(sessions.evict(&dead.id), None, "evicting twice is empty, not a panic");
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
        sessions.reserve("https://example.com/".into()).expect("second");
        let drained = sessions.drain();
        assert_eq!(drained.len(), 2);
        assert!(sessions.is_empty(), "close_all leaves no view behind");
        assert!(sessions.drain().is_empty(), "draining twice is empty, not a panic");
    }

    // ── waiting for a pane (`auto-open-pane.md`, S1/S3/S4) ───────────────────
    //
    // The MCP bridge asks the UI for a view and waits here. What is verified is
    // every decision that does not need a window: a view that appears answers,
    // silence is bounded, and a close is a no rather than a longer wait.

    #[test]
    fn waiting_for_first_view_answers_at_once_when_one_is_open() {
        let sessions = EmbedSessions::new();
        sessions.reserve(DEFAULT_URL.into()).expect("an open view");

        let started = Instant::now();
        assert!(sessions.wait_for_first_view(Duration::from_secs(10)));
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "an open view is not a wait, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn waiting_for_first_view_resolves_when_a_view_is_reserved_elsewhere() {
        let sessions = EmbedSessions::new();
        let creating = sessions.clone();
        let ui = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            // What the pane's `embed_create` does first: the view exists in the
            // registry before the webview does.
            creating.reserve(DEFAULT_URL.into()).expect("the pane's create")
        });

        let started = Instant::now();
        let opened = sessions.wait_for_first_view(Duration::from_secs(10));
        let waited = started.elapsed();
        ui.join().expect("the creating thread panicked");

        assert!(opened, "a registered view answers the wait");
        assert!(
            waited >= Duration::from_millis(30),
            "it waited for the registration instead of guessing, returned after {waited:?}"
        );
    }

    #[test]
    fn waiting_for_first_view_times_out_when_nothing_is_registered() {
        let sessions = EmbedSessions::new();

        let started = Instant::now();
        let opened = sessions.wait_for_first_view(Duration::from_millis(80));
        let waited = started.elapsed();

        assert!(!opened, "silence is a no, not an invented view");
        assert!(waited >= Duration::from_millis(80), "the wait is the timeout it was given");
        assert!(waited < Duration::from_secs(5), "and no longer than that, took {waited:?}");
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
            creating.reserve(DEFAULT_URL.into()).expect("a later view")
        });

        assert!(sessions.wait_for_first_view(Duration::from_secs(10)));
        ui.join().expect("the creating thread panicked");
    }

    // Nothing below runs without a real window: `embed_create`, `set_bounds`,
    // `navigate`, `eval`, `close` and `close_all` need Tauri's event loop, a
    // WKWebView and a main thread, so they are manually verified in
    // `npm run tauri dev`. What is left here is every decision that does not need
    // the platform: url validation, label allocation, the cap and the url
    // bookkeeping the navigation handler performs.
}
