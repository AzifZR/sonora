//! A native browser window with a throwaway session, for providers that sign in with cookies.
//!
//! The window loads a sign-in page in a data store that lives only as long as the window. Once the
//! page lands on the provider's own host with the proof cookies set, the cookie header is handed
//! back and the window closes. No browser profile ever holds that session, so nothing rotates the
//! cookies behind the app's back the way a shared browser session does.
//!
//! Only macOS has a backend. Every other platform reports `supported() == false` and `Login::open`
//! fails, so a caller falls back to pasting a header.

use anyhow::Result;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
use unsupported as platform;

/// What a sign-in window is asked to do. `url` opens first. The user is through once the page is on
/// `landing` and the cookies for `domain` carry at least one of the `proof` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    pub landing: String,
    pub domain: String,
    pub proof: Vec<String>,
    pub title: String,
}

/// One cookie as the page holds it. `domain` keeps the leading dot when the browser stored one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
}

/// Where a sign-in window is between two polls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poll {
    /// Still open, the user is not through yet.
    Pending,
    /// The user closed the window before signing in.
    Closed,
    /// The `Cookie` header for the target's domain. The window has closed itself.
    Cookies(String),
}

/// A sign-in window. Open it on the main thread and poll it from there; dropping it closes it.
pub struct Login {
    target: Target,
    window: platform::Window,
}

/// Whether this platform can open a sign-in window at all.
pub fn supported() -> bool {
    platform::SUPPORTED
}

impl Login {
    /// Opens the window and starts loading the target url. Must run on the main thread.
    pub fn open(target: Target) -> Result<Self> {
        let window = platform::Window::open(&target)?;
        Ok(Self { target, window })
    }

    /// Checks where the window is. Call it every few hundred milliseconds until it stops answering
    /// `Pending`; the cookies come back once, and the window closes on that poll.
    pub fn poll(&mut self) -> Poll {
        if self.window.closed() {
            return Poll::Closed;
        }
        let Some(host) = self.window.host() else {
            return Poll::Pending;
        };
        if host != self.target.landing {
            self.window.discard();
            return Poll::Pending;
        }
        let Some(cookies) = self.window.fetch() else {
            return Poll::Pending;
        };
        let header = header(&cookies, &self.target.domain);
        match proven(&header, &self.target.proof) {
            true => {
                self.window.close();
                Poll::Cookies(header)
            }
            false => Poll::Pending,
        }
    }
}

/// Joins the cookies a browser would send to `domain` into a `Cookie` header value.
fn header(cookies: &[Cookie], domain: &str) -> String {
    cookies
        .iter()
        .filter(|cookie| matches(&cookie.domain, domain))
        .map(|cookie| format!("{}={}", cookie.name, cookie.value))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Whether a cookie stored for `stored` is sent to `domain` and its subdomains.
fn matches(stored: &str, domain: &str) -> bool {
    let stored = stored.trim_start_matches('.');
    stored == domain || stored.ends_with(&format!(".{domain}"))
}

/// Whether the header names at least one of the proof cookies.
fn proven(header: &str, proof: &[String]) -> bool {
    header
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .any(|(name, _)| proof.iter().any(|wanted| wanted == name))
}

#[cfg(test)]
mod tests {
    use super::{Cookie, header, matches, proven};

    fn cookie(name: &str, domain: &str) -> Cookie {
        Cookie {
            name: name.to_string(),
            value: format!("{name}-value"),
            domain: domain.to_string(),
        }
    }

    #[test]
    fn keeps_the_domain_and_its_subdomains() {
        assert!(matches(".youtube.com", "youtube.com"));
        assert!(matches("youtube.com", "youtube.com"));
        assert!(matches("music.youtube.com", "youtube.com"));
        assert!(!matches(".google.com", "youtube.com"));
        assert!(!matches("notyoutube.com", "youtube.com"));
    }

    #[test]
    fn joins_only_the_matching_cookies() {
        let cookies = [
            cookie("SAPISID", ".youtube.com"),
            cookie("NID", ".google.com"),
            cookie("PREF", "music.youtube.com"),
        ];
        assert_eq!(
            header(&cookies, "youtube.com"),
            "SAPISID=SAPISID-value; PREF=PREF-value"
        );
    }

    #[test]
    fn proof_needs_one_of_the_names() {
        let proof = vec!["SAPISID".to_string(), "__Secure-3PAPISID".to_string()];
        assert!(proven("VISITOR=1; __Secure-3PAPISID=x", &proof));
        assert!(!proven("VISITOR=1; PREF=x", &proof));
        assert!(!proven("", &proof));
    }
}
