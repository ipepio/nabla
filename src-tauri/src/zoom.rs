//! Webview zoom: the range the app accepts, and the one command that applies it.
//!
//! The UI's own zoom (maintainer request: «añadir en los atajos el cmd + y el cmd -
//! para el zoom») is the zoom of the MAIN webview — the app shell. Tauri exposes it
//! as `Webview::set_zoom` (tauri 2.12.5), which is `wry`'s page zoom:
//!
//! - **macOS** `WKWebView.setPageZoom` (macOS 11+),
//! - **Windows** WebView2 `SetZoomFactor`,
//! - **Linux** webkit2gtk `WebView::set_zoom_level`.
//!
//! All three exist in this build, and all three take a FACTOR where `1.0` is 100%.
//! The zoom is a property of ONE webview, so the embedded browser's child webviews
//! (`embed-<n>`, see `embed.rs`) are untouched: the page the agent drives keeps its
//! own scale while the shell around it grows. That is the behaviour we want — the
//! child is somebody else's page, not part of our layout — and it is also why the
//! rect the UI sends for a child stays valid under native zoom: page zoom does not
//! change CSS pixels, so `getBoundingClientRect()` still matches the logical
//! position Rust places the child at.
//!
//! Only the range is Rust's business. The chord, the step and the list of levels
//! are policy in `src/lib/zoom.ts`, tested there; this module refuses a factor that
//! is not a real number inside the same ends, so a hand-written `invoke` cannot put
//! the window at 10000% where the human cannot undo it with the keyboard.

/// Smallest factor accepted; mirrors `MIN_ZOOM` in `src/lib/zoom.ts`.
pub const MIN_SCALE: f64 = 0.5;

/// Largest factor accepted; mirrors `MAX_ZOOM` in `src/lib/zoom.ts`.
pub const MAX_SCALE: f64 = 2.0;

/// The factor to apply, or the reason it is refused. `NaN` and the infinities are
/// refused by the range check itself (`NaN` comparisons are always false), but they
/// are named in the message because they are what a wrong payload looks like.
pub fn validate_scale(scale: f64) -> Result<f64, String> {
    if !scale.is_finite() {
        return Err(format!("zoom: {scale} is not a finite scale factor"));
    }
    if scale < MIN_SCALE || scale > MAX_SCALE {
        return Err(format!("zoom: {scale} is outside {MIN_SCALE}–{MAX_SCALE}"));
    }
    Ok(scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_the_ui_can_send_is_accepted() {
        // The end of the list in `src/lib/zoom.ts` and the one in the middle: the
        // range exists so these three never bounce.
        for scale in [MIN_SCALE, 1.0, 1.25, MAX_SCALE] {
            assert_eq!(validate_scale(scale), Ok(scale), "refused {scale}");
        }
    }

    #[test]
    fn a_factor_below_or_above_the_range_is_refused() {
        for scale in [0.0, -1.0, MIN_SCALE - 0.01, MAX_SCALE + 0.01, 10.0] {
            let err = validate_scale(scale).expect_err("out of range");
            assert!(err.contains("outside"), "got {err:?}");
            assert!(err.contains(&scale.to_string()), "got {err:?}");
        }
    }

    #[test]
    fn a_payload_that_is_not_a_number_is_refused() {
        for scale in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = validate_scale(scale).expect_err("not finite");
            assert!(err.contains("not a finite"), "got {err:?}");
        }
    }
}
