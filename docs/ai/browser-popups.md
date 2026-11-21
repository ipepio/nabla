# Browser popups: tabs inside the Browser pane on macOS

Status: **implemented** — `src-tauri/src/embed.rs` (native half),
`src/components/tools/BrowserTool.vue` + `browser-tabs.ts` (pane half). This records what the
embedded browser can and cannot do with a page that opens a popup, what was built on top of it, and
the one thing the platform does not give us.

Short version: the popup is reachable, becomes a tab **inside the Browser pane** with real
`window.opener` and real `postMessage` in both directions, and never becomes an agent target. What
cannot be reached is **the popup closing itself**: on macOS, WebKit's `window.close()` never tells the
app. L3 decided to ship anyway, with the tab's close button as the end of a popup's life; the missing
native signal is stated where the human meets it, and there is no crash path because
`NewWindowResponse::Allow` is never returned.

## The requirement

S3 of `drawer-responsiveness-oauth.md`:

> «permitir al webview arbrir pestañas nuevas (para autenticaciones oauth que van en pestaña
> separada) dentro del mismo drawer que puedes tener varias pestañas»

A popup tab is only a popup tab if three things are true — an OAuth provider page depends on all of
them:

1. the navigation is **intercepted** (`window.open` and `target=_blank` do not just do nothing),
2. `window.opener` exists inside the popup, so the provider can `postMessage` the result back,
3. `window.close()` from inside the popup ends it, so the tab goes away when the flow finishes.

A fourth follows from S3's last clause ("protect human auth and agent target"): the popup must not
silently become a new **agent** target.

L3 («claro, lo que quiero es abrir una nueva pestaña en el browser, no en el drawer») settled where the
tab lives — inside the Browser pane, next to the page that opened it, not in the drawer's tab strip —
and, by accepting that the human closes it, settled point 3 as well: the provider's `window.close()`
is not heard on macOS, so a close button on the tab is what ends it. Point 3 is therefore met in the
sense S3 needs — a popup can always be ended, and ending it returns to the page that started the flow
— and not in the sense macOS does not offer.

## What the pinned Tauri exposes

Versions are the ones `src-tauri/Cargo.lock` pins: tauri `2.12.5`, tauri-runtime-wry `2.12.3`,
tauri-runtime `2.12.2`, wry `0.57.2`.

- **`window.open` and `target=_blank` are interceptable from a child webview.**
  `tauri::webview::WebviewBuilder::on_new_window` (`tauri-2.12.5/src/webview/mod.rs:592`) exists on
  the same builder `Window::add_child` takes, so an `embed-<n>` view can answer them. macOS is
  supported; Android/iOS are not.
- **The handler cannot hand back a child webview.** The response is
  `NewWindowResponse::Create { window: crate::WebviewWindow<R> }`
  (`tauri-2.12.5/src/webview/mod.rs:243-256`): a *top-level window* with its own webview. The runtime
  resolves it to the first webview of that window (`tauri-runtime-wry-2.12.3/src/lib.rs:4852-4866`),
  and wry returns that `WKWebView` to WebKit
  (`wry-0.57.2/src/wkwebview/class/wry_web_view_ui_delegate.rs:295`). So a popup inside the drawer
  needs two steps, not one: build a window, then **`Webview::reparent`** it into the main window
  (`tauri-2.12.5/src/webview/mod.rs:1690-1741`, allowed for any webview with the `unstable` feature,
  which this crate already enables).
- **The opener relationship comes from the webview configuration, and Tauri forwards it.**
  WebKit requires the returned webview to be built with the *same* `WKWebViewConfiguration` object;
  wry uses the given configuration as-is (`wry-0.57.2/src/wkwebview/mod.rs:253-259`), and
  `WebviewWindowBuilder::window_features` applies the opener's own configuration
  (`tauri-2.12.5/src/webview/webview_window.rs:1481`).
- **macOS never hears `window.close()`.** Wry's macOS UI delegate implements
  `webView:createWebViewWithConfiguration:…` but **not** `webViewDidClose:`
  (`grep -rn "didClose" wry-0.57.2/src` finds nothing), so a page that closes itself closes nothing
  and raises no event. The other platforms do handle it: Windows closes the container
  HWND on a JS close (`wry-0.57.2/src/webview2/mod.rs:695`) and webkit2gtk closes its window
  (`wry-0.57.2/src/webkitgtk/mod.rs:473`). **The gap is macOS-only, and it is the drawer's platform.**
- **`NewWindowResponse::Allow` is not a safe fallback here.** It makes wry build its own `NSWindow`
  and a plain `WKWebView` (`wry_web_view_ui_delegate.rs:222-295`), but wry's custom-protocol handler
  later casts that webview to its own subclass and panics on the missing instance variables
  (`wry-0.57.2/src/wkwebview/class/wry_web_view.rs:129`, `tried to access uninitialized instance
  variable`). Any popup that touches a `tauri://` url aborts the whole process. Reproduced below.

## The probe

`tests/browser-popup-probe/` is a standalone Tauri app (own `Cargo.toml`, own `tauri.conf.json`,
`[workspace]` so it is never part of the app build) that reproduces the drawer exactly: one window,
one child webview added with `Window::add_child`, `on_new_window` installed on it. Every step is
driven by `eval` from `src/main.rs`; the pages are a local app asset
(`tests/browser-popup-probe/assets/index.html`), a `data:` url and `about:blank`. **No network, no external account, no credentials, no third-party
page, no injection into a remote document.** The window is on screen for a few seconds and the app
exits by itself.

```sh
CARGO_TARGET_DIR=$(mktemp -d) \
  cargo run --manifest-path tests/browser-popup-probe/Cargo.toml
# one JSON object per line, also appended to $TMPDIR/nabla-popup-probe.jsonl
```

## Observed result — macOS 27.2, arm64

The final run's records, in order. A few setup and bookkeeping lines (`env`, `opener_ready`,
`popup_ready`, `done`, …) are dropped; every remaining line is verbatim:

```text
{"step":"on_new_window","ok":true,"detail":"request=1 url=about:blank size=Some(LogicalSize { width: 420.0, height: 320.0 }) position=None"}
{"step":"popup_build","ok":true,"detail":"label=popup-1 elapsed_ms=6"}
{"step":"window_open","ok":true,"detail":"answer=\"opened\""}
{"step":"window_open_reached_delegate","ok":true,"detail":"handle=true label=Some(\"popup-1\")"}
{"step":"popup_opener_present","ok":true,"detail":"answer=\"true\""}
{"step":"popup_has_opener","ok":true,"detail":"window.opener -> Some(\"\\\"true\\\"\")"}
{"step":"popup_post_to_opener","ok":true,"detail":"answer=\"sent\""}
{"step":"postmessage_popup_to_opener","ok":true,"detail":"opener inbox=\"[\\\"from-popup\\\"]\""}
{"step":"opener_post_to_popup","ok":true,"detail":"answer=\"sent\""}
{"step":"postmessage_opener_to_popup","ok":true,"detail":"popup inbox=\"[\\\"from-opener\\\"]\""}
{"step":"popup_window_close","ok":true,"detail":"answer=\"called\""}
{"step":"popup_still_evaluates_after_js_close","ok":true,"detail":"eval=Ok(\"4\") webview_registered=true window_open=true"}
{"step":"opener_sees_popup_closed_after_js_close","ok":true,"detail":"window.__popup.closed -> \"true\""}
{"step":"reparent_into_main","ok":true,"detail":"Some(Ok(()))"}
{"step":"popup_drawer_bounds","ok":true,"detail":"Ok(())"}
{"step":"popup_drawer_visible","ok":true,"detail":"Ok(())"}
{"step":"popup_opener_after_reparent","ok":true,"detail":"answer=\"true\""}
{"step":"popup_has_opener_in_drawer","ok":true,"detail":"window.opener -> Some(\"\\\"true\\\"\")"}
{"step":"popup_post_to_opener_after_reparent","ok":true,"detail":"answer=\"sent\""}
{"step":"postmessage_popup_to_opener_in_drawer","ok":true,"detail":"opener inbox=\"[\\\"from-popup\\\",\\\"from-drawer-popup\\\"]\""}
{"step":"drawer_popup_window_close","ok":true,"detail":"answer=\"called\""}
{"step":"drawer_popup_still_evaluates_after_js_close","ok":true,"detail":"eval=Ok(\"6\")"}
{"step":"host_window_close","ok":true,"detail":"closed_the_empty_popup_window=Ok(())"}
{"step":"popup_window_event","ok":true,"detail":"CloseRequested { api: CloseRequestApi(Sender { .. }) }"}
{"step":"popup_window_event","ok":true,"detail":"Destroyed"}
{"step":"drawer_popup_alive_after_host_window_close","ok":true,"detail":"eval=Ok(\"8\") webview_registered=true"}
{"step":"target_blank_click_in_drawer_phase","ok":true,"detail":"answer=\"clicked\""}
{"step":"on_new_window","ok":true,"detail":"request=2 url=about:blank size=None position=None"}
{"step":"popup_build","ok":true,"detail":"label=popup-2 elapsed_ms=9"}
{"step":"target_blank_reached_delegate","ok":true,"detail":"label=popup-2 requests_after=2"}
{"step":"popup_window_event","ok":true,"detail":"CloseRequested { api: CloseRequestApi(Sender { .. }) }"}
{"step":"popup_window_event","ok":true,"detail":"Destroyed"}
{"step":"on_new_window","ok":true,"detail":"request=3 url=data:text/html,%3Cscript%3EsetTimeout(function()%7Bwindow.close()%7D,300)%3C/script%3E size=Some(LogicalSize { width: 400.0, height: 300.0 }) position=None"}
{"step":"allow_window_open","ok":true,"detail":"answer=\"opened\""}
{"step":"allow_delegate_reached","ok":true,"detail":"before=2 after=3"}
{"step":"allow_popup_closed_itself","ok":false,"detail":"window.__popup2.closed -> \"false\""}
```

Read it as a table:

| # | Question | Answer |
| --- | --- | --- |
| 1 | Is `window.open` from a child webview interceptable? | **Yes.** `on_new_window` fires with the url and the requested size; the handler built a window in 9 ms, on the main thread, inside WebKit's delegate (the documented shape does not deadlock). |
| 2 | Is `target=_blank` interceptable? | **Yes.** A synthetic anchor click arrived as `url=about:blank, size=None` — the same handler, no size features. |
| 3 | Does the popup get `window.opener`? | **Yes** (`"true"`), because `window_features` reuses the opener's `WKWebViewConfiguration`. |
| 4 | Does `postMessage` work both ways? | **Yes.** popup → opener and opener → popup both delivered. |
| 5 | Can the popup live in the drawer? | **Yes, in two steps.** `reparent` into the main window answered `Ok(())`, `set_bounds`/`show` placed it at the drawer rect, and `window.opener` **and** `postMessage` still worked afterwards. |
| 6 | Can the empty host window be closed afterwards? | **Yes.** The popup window raised `CloseRequested` then `Destroyed`, and the reparented webview kept running (`4+4 → 8`). No leak, no platform hack. |
| 7 | Does `window.close()` from the popup end it? | **No.** After `window.close()` the webview still evaluates (`2+2 → 4`, `3+3 → 6`), the Tauri window stays open, and no window event fires. |
| 8 | Is the close at least visible somewhere? | **Only to the opener page.** The opener's proxy reports `window.__popup.closed === true`. There is no native signal. |
| 9 | Is `Allow` (wry's own window) a fallback? | **No.** It aborts the process as soon as the popup loads a `tauri://` url: `panicked at wry-0.57.2/src/wkwebview/class/wry_web_view.rs:129: tried to access uninitialized instance variable`, exit `134` (SIGABRT). Reproduce with `NABLA_PROBE_ALLOW_ASSET_URL=1`. With a `data:` popup there is no abort, but that page's own `window.close()` did not flip the opener's proxy either (`window.__popup2.closed -> "false"`), and wry exposes no handle to its window, so nothing better can be claimed for `Allow`. |

Two details worth keeping:

- **No user activation was needed.** The probe's `window.open` came from an `eval`, not a click, and
  macOS still routed it to the delegate. Nothing here depends on a gesture having happened.
- **The popup window never has to be seen.** The probe created it with `visible(false)`, moved it into
  the drawer and showed it there.

## What that means for S3

| S3 half | Verdict |
| --- | --- |
| opener callback (`window.opener` + `postMessage`) | **proven and shipped** on macOS, including after the move into the pane |
| close | **manual only.** `window.close()` reaches no one on macOS, so the tab's close button is the end of a popup's life |

A tab that cannot hear `window.close()` is a tab the human ends. That limitation is stated where the
human meets it: every popup tab carries a close button, closing it returns to the page that opened the
flow, and the page keeps its own login (the popup's session dies with its webview). What the app does
**not** claim is that a finished OAuth popup disappears on its own — it does not, and that is why this
paragraph exists instead of a promise.

## How it is implemented

`src-tauri/src/embed.rs` — the native half:

- **Interception.** `WebviewBuilder::on_new_window` is installed on every page view `create` builds.
  The handler answers `NewWindowResponse::Deny` outside the popup policy, or `Create { window }` with a
  `WebviewWindow` built from `window_features` (the opener's own `WKWebViewConfiguration` on macOS,
  which is what gives the popup its `window.opener`) and `visible(false)`. It never answers `Allow`:
  wry's own window panics on the first `tauri://` navigation and gives the app no handle.
- **The move.** `Create` cannot hand back a child webview, so a thread of its own finishes the job:
  `hide` (a page nobody has placed yet must not paint over the app), `reparent` into the main window,
  a one-pixel placeholder rect, then `close` on the emptied host window. `reparent` waits for the
  event loop, which is exactly why it never runs inside the delegate that called the handler.
- **The link.** A popup is a registered view like any other — same `embed-<n>` label, same cap
  (`MAX_VIEWS`, popups included) — plus one field: the id of the view that opened it. That link is
  what the pane turns into a strip tab, what a close follows (closing a page closes its popups,
  transitively), and what keeps the popup out of `ready_views()` and `is_ready()` — the two answers
  the MCP bridge gives the agent, so a popup can never be navigated or evaluated by a tool.
- **Two signals, one list.** `embed-popup` (`{ viewId, openerId }`) tells the pane a tab is waiting;
  `embed_popups` is what it reconciles on mount, for a popup that arrived while no listener was up.
  Neither payload carries a url.
- **No logging, no crash path.** Every line this path writes names ids and reasons only: a popup
  request is where a provider's redirect lands, and a redirect can carry an authorization code. The
  popup policy is `http`, `https` and `about:blank`, so a `tauri://`, `file:`, `javascript:` or
  `data:` request never reaches a webview at all.
- **Orphans are reaped.** A popup whose opener left the registry (a page evicted by the MCP driver, a
  close racing a build) has no tab to return to, so the next `create` closes it rather than leaving a
  hidden webview holding a session.
- **Popups do not nest.** The popup window carries no `on_new_window` of its own, so a provider page
  that calls `window.open` once more gets nothing (`window.open` returns `null`, which those pages
  already handle). One level is what an OAuth flow needs, and one level is what the pane's strip
  draws.

`src/components/tools/BrowserTool.vue` + `browser-tabs.ts` — the pane half:

- One strip, one pane: the page tab (the view the pane created, the one the agent drives, the only tab
  with no close button) plus one tab per popup, inserted right after its opener.
- Only the drawn tab's view is placed and shown; the others keep their document and are hidden with
  the platform's own switch. Switching tabs re-measures the body rect before showing, exactly as the
  drawer reopening does.
- The drawer collapsing hides every view; the pane's own tab no longer being drawn closes them — a tab
  switch is the human saying they are looking at something else, and that was already true of the
  single page view.
- The tab model is pure and covered by `tests/browser-tabs.test.ts`: one tab per view, one drawn tab, a
  close that returns to the opener, and no path that closes the page tab at all.

## Rejected alternatives

- **Open the url in an existing `embed-<n>` view** ("the url clone"). No opener, no `postMessage`, and
  the original page is navigated away — the provider's callback would land in a document that lost
  the context that started the flow. Also explicitly ruled out by the task.
- **`NewWindowResponse::Allow`** (wry's own native window). It keeps a real popup window, but it
  aborts the process on any `tauri://` navigation (row 9) and gives the app no handle to the window,
  so the drawer could not manage it. Unsafe, not merely awkward.
- **Polling the popup or the opener from the app.** Polling inside the popup cannot work (a closed
  browsing context has no page to evaluate in), and polling the opener means injecting a harness into
  whatever third-party page started the flow — precisely the remote-page injection this task forbids,
  and it would break on the next provider that scopes its `window` differently.
- **A platform hack** (an `objc2` delegate of our own on the popup's `WKWebView`, or attaching an
  `NSView` we build ourselves). Out of bounds: it is exactly the "invasive platform hack" the task
  rules out, and it would bypass every guarantee `reparent` currently keeps.

## What changed in L3

Two of the three "safe alternatives" this document listed before T3 stopped are now moot, and the
record of why is worth keeping:

1. **Hand the flow to the system browser** (loopback redirect) — still the shape most desktop apps
   use, still unchanged here, and still not S3: it leaves the window.
2. **Popup tabs as a separate native window** — not chosen. Everything in the table above holds for
   it, but L3 asked for the tab *inside the Browser pane*, and a separate window would be a second
   surface the pane cannot place, hide or resize.
3. **Wait for the upstream fix** (`webViewDidClose:` in wry plus a Tauri event) — **not waited for.**
   L3 accepted a manual close, which makes the tab endable today. When the upstream signal lands, the
   close button stays and the provider's own `window.close()` can join it behind the same
   `embed_close` path; nothing else in this design has to change.

## If it is implemented later

Nothing below is outstanding: the list this section used to carry was what a future integration had to
settle, and each item is answered under
[How it is implemented](#how-it-is-implemented) (agent exclusion via `ready_views`/`is_ready`, the
honest two-step creation plus the label sweeps in `close_all`, pixels as a manual `tauri dev` check,
and the human's close as the documented end of a popup). It is kept as a checklist for the next change
here rather than deleted, because each line is a decision somebody made.

## Not covered

- **Pixels.** The probe proves JavaScript, bounds and visibility, not that the reparented webview
  repaints over the pane's body rect. That check is visual and belongs to `npm run tauri dev`.
- **Linux and Windows were not run**; the platform notes above are source-level (`wry-0.57.2`). Linux
  additionally wants `with_related_view` for the webview WebKit hands back, which this integration
  does not set: a popup is expected to be refused there rather than misbehave, and that expectation is
  untested.
- **iOS/Android**: `Window::add_child` and `on_new_window` do not exist there.
- **Anything requiring a real OAuth provider or credentials**: out of scope by instruction.
