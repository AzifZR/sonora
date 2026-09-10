#[cfg(any(target_os = "macos", windows))]
mod native;
#[cfg(target_os = "linux")]
mod sni;

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use futures::AsyncReadExt as _;
use gpui::http_client::{AsyncBody, HttpClient};
use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use i18n::t;
use state::{PlaybackState, Sonora};
use tokio::sync::mpsc::{self, UnboundedReceiver};

#[cfg(any(target_os = "macos", windows))]
use native::Icon;
#[cfg(target_os = "linux")]
use sni::Icon;

/// The longest side of the cover handed to the menu. macOS draws it 18pt tall whatever it
/// measures, Windows and the status notifier hosts draw it at its own size.
const COVER: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Show,
    Toggle,
    Previous,
    Next,
    Quit,
}

/// The cover of the playing track, decoded and scaled for a menu row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Art {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub artwork: Option<Art>,
    pub caption: String,
    pub toggle: String,
    pub previous: String,
    pub next: String,
    pub show: String,
    pub quit: String,
    pub playing: bool,
}

struct Installed {
    _tray: Entity<Tray>,
}

impl Global for Installed {}

pub fn install(show: impl Fn(&mut App) + 'static, cx: &mut App) -> bool {
    let (sender, receiver) = mpsc::unbounded_channel();
    let Some(icon) = Icon::new(sender) else {
        return false;
    };
    let tray = cx.new(|cx| Tray::new(icon, receiver, show, cx));
    cx.set_global(Installed { _tray: tray });
    true
}

pub struct Tray {
    icon: Icon,
    shown: Shown,
    /// The cover the art below was loaded from, so a repeat of the same track loads nothing.
    cover: Option<String>,
    art: Option<Art>,
    artwork: Option<Task<()>>,
    _events: Task<()>,
}

impl Tray {
    fn new(
        mut icon: Icon,
        mut receiver: UnboundedReceiver<Event>,
        show: impl Fn(&mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let _events = cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.recv().await {
                if this.upgrade().is_none() {
                    break;
                }
                cx.update(|cx| match event {
                    Event::Show => show(cx),
                    Event::Quit => cx.quit(),
                    Event::Toggle | Event::Previous | Event::Next => {
                        let playback = Sonora::global(cx).playback.clone();
                        playback.update(cx, |playback, cx| match event {
                            Event::Toggle => playback.toggle_play(cx),
                            Event::Previous => playback.previous(cx),
                            _ => playback.next(cx),
                        });
                    }
                });
            }
        });

        let playback = Sonora::global(cx).playback.clone();
        cx.observe(&playback, |this, _, cx| this.publish(cx))
            .detach();

        let shown = shown(None, cx);
        icon.show(&shown);
        let mut tray = Self {
            icon,
            shown,
            cover: None,
            art: None,
            artwork: None,
            _events,
        };
        tray.follow(cx);
        tray
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        self.follow(cx);
        let shown = shown(self.art.clone(), cx);
        if shown == self.shown {
            return;
        }
        self.icon.show(&shown);
        self.shown = shown;
    }

    /// Starts loading the cover of the track that is playing now. Dropping the task cancels the
    /// load for the track that was playing before, so a run of skips only draws the last cover.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let cover = Sonora::global(cx)
            .playback
            .read(cx)
            .track()
            .and_then(|track| track.cover.clone());
        if cover == self.cover {
            return;
        }

        self.cover = cover.clone();
        self.art = None;
        self.artwork = cover.map(|cover| self.load(cover, cx));
    }

    fn load(&self, cover: String, cx: &mut Context<Self>) -> Task<()> {
        let http = cx.http_client();
        cx.spawn(async move |this, cx| {
            let art = match cx.background_spawn(art(cover, http)).await {
                Ok(art) => art,
                Err(error) => return log::warn!("tray: cannot draw the cover: {error:#}"),
            };
            this.update(cx, |this, cx| {
                this.art = Some(art);
                this.publish(cx);
            })
            .ok();
        })
    }
}

/// Reads a cover, over http or from the disk, and scales it down to a menu row. An unreadable
/// cover leaves the caption on its own rather than holding up the rest of the menu.
async fn art(cover: String, http: Arc<dyn HttpClient>) -> Result<Art> {
    let bytes = match cover.starts_with("http://") || cover.starts_with("https://") {
        true => fetch(&http, &cover).await?,
        false => {
            let path = cover.strip_prefix("file://").unwrap_or(&cover);
            std::fs::read(path).context("cannot read the cover")?
        }
    };
    let image = image::load_from_memory(&bytes)
        .context("cannot decode the cover")?
        .thumbnail(COVER, COVER)
        .into_rgba8();

    let (width, height) = image.dimensions();
    Ok(Art {
        data: image.into_raw(),
        width,
        height,
    })
}

async fn fetch(http: &Arc<dyn HttpClient>, url: &str) -> Result<Vec<u8>> {
    let mut response = http
        .get(url, AsyncBody::empty(), true)
        .await
        .context("cannot fetch the cover")?;
    if !response.status().is_success() {
        bail!("the cover request answered {}", response.status());
    }

    let mut bytes = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut bytes)
        .await
        .context("cannot read the cover")?;

    Ok(bytes)
}

fn shown(artwork: Option<Art>, cx: &App) -> Shown {
    let playback = Sonora::global(cx).playback.read(cx);
    let playing = matches!(
        playback.state(),
        PlaybackState::Playing | PlaybackState::Loading
    );
    let caption = match playback.track() {
        Some(track) => format!("{} – {}", track.artists, track.name),
        None => t!("player-nothing-playing").to_string(),
    };
    Shown {
        artwork,
        caption,
        toggle: match playing {
            true => t!("tray-pause"),
            false => t!("tray-play"),
        }
        .to_string(),
        previous: t!("player-previous").to_string(),
        next: t!("player-next").to_string(),
        show: t!("tray-show").to_string(),
        quit: t!("app-quit").to_string(),
        playing,
    }
}
