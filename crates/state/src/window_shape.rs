//! The Windows-only hook that lets [`crate::AppSettings::rounded_window`] reach the real
//! window. The actual `DwmSetWindowAttribute` call is platform code that only `sonora` links
//! against, so it installs the closure here once at startup; `views` calls [`apply_rounded_window`]
//! from `Root`'s render whenever the setting changes, keeping it live without either crate
//! depending on the other's platform bindings.

use gpui::{App, Global, Window};

type Apply = Box<dyn Fn(&Window, bool)>;

struct RoundedWindowHook(Apply);

impl Global for RoundedWindowHook {}

/// Registers the platform-specific corner-rounding hook. A no-op until this is called, so
/// non-Windows targets simply never install one.
pub fn install_rounded_window_hook(apply: impl Fn(&Window, bool) + 'static, cx: &mut App) {
    cx.set_global(RoundedWindowHook(Box::new(apply)));
}

/// Applies the rounded-window preference to the live window, if a hook is installed.
pub fn apply_rounded_window(window: &Window, rounded: bool, cx: &App) {
    if let Some(hook) = cx.try_global::<RoundedWindowHook>() {
        (hook.0)(window, rounded);
    }
}
