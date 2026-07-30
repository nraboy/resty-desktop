//! Works around a known WebKitGTK/NVIDIA/Wayland crash — `Gdk-Message: Error 71 (Protocol
//! error) dispatching to Wayland display.` — by setting the same env vars Tauri's own docs
//! recommend (https://v2.tauri.app/develop/debug/linux-graphics/), but only on machines that
//! are actually affected: NVIDIA driver present *and* a Wayland session. See CLAUDE.md's
//! "Linux GPU Compatibility" section for the full rationale, including why the detection is
//! gated rather than unconditional and why `WEBKIT_DISABLE_COMPOSITING_MODE` is excluded.
//!
//! Must run before `tauri::Builder::default()` — see `lib.rs::run()`.

#[cfg(target_os = "linux")]
use std::env;

/// Env var a user can set to skip the workaround entirely (e.g. to test whether a driver
/// update has fixed things upstream, or to rule the workaround out as a cause of unrelated
/// rendering issues).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const OPT_OUT_VAR: &str = "RESTY_DISABLE_GPU_WORKAROUND";

// Linux is the only consumer of the items below — `apply()`'s real body is cfg'd out
// everywhere else, so on macOS/Windows they're referenced solely by `#[cfg(test)]` code and
// rustc's `dead_code` lint fires in the plain lib build (`npm run lint:rust` runs
// `cargo clippy --all-targets -D warnings`, which builds both the test target, where these
// are used, and the plain lib target, where they aren't). Kept cross-platform on purpose so
// the pure logic below stays unit-testable on any dev machine, matching the existing
// targeted-`#[allow]`-with-a-comment convention (see `cache.rs`).

/// Variables applied, cheapest-first. `__NV_DISABLE_EXPLICIT_SYNC` often fixes Error 71 with
/// no performance cost; `WEBKIT_DISABLE_DMABUF_RENDERER` is the stronger, verified fix (loses
/// the faster DMA-BUF rendering path). `WEBKIT_DISABLE_COMPOSITING_MODE` is deliberately not
/// included — it's the most expensive of Tauri's three documented options and nothing here
/// points to the symptom (crash-on-resize) it targets.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const WORKAROUND_VARS: &[(&str, &str)] = &[
    ("__NV_DISABLE_EXPLICIT_SYNC", "1"),
    ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
];

/// Pure decision: no I/O, no cfg gating — every branch is unit-tested.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn should_apply(nvidia_present: bool, wayland_session: bool, opted_out: bool) -> bool {
    !opted_out && nvidia_present && wayland_session
}

/// True when `OPT_OUT_VAR` is set to anything other than empty or `"0"`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn is_opted_out(opt_out_var: Option<&str>) -> bool {
    matches!(opt_out_var, Some(v) if !v.is_empty() && v != "0")
}

/// True when either `WAYLAND_DISPLAY` is set to a non-empty value or `XDG_SESSION_TYPE` is
/// `"wayland"` — covers sessions that only set one of the two. A set-but-empty
/// `WAYLAND_DISPLAY` doesn't count, matching `is_opted_out`'s empty-string handling.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn is_wayland(wayland_display: Option<&str>, xdg_session_type: Option<&str>) -> bool {
    matches!(wayland_display, Some(v) if !v.is_empty()) || xdg_session_type == Some("wayland")
}

/// Applies the workaround on Linux when NVIDIA + Wayland are both detected and the user
/// hasn't opted out. Never overwrites a variable the user already set themselves.
#[cfg(target_os = "linux")]
pub(crate) fn apply() {
    use std::path::Path;

    let nvidia_present = Path::new("/sys/module/nvidia").exists();
    let wayland_display = env::var("WAYLAND_DISPLAY").ok();
    let xdg_session_type = env::var("XDG_SESSION_TYPE").ok();
    let wayland_session = is_wayland(wayland_display.as_deref(), xdg_session_type.as_deref());
    let opt_out = env::var(OPT_OUT_VAR).ok();
    let opted_out = is_opted_out(opt_out.as_deref());

    if !should_apply(nvidia_present, wayland_session, opted_out) {
        return;
    }

    let mut applied = Vec::new();
    for (name, value) in WORKAROUND_VARS {
        if env::var_os(name).is_none() {
            // SAFETY: this runs as the first statement of `run()`, before the Tauri builder
            // spawns any thread or webview, so there is no concurrent reader to race with.
            unsafe {
                env::set_var(name, value);
            }
            applied.push(*name);
        }
    }

    if !applied.is_empty() {
        eprintln!(
            "[resty] Detected NVIDIA + Wayland; applied GPU compatibility workaround(s): {}. \
             Set {}=1 to disable this and use the default rendering path.",
            applied.join(", "),
            OPT_OUT_VAR
        );
    }
}

/// No-op everywhere except Linux — keeps the call site in `lib.rs::run()` cfg-free.
#[cfg(not(target_os = "linux"))]
pub(crate) fn apply() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_when_nvidia_and_wayland_and_not_opted_out() {
        assert!(should_apply(true, true, false));
    }

    #[test]
    fn does_not_apply_without_nvidia() {
        assert!(!should_apply(false, true, false));
    }

    #[test]
    fn does_not_apply_without_wayland() {
        assert!(!should_apply(true, false, false));
    }

    #[test]
    fn does_not_apply_when_opted_out_even_if_matched() {
        assert!(!should_apply(true, true, true));
    }

    #[test]
    fn does_not_apply_when_neither_condition_matches() {
        assert!(!should_apply(false, false, false));
    }

    #[test]
    fn opt_out_var_unset_is_not_opted_out() {
        assert!(!is_opted_out(None));
    }

    #[test]
    fn opt_out_var_empty_is_not_opted_out() {
        assert!(!is_opted_out(Some("")));
    }

    #[test]
    fn opt_out_var_zero_is_not_opted_out() {
        assert!(!is_opted_out(Some("0")));
    }

    #[test]
    fn opt_out_var_one_is_opted_out() {
        assert!(is_opted_out(Some("1")));
    }

    #[test]
    fn opt_out_var_any_nonzero_value_is_opted_out() {
        assert!(is_opted_out(Some("true")));
        assert!(is_opted_out(Some("yes")));
    }

    #[test]
    fn wayland_display_set_is_wayland() {
        assert!(is_wayland(Some("wayland-0"), None));
    }

    #[test]
    fn xdg_session_type_wayland_is_wayland() {
        assert!(is_wayland(None, Some("wayland")));
    }

    #[test]
    fn xdg_session_type_x11_is_not_wayland() {
        assert!(!is_wayland(None, Some("x11")));
    }

    #[test]
    fn neither_var_set_is_not_wayland() {
        assert!(!is_wayland(None, None));
    }

    #[test]
    fn wayland_display_empty_is_not_wayland() {
        assert!(!is_wayland(Some(""), None));
    }

    #[test]
    fn wayland_display_empty_but_xdg_wayland_is_wayland() {
        assert!(is_wayland(Some(""), Some("wayland")));
    }

    #[test]
    fn workaround_vars_are_cheapest_first_and_excludes_compositing_mode() {
        let names: Vec<&str> = WORKAROUND_VARS.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, vec!["__NV_DISABLE_EXPLICIT_SYNC", "WEBKIT_DISABLE_DMABUF_RENDERER"]);
        assert!(!names.contains(&"WEBKIT_DISABLE_COMPOSITING_MODE"));
    }

    #[test]
    fn workaround_vars_all_set_to_one() {
        for (_, value) in WORKAROUND_VARS {
            assert_eq!(*value, "1");
        }
    }
}
