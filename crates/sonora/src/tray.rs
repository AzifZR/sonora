#[cfg(any(target_os = "macos", windows))]
mod native;
#[cfg(target_os = "linux")]
mod sni;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use i18n::t;
use state::{PlaybackState, Sonora};
use tokio::sync::mpsc::{self, UnboundedReceiver};

#[cfg(any(target_os = "macos", windows))]
use native::Icon;
#[cfg(target_os = "linux")]
use sni::Icon;

const CAPTION_LIMIT: usize = 26;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Show,
    Toggle,
    Previous,
    Next,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
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

        let shown = shown(cx);
        icon.show(&shown);
        Self {
            icon,
            shown,
            _events,
        }
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        let shown = shown(cx);
        if shown == self.shown {
            return;
        }
        self.icon.show(&shown);
        self.shown = shown;
    }
}

fn shown(cx: &App) -> Shown {
    let playback = Sonora::global(cx).playback.read(cx);
    let playing = matches!(
        playback.state(),
        PlaybackState::Playing | PlaybackState::Loading
    );
    let caption = match playback.track() {
        Some(track) => {
            let full = match track.artists.is_empty() {
                true => track.name.clone(),
                false => format!("{} – {}", track.artists, track.name),
            };
            clip(&full, CAPTION_LIMIT)
        }
        None => t!("player-nothing-playing").to_string(),
    };
    Shown {
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

/// Truncates `text` to at most `limit` characters, appending an ellipsis if shortened.
fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some(_) => {
            let cut = limit.saturating_sub(1);
            let offset = text
                .char_indices()
                .nth(cut)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            let trimmed = text[..offset].trim_end_matches(|c: char| {
                c.is_whitespace() || c == '-' || c == '–' || c == '—' || c == ',' || c == '('
            });
            format!("{trimmed}…")
        }
        None => text.to_string(),
    }
}
