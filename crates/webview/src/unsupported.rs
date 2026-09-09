//! The backend for platforms without a sign-in window yet. Windows wants WebView2 and Linux
//! webkit2gtk; both fit the same five calls `macos.rs` implements.

use anyhow::{Result, bail};

use crate::{Cookie, Target};

pub(crate) const SUPPORTED: bool = false;

pub(crate) struct Window;

impl Window {
    pub(crate) fn open(_target: &Target) -> Result<Self> {
        bail!("this platform has no sign-in window yet")
    }

    pub(crate) fn closed(&self) -> bool {
        true
    }

    pub(crate) fn host(&self) -> Option<String> {
        None
    }

    pub(crate) fn fetch(&mut self) -> Option<Vec<Cookie>> {
        None
    }

    pub(crate) fn discard(&mut self) {}

    pub(crate) fn close(&self) {}
}
