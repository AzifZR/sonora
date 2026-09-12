//! The Windows-only hook that lets [`crate::AppSettings::window_rounding`] reach the real
//! window. The actual `DwmSetWindowAttribute` call is platform code that only `sonora` links
//! against, so it installs the closure here once at startup; `views` calls [`apply_window_rounding`]
//! from `Root`'s render whenever the setting changes, keeping it live without either crate
//! depending on the other's platform bindings. Linux/FreeBSD round their own chrome directly in
//! `views` instead, since that's ordinary element styling rather than a platform call.

use gpui::{App, Global, Window};
use ui::Rounding;

type Apply = Box<dyn Fn(&Window, Rounding)>;

struct RoundedWindowHook(Apply);

impl Global for RoundedWindowHook {}

/// Registers the platform-specific corner-rounding hook. A no-op until this is called, so
/// non-Windows targets simply never install one.
pub fn install_rounded_window_hook(apply: impl Fn(&Window, Rounding) + 'static, cx: &mut App) {
    cx.set_global(RoundedWindowHook(Box::new(apply)));
}

/// Applies the window-rounding preference to the live window, if a hook is installed.
pub fn apply_window_rounding(window: &Window, rounding: Rounding, cx: &App) {
    if let Some(hook) = cx.try_global::<RoundedWindowHook>() {
        (hook.0)(window, rounding);
    }
}
