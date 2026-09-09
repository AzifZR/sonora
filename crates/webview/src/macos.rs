//! The macOS backend: an `NSWindow` whose content view is a `WKWebView` over a non-persistent data
//! store. AppKit and WebKit are main-thread only, and so is every call here; WebKit runs the cookie
//! completion on the main thread too, which is what lets `Fetch` sit in an `Rc`.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;

use anyhow::{Context as _, Result};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly as _};
use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
use objc2_foundation::{
    NSArray, NSHTTPCookie, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest,
};
use objc2_web_kit::{WKHTTPCookieStore, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore};

use crate::{Cookie, Target};

pub(crate) const SUPPORTED: bool = true;

const WIDTH: f64 = 520.;
const HEIGHT: f64 = 720.;
const MIN_WIDTH: f64 = 400.;
const MIN_HEIGHT: f64 = 500.;
/// Safari's own user agent. WebKit's default leaves out the `Version/… Safari/…` tail, and
/// Google refuses to sign in a browser it reads as embedded.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15";

/// The state of one `getAllCookies` round trip.
enum Fetch {
    Idle,
    InFlight,
    Done(Vec<Cookie>),
}

pub(crate) struct Window {
    window: Retained<NSWindow>,
    view: Retained<WKWebView>,
    cookies: Retained<WKHTTPCookieStore>,
    fetch: Rc<RefCell<Fetch>>,
}

impl Window {
    pub(crate) fn open(target: &Target) -> Result<Self> {
        let mtm =
            MainThreadMarker::new().context("the sign-in window has to open on the main thread")?;
        let url = NSURL::URLWithString(&NSString::from_str(&target.url))
            .context("cannot parse the sign-in url")?;
        let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(WIDTH, HEIGHT));

        // The view copies the configuration, so the store is read back off the view afterwards.
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        let store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) };
        unsafe { configuration.setWebsiteDataStore(&store) };
        let view = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &configuration)
        };
        unsafe { view.setCustomUserAgent(Some(&NSString::from_str(USER_AGENT))) };
        let cookies = unsafe { view.configuration().websiteDataStore().httpCookieStore() };

        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // Closing must not free the window under the `Retained` this struct holds.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&target.title));
        window.setMinSize(NSSize::new(MIN_WIDTH, MIN_HEIGHT));
        window.setContentView(Some(&view));
        window.center();
        window.makeKeyAndOrderFront(None);
        unsafe { view.loadRequest(&NSURLRequest::requestWithURL(&url)) };

        Ok(Self {
            window,
            view,
            cookies,
            fetch: Rc::new(RefCell::new(Fetch::Idle)),
        })
    }

    /// True once the user closed the window. A miniaturized window is not visible but still open.
    pub(crate) fn closed(&self) -> bool {
        !self.window.isVisible() && !self.window.isMiniaturized()
    }

    pub(crate) fn host(&self) -> Option<String> {
        let url = unsafe { self.view.URL() }?;
        url.host().map(|host| host.to_string())
    }

    /// Hands back a finished cookie fetch, or starts one when none is in flight.
    pub(crate) fn fetch(&mut self) -> Option<Vec<Cookie>> {
        let state = std::mem::replace(&mut *self.fetch.borrow_mut(), Fetch::Idle);
        match state {
            Fetch::Done(cookies) => return Some(cookies),
            Fetch::InFlight => {
                *self.fetch.borrow_mut() = Fetch::InFlight;
                return None;
            }
            Fetch::Idle => {}
        }
        *self.fetch.borrow_mut() = Fetch::InFlight;
        let slot = self.fetch.clone();
        let done = RcBlock::new(move |found: NonNull<NSArray<NSHTTPCookie>>| {
            let found = unsafe { found.as_ref() };
            let cookies = found
                .iter()
                .map(|cookie| Cookie {
                    name: cookie.name().to_string(),
                    value: cookie.value().to_string(),
                    domain: cookie.domain().to_string(),
                })
                .collect();
            *slot.borrow_mut() = Fetch::Done(cookies);
        });
        unsafe { self.cookies.getAllCookies(&done) };
        None
    }

    /// Forgets a finished fetch taken on a page that turned out not to be the landing.
    pub(crate) fn discard(&mut self) {
        let mut fetch = self.fetch.borrow_mut();
        if matches!(*fetch, Fetch::Done(_)) {
            *fetch = Fetch::Idle;
        }
    }

    pub(crate) fn close(&self) {
        if !self.closed() {
            self.window.close();
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.close();
    }
}
