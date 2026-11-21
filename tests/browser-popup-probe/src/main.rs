//! Isolated macOS viability probe for popup tabs in the embedded browser
//! (`odd/tasks/drawer-responsiveness-oauth.md`, T3 / S3).
//!
//! WHAT IT PROVES — whether a Tauri *child webview* (the drawer's embedded browser,
//! `src-tauri/src/embed.rs`) can hand `window.open` / `target=_blank` to the app and
//! keep the three things a real OAuth popup depends on:
//!
//! 1. `window.opener` exists inside the popup (the provider page posts the result
//!    back with `window.opener.postMessage`),
//! 2. `postMessage` crosses both directions,
//! 3. `window.close()` from inside the popup ends it (the drawer tab must go away),
//!
//! and whether the popup can live INSIDE the drawer, i.e. as a child webview of the
//! main window at a drawer rect, instead of as a separate top-level window.
//!
//! THE FLOW IS LOCAL AND SYNTHETIC — the opener document is the app asset in
//! `assets/index.html`, the popup is `about:blank`, every step is a `eval` from this
//! file. No network, no external account, no credentials, no third-party page and
//! no injection into a remote document. The window is on screen for a few seconds.
//!
//! HOW TO READ IT — one JSON object per line, on stdout and appended to
//! `$NABLA_PROBE_OUT` / `$TMPDIR/nabla-popup-probe.jsonl`:
//! `{"step": "...", "ok": true|false, "detail": "..."}`. `docs/ai/browser-popups.md`
//! records what the run on macOS answered.
//!
//! Run: `cargo run --manifest-path tests/browser-popup-probe/Cargo.toml`
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, Rect, Url, WebviewUrl, WebviewWindowBuilder,
};

/// The single window of the probe; the drawer's child webview is added to it.
const MAIN: &str = "main";
/// The child webview that plays the embedded browser of the drawer.
const OPENER: &str = "embed-1";
/// One writer at a time: window events arrive on the main thread while the probe
/// thread writes its own records, and two interleaved lines are not a log.
static LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// How many new-window requests the handler has seen.
static POPUP_REQUESTS: AtomicUsize = AtomicUsize::new(0);
/// How the handler answers: 0 = `Create` (popup as a Tauri window, moved into the
/// drawer), 1 = `Allow` (wry's own native window). Flipped by the probe thread.
static ALLOW_NATIVE_WINDOW: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn out_path() -> std::path::PathBuf {
    std::env::var_os("NABLA_PROBE_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("nabla-popup-probe.jsonl"))
}

/// One observed fact: a step name, whether it holds, and the evidence.
fn record(step: &str, ok: bool, detail: impl AsRef<str>) {
    let _guard = LOG_LOCK.lock();
    let detail = detail
        .as_ref()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ");
    let line = format!("{{\"step\":\"{step}\",\"ok\":{ok},\"detail\":\"{detail}\"}}");
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
    // The file is the record that survives the window: stdout may be lost when the
    // process exits from the watchdog.
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out_path())
    {
        let _ = writeln!(file, "{line}");
    }
}

/// `Webview::eval` fires and forgets; this is the answering form, with a bound so a
/// page that is loading or gone cannot hang the probe.
fn eval(webview: &tauri::Webview, script: &str) -> Result<String, String> {
    let (tx, rx) = mpsc::channel::<String>();
    webview
        .eval_with_callback(script.to_string(), move |result| {
            let _ = tx.send(result);
        })
        .map_err(|err| err.to_string())?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "no answer within 5s".to_string())
}

/// `eval_with_callback` hands back the JSON serialization of the result, so a JS
/// string arrives quoted (`"true"`). This is the bare value.
fn plain(answer: &str) -> &str {
    answer.trim().trim_matches('"')
}

/// One recorded `eval`: the answer on success, the platform's own message on failure.
fn step_eval(step: &str, webview: &tauri::Webview, script: &str) -> Option<String> {
    match eval(webview, script) {
        Ok(answer) => {
            record(step, true, format!("answer={answer}"));
            Some(answer)
        }
        Err(err) => {
            record(step, false, format!("error={err}"));
            None
        }
    }
}

/// Waits until a webview answers at all (a document that has not loaded does not).
fn wait_for_eval(webview: &tauri::Webview, script: &str, budget: Duration) -> Option<String> {
    let started = Instant::now();
    let mut last = String::new();
    while started.elapsed() < budget {
        match eval(webview, script) {
            Ok(answer) => return Some(answer),
            Err(err) => last = err,
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    record(
        "eval_never_answered",
        false,
        format!("script={script} last_error={last}"),
    );
    None
}

/// Records the end of the run and leaves: the app exits through the event loop, and
/// the process leaves after it, so a stuck loop cannot keep the probe alive.
fn finish(app: &AppHandle, note: &str) {
    record("done", true, note);
    app.exit(0);
    std::thread::sleep(Duration::from_millis(600));
    std::process::exit(0);
}

fn main() {
    let _ = std::fs::remove_file(out_path());
    record(
        "env",
        true,
        format!(
            "out={} macos={} host={}",
            out_path().display(),
            std::process::Command::new("sw_vers")
                .arg("-productVersion")
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .unwrap_or_else(|_| "unknown".into()),
            std::process::Command::new("uname")
                .arg("-m")
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .unwrap_or_else(|_| "unknown".into()),
        ),
    );

    // The handler below runs on the main thread (WebKit's delegate), so a blocked
    // main thread is the failure mode to bound: the watchdog records it and exits.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(180));
        record(
            "watchdog",
            false,
            "the probe never finished: the main thread is blocked",
        );
        std::process::exit(9);
    });

    let app = tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let window = tauri::window::WindowBuilder::new(app, MAIN)
                .title("nabla popup probe")
                .inner_size(1000.0, 760.0)
                .build()?;

            // The handler sends the popup's label, which is how the probe thread
            // learns that the delegate fired and what to evaluate next.
            let (tx, rx) = mpsc::channel::<String>();
            let handler_handle = handle.clone();
            let opener_builder = WebviewBuilder::new(OPENER, WebviewUrl::App("index.html".into()))
                .on_new_window(move |url, features| {
                    let request = POPUP_REQUESTS.fetch_add(1, Ordering::SeqCst) + 1;
                    let label = format!("popup-{request}");
                    record(
                        "on_new_window",
                        true,
                        format!(
                            "request={request} url={url} size={:?} position={:?}",
                            features.size(),
                            features.position()
                        ),
                    );
                    if ALLOW_NATIVE_WINDOW.load(Ordering::SeqCst) {
                        // The comparison shape: let wry open its own window.
                        record("on_new_window_answered", true, "allow (wry's own window)");
                        return NewWindowResponse::Allow;
                    }
                    record("on_new_window_answered", true, "create (tauri window)");
                    // The documented shape: a WebviewWindow built with the opener's
                    // window features, which on macOS carries the opener's own
                    // WKWebViewConfiguration (`with_webview_configuration`) — the
                    // one thing WebKit requires of the webview it accepts back.
                    let started = Instant::now();
                    let built = WebviewWindowBuilder::new(
                        &handler_handle,
                        label.clone(),
                        WebviewUrl::External(
                            Url::parse("about:blank").expect("about:blank parses"),
                        ),
                    )
                    .window_features(features)
                    .visible(false)
                    .title(format!("probe popup {request}"))
                    .build();
                    match &built {
                        Ok(popup) => {
                            // Does a JS close of the popup surface as a window event?
                            // Nothing else can tell the drawer tab to go away.
                            popup.on_window_event(|event| {
                                record("popup_window_event", true, format!("{event:?}"));
                            });
                            record(
                                "popup_build",
                                true,
                                format!(
                                    "label={label} elapsed_ms={}",
                                    started.elapsed().as_millis()
                                ),
                            );
                        }
                        Err(err) => record(
                            "popup_build",
                            false,
                            format!(
                                "label={label} elapsed_ms={} error={err}",
                                started.elapsed().as_millis()
                            ),
                        ),
                    }
                    match built {
                        Ok(popup) => {
                            let _ = tx.send(label);
                            NewWindowResponse::Create { window: popup }
                        }
                        Err(_) => NewWindowResponse::Deny,
                    }
                });

            let opener = window.add_child(
                opener_builder,
                LogicalPosition::new(20.0, 60.0),
                LogicalSize::new(900.0, 600.0),
            )?;
            record("add_child_opener", true, format!("label={}", opener.label()));

            // Off the main thread: `eval_with_callback` answers ON the main thread,
            // so waiting for it there would deadlock.
            let probe_app = handle.clone();
            std::thread::spawn(move || run_probe(opener, probe_app, rx));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("the probe app must build");
    app.run(|_app, _event| {});
}

fn run_probe(opener: tauri::Webview, app: AppHandle, popups: Receiver<String>) {
    if wait_for_eval(&opener, "1+1", Duration::from_secs(20)).is_none() {
        return finish(&app, "the opener page never answered");
    }
    record("opener_ready", true, "the child webview runs the app asset");

    // A sink on the opener, so a postMessage that arrives can be read back.
    step_eval(
        "opener_inbox",
        &opener,
        "window.__inbox = []; window.addEventListener('message', e => window.__inbox.push(String(e.data))); 'ok'",
    );

    // 1. `window.open` from a script. A synthetic eval carries no user activation,
    //    so this may be refused by WebKit's popup policy — the fallback below is the
    //    same delegate reached by a `target=_blank` navigation.
    let opened = step_eval(
        "window_open",
        &opener,
        "(() => { let w = null; try { w = window.open('about:blank', 'probe', 'width=420,height=320'); } catch (err) { return 'throw:' + err; } window.__popup = w; return w ? 'opened' : 'null'; })()",
    );
    let has_handle = opened.as_deref().map(plain) == Some("opened");

    let mut popup_label = popups.recv_timeout(Duration::from_secs(3)).ok();
    record(
        "window_open_reached_delegate",
        popup_label.is_some(),
        format!("handle={has_handle} label={popup_label:?}"),
    );

    if popup_label.is_none() {
        let before = POPUP_REQUESTS.load(Ordering::SeqCst);
        step_eval(
            "target_blank_click",
            &opener,
            "(() => { const a = document.createElement('a'); a.href = 'about:blank'; a.target = '_blank'; a.textContent = 'probe'; document.body.appendChild(a); a.click(); a.remove(); return 'clicked'; })()",
        );
        popup_label = popups.recv_timeout(Duration::from_secs(3)).ok();
        record(
            "target_blank_reached_delegate",
            popup_label.is_some(),
            format!(
                "requests_before={before} requests_after={} label={popup_label:?}",
                POPUP_REQUESTS.load(Ordering::SeqCst)
            ),
        );
    }

    let Some(popup_label) = popup_label else {
        return finish(
            &app,
            "no new-window request reached the delegate, so nothing else could be checked",
        );
    };
    let Some(popup) = app.get_webview(&popup_label) else {
        return finish(&app, "the popup window exists but has no webview");
    };
    if wait_for_eval(&popup, "1+1", Duration::from_secs(15)).is_none() {
        return finish(&app, "the popup webview never answered");
    }
    record("popup_ready", true, format!("label={popup_label}"));

    // 2. The opener relationship: what an OAuth provider page needs to post back.
    let opener_present = step_eval("popup_opener_present", &popup, "String(!!window.opener)");
    record(
        "popup_has_opener",
        opener_present.as_deref().map(plain) == Some("true"),
        format!("window.opener -> {opener_present:?}"),
    );

    // 3. popup → opener
    step_eval(
        "popup_post_to_opener",
        &popup,
        "window.opener ? (window.opener.postMessage('from-popup', '*'), 'sent') : 'no-opener'",
    );
    std::thread::sleep(Duration::from_millis(400));
    let inbox = eval(&opener, "JSON.stringify(window.__inbox)").unwrap_or_default();
    record(
        "postmessage_popup_to_opener",
        inbox.contains("from-popup"),
        format!("opener inbox={inbox}"),
    );

    // 4. opener → popup (only the `window.open` handle allows this direction)
    if has_handle {
        step_eval(
            "popup_inbox",
            &popup,
            "window.__inbox = []; window.addEventListener('message', e => window.__inbox.push(String(e.data))); 'ok'",
        );
        step_eval(
            "opener_post_to_popup",
            &opener,
            "(() => { const w = window.__popup; if (!w) return 'no-handle'; w.postMessage('from-opener', '*'); return 'sent'; })()",
        );
        std::thread::sleep(Duration::from_millis(400));
        let inbox = eval(&popup, "JSON.stringify(window.__inbox)").unwrap_or_default();
        record(
            "postmessage_opener_to_popup",
            inbox.contains("from-opener"),
            format!("popup inbox={inbox}"),
        );
    }

    // 5. `window.close()` from inside a popup that is its OWN top-level window.
    step_eval("popup_window_close", &popup, "window.close(); 'called'");
    std::thread::sleep(Duration::from_millis(800));
    let alive = eval(&popup, "2+2");
    record(
        "popup_still_evaluates_after_js_close",
        matches!(&alive, Ok(answer) if answer.contains('4')),
        format!(
            "eval={alive:?} webview_registered={} window_open={}",
            app.get_webview(&popup_label).is_some(),
            app.get_window(&popup_label).is_some()
        ),
    );

    let proxy = eval(&opener, "String(window.__popup ? window.__popup.closed : 'no-handle')")
        .unwrap_or_default();
    record(
        "opener_sees_popup_closed_after_js_close",
        plain(&proxy) == "true",
        format!("window.__popup.closed -> {proxy}"),
    );

    // 6. The drawer shape: move that webview into the main window, at a drawer rect.
    let Some(main) = app.get_window(MAIN) else {
        return finish(&app, "no main window");
    };
    let reparent = app
        .get_webview(&popup_label)
        .map(|webview| webview.reparent(&main));
    record(
        "reparent_into_main",
        reparent.as_ref().map(std::result::Result::is_ok) == Some(true),
        format!("{reparent:?}"),
    );
    if let Some(webview) = app.get_webview(&popup_label) {
        let bounds = webview.set_bounds(Rect {
            position: LogicalPosition::new(600.0, 120.0).into(),
            size: LogicalSize::new(360.0, 520.0).into(),
        });
        record("popup_drawer_bounds", bounds.is_ok(), format!("{bounds:?}"));
        let shown = webview.show();
        record("popup_drawer_visible", shown.is_ok(), format!("{shown:?}"));
    }
    std::thread::sleep(Duration::from_millis(400));

    // 7. Does the relationship survive the move into the drawer?
    let Some(popup) = app.get_webview(&popup_label) else {
        return finish(&app, "the reparented popup webview is gone");
    };
    let opener_present = step_eval(
        "popup_opener_after_reparent",
        &popup,
        "String(!!window.opener)",
    );
    record(
        "popup_has_opener_in_drawer",
        opener_present.as_deref().map(plain) == Some("true"),
        format!("window.opener -> {opener_present:?}"),
    );
    step_eval(
        "popup_inbox_after_reparent",
        &popup,
        "window.__inbox = []; window.addEventListener('message', e => window.__inbox.push(String(e.data))); 'ok'",
    );
    step_eval(
        "popup_post_to_opener_after_reparent",
        &popup,
        "window.opener ? (window.opener.postMessage('from-drawer-popup', '*'), 'sent') : 'no-opener'",
    );
    std::thread::sleep(Duration::from_millis(400));
    let inbox = eval(&opener, "JSON.stringify(window.__inbox)").unwrap_or_default();
    record(
        "postmessage_popup_to_opener_in_drawer",
        inbox.contains("from-drawer-popup"),
        format!("opener inbox={inbox}"),
    );

    // 8. `window.close()` again, now that the popup is a child webview: there is no
    //    window of its own left to close.
    step_eval(
        "drawer_popup_window_close",
        &popup,
        "window.close(); 'called'",
    );
    std::thread::sleep(Duration::from_millis(800));
    let alive = eval(&popup, "3+3");
    record(
        "drawer_popup_still_evaluates_after_js_close",
        matches!(&alive, Ok(answer) if answer.contains('6')),
        format!("eval={alive:?}"),
    );

    // 9. The empty host window of the reparented webview: does closing it take the
    //    drawer's webview with it?
    if let Some(host) = app.get_window(&popup_label) {
        let closed = host.close();
        record(
            "host_window_close",
            closed.is_ok(),
            format!("closed_the_empty_popup_window={closed:?}"),
        );
        std::thread::sleep(Duration::from_millis(600));
        let alive = eval(&popup, "4+4");
        record(
            "drawer_popup_alive_after_host_window_close",
            matches!(&alive, Ok(answer) if answer.contains('8')),
            format!("eval={alive:?} webview_registered={}", app.get_webview(&popup_label).is_some()),
        );
    }

    // 9b. `target=_blank` — the other way a page asks for a new tab, in the same
    //     drawer shape (a `Create` popup we can then close by window).
    let before = POPUP_REQUESTS.load(Ordering::SeqCst);
    step_eval(
        "target_blank_click_in_drawer_phase",
        &opener,
        "(() => { const a = document.createElement('a'); a.href = 'about:blank'; a.target = '_blank'; a.textContent = 'probe'; document.body.appendChild(a); a.click(); a.remove(); return 'clicked'; })()",
    );
    match popups.recv_timeout(Duration::from_secs(3)) {
        Ok(label) => {
            record(
                "target_blank_reached_delegate",
                true,
                format!("label={label} requests_after={}", POPUP_REQUESTS.load(Ordering::SeqCst)),
            );
            if let Some(window) = app.get_window(&label) {
                let _ = window.close();
            }
        }
        Err(_) => record(
            "target_blank_reached_delegate",
            false,
            format!(
                "before={before} after={} (no request reached the handler)",
                POPUP_REQUESTS.load(Ordering::SeqCst)
            ),
        ),
    }
    std::thread::sleep(Duration::from_millis(300));

    // 10. The comparison shape: `Allow`, i.e. wry's own native popup window. Its
    //     document closes ITSELF, which is what an OAuth provider page does at the
    //     end of the flow, and that is the one signal the drawer shape cannot hear.
    ALLOW_NATIVE_WINDOW.store(true, Ordering::SeqCst);
    let before = POPUP_REQUESTS.load(Ordering::SeqCst);
    // `data:` carries its own inline script, so no custom-scheme handler is involved.
    // `close.html` is the app-asset variant, kept to reproduce what that path does
    // when the wry-created window asks for a custom-scheme url (`ABORTS`, see
    // `docs/ai/browser-popups.md`).
    let asset = std::env::var_os("NABLA_PROBE_ALLOW_ASSET_URL").is_some();
    let url = if asset {
        "close.html".to_string()
    } else {
        "data:text/html,%3Cscript%3EsetTimeout(function()%7Bwindow.close()%7D,300)%3C/script%3E"
            .to_string()
    };
    record("allow_popup_url", true, format!("asset={asset} url={url}"));
    step_eval(
        "allow_window_open",
        &opener,
        &format!(
            "(() => {{ window.__popup2 = window.open('{url}', 'probe2', 'width=400,height=300'); return window.__popup2 ? 'opened' : 'null'; }})()"
        ),
    );
    std::thread::sleep(Duration::from_millis(1500));
    record(
        "allow_delegate_reached",
        POPUP_REQUESTS.load(Ordering::SeqCst) > before,
        format!("before={before} after={}", POPUP_REQUESTS.load(Ordering::SeqCst)),
    );
    let proxy = eval(
        &opener,
        "String(window.__popup2 ? window.__popup2.closed : 'no-handle')",
    )
    .unwrap_or_default();
    record(
        "allow_popup_closed_itself",
        plain(&proxy) == "true",
        format!("window.__popup2.closed -> {proxy}"),
    );

    finish(&app, "probe complete");
}
