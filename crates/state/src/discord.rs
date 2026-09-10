use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use i18n::t;
use music::Track;
use tokio::sync::watch;

use crate::{AppSettings, Cover, Io, Playback, Session};

const APPLICATION_ID: &str = "1547350467904806923";
const RETRY_DELAY: Duration = Duration::from_secs(3);
const SWITCH_DEBOUNCE: Duration = Duration::from_secs(1);
const MAX_START_DRIFT_SECONDS: u64 = 2;
const MAX_TEXT_UTF16_UNITS: usize = 128;

struct Attached {
    _discord: Entity<Discord>,
}

impl Global for Attached {}

pub(crate) fn attach(
    playback: Entity<Playback>,
    settings: Entity<AppSettings>,
    session: Entity<Session>,
    cover: Entity<Cover>,
    io: Io,
    cx: &mut App,
) {
    let discord = cx.new(|cx| Discord::new(playback, settings, session, cover, io, cx));
    cx.set_global(Attached { _discord: discord });
}

#[derive(Clone, Debug, Default, PartialEq)]
enum Shown {
    #[default]
    Off,
    On(Presence),
}

#[derive(Clone, Debug, PartialEq)]
struct Presence {
    provider: Option<Provider>,
    details: String,
    state: Option<String>,
    image: Option<String>,
    image_text: Option<String>,
    started_at: i64,
    ends_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Provider {
    Spotify,
    YouTube,
}

impl Provider {
    fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "spotify" => Some(Self::Spotify),
            "youtube" => Some(Self::YouTube),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Spotify => "Spotify",
            Self::YouTube => "YouTube Music",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Spotify => "https://open.spotifycdn.com/cdn/images/favicon32.b64ecc03.png",
            Self::YouTube => "https://music.youtube.com/img/favicon_144.png",
        }
    }
}

#[derive(Default)]
struct Timing {
    listening_since: Option<i64>,
    track: Option<String>,
    track_started_at: Option<i64>,
}

impl Timing {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn listening_since(&mut self) -> i64 {
        *self.listening_since.get_or_insert_with(unix_time)
    }

    fn track_started_at(&mut self, track: &Track, position: Duration) -> i64 {
        let measured = unix_time().saturating_sub(position.as_secs() as i64);
        let moved = self
            .track_started_at
            .is_none_or(|started_at| started_at.abs_diff(measured) > MAX_START_DRIFT_SECONDS);
        if moved || self.track.as_deref() != track.id.as_deref() {
            self.track = track.id.clone();
            self.track_started_at = Some(measured);
        }
        self.track_started_at.unwrap_or(measured)
    }
}

/// Watches playback and settings, and hands each new presence to the worker.
struct Discord {
    playback: Entity<Playback>,
    settings: Entity<AppSettings>,
    session: Entity<Session>,
    cover: Entity<Cover>,
    sender: watch::Sender<Shown>,
    timing: Timing,
    _worker: Task<()>,
}

impl Discord {
    fn new(
        playback: Entity<Playback>,
        settings: Entity<AppSettings>,
        session: Entity<Session>,
        cover: Entity<Cover>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        let (sender, receiver) = watch::channel(Shown::Off);
        let worker = io.spawn(run(receiver));
        let _worker = cx.spawn(async move |_, _| {
            if let Err(error) = worker.await {
                log::warn!("discord: the presence worker stopped: {error}");
            }
        });

        cx.observe(&playback, |this, _, cx| this.publish(cx))
            .detach();
        cx.observe(&settings, |this, _, cx| this.publish(cx))
            .detach();
        cx.observe(&cover, |this, _, cx| this.publish(cx)).detach();

        Self {
            playback,
            settings,
            session,
            cover,
            sender,
            timing: Timing::default(),
            _worker,
        }
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown(cx);
        self.sender.send_if_modified(|current| {
            let changed = *current != shown;
            if changed {
                *current = shown;
            }
            changed
        });
    }

    fn shown(&mut self, cx: &App) -> Shown {
        let playback = self.playback.read(cx);
        let Some(track) = playback.track().filter(|_| playback.wants_playing()) else {
            self.timing.reset();
            return Shown::Off;
        };

        let since = self.timing.listening_since();
        let settings = self.settings.read(cx);
        if !settings.discord_presence() {
            return Shown::Off;
        }

        let provider = track
            .id
            .as_deref()
            .and_then(|id| self.session.read(cx).slug_for(id))
            .and_then(Provider::from_slug);
        let named = provider.filter(|_| settings.discord_as_provider());
        if settings.discord_without_details() {
            return Shown::On(Presence {
                provider: named,
                details: anonymous_details(),
                state: None,
                image: None,
                image_text: None,
                started_at: since,
                ends_at: None,
            });
        }

        let started_at = self
            .timing
            .track_started_at(track, playback.live_position());
        let duration = track.duration.as_secs() as i64;
        Shown::On(Presence {
            provider: named,
            details: fit_text(&track.name).unwrap_or_else(anonymous_details),
            state: fit_text(&track.artists),
            image: self.artwork(track, provider, cx),
            image_text: fit_text(&track.album),
            started_at,
            ends_at: (duration > 0).then(|| started_at.saturating_add(duration)),
        })
    }

    /// Cover art for the track, but only for a provider whose art is a public URL. A local
    /// file means nothing to Discord, and a private server's URL is nobody else's business.
    fn artwork(&self, track: &Track, provider: Option<Provider>, cx: &App) -> Option<String> {
        provider?;
        let album = track.album_id.as_deref()?;
        self.cover
            .read(cx)
            .large_for(album)
            .map(str::to_owned)
            .or_else(|| track.cover.clone())
            .filter(|cover| cover.starts_with("https://"))
    }
}

/// Why a presence did not land. A refused payload is permanent; an unreachable socket is
/// worth trying again.
enum Failure {
    Unreachable(String),
    Refused(String),
}

async fn run(mut receiver: watch::Receiver<Shown>) {
    let mut client = DiscordIpcClient::new(APPLICATION_ID);
    let mut shown = Shown::Off;
    let mut reported: Option<String> = None;

    while let Some(wanted) = next_presence(&mut receiver, &shown).await {
        // the socket blocks on both halves of a round trip, so it gets a thread of its own
        let attempted = tokio::task::spawn_blocking(move || {
            let result = send(&mut client, &wanted);
            (client, wanted, result)
        })
        .await;

        let Ok((used, wanted, result)) = attempted else {
            log::warn!("discord: the presence thread went away");
            return;
        };
        client = used;

        match result {
            Ok(()) => {
                shown = wanted;
                reported = None;
            }
            // Offering the same refused payload again would only reconnect forever, so it
            // counts as shown and waits for the next change.
            Err(Failure::Refused(message)) => {
                complain(&mut reported, message);
                shown = wanted;
            }
            Err(Failure::Unreachable(message)) => {
                complain(&mut reported, message);
                tokio::select! {
                    changed = receiver.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                    () = tokio::time::sleep(RETRY_DELAY) => {}
                }
            }
        }
    }
}

/// Returns the next useful presence. Replacements are debounced until the listener settles on a
/// track, while turning presence off remains immediate.
async fn next_presence(receiver: &mut watch::Receiver<Shown>, shown: &Shown) -> Option<Shown> {
    loop {
        let wanted = receiver.borrow_and_update().clone();
        if &wanted == shown {
            receiver.changed().await.ok()?;
            continue;
        }
        if !matches!((shown, &wanted), (Shown::On(_), Shown::On(_))) {
            return Some(wanted);
        }

        tokio::select! {
            biased;

            changed = receiver.changed() => {
                changed.ok()?;
            }
            () = tokio::time::sleep(SWITCH_DEBOUNCE) => {
                return Some(wanted);
            }
        }
    }
}

/// Says what went wrong once, however long it goes on going wrong.
fn complain(reported: &mut Option<String>, failure: String) {
    if reported.as_ref() == Some(&failure) {
        return;
    }

    log::warn!("discord: cannot show the presence: {failure}");
    *reported = Some(failure);
}

/// Offers one presence to Discord, reconnecting once for a socket that has gone stale. The
/// client connects lazily, so the first offer of a session always takes this second path.
fn send(client: &mut DiscordIpcClient, shown: &Shown) -> Result<(), Failure> {
    match offer(client, shown) {
        Err(Failure::Unreachable(_)) => {
            client
                .connect()
                .map_err(|error| Failure::Unreachable(error.to_string()))?;
            offer(client, shown)
        }
        result => result,
    }
}

fn offer(client: &mut DiscordIpcClient, shown: &Shown) -> Result<(), Failure> {
    match activity(shown) {
        Some(activity) => client.set_activity(activity),
        None => client.clear_activity(),
    }
    .map_err(|error| Failure::Unreachable(error.to_string()))?;

    let (_, answer) = client
        .recv()
        .map_err(|error| Failure::Unreachable(error.to_string()))?;
    if answer.get("evt").and_then(|event| event.as_str()) != Some("ERROR") {
        return Ok(());
    }

    let message = answer
        .pointer("/data/message")
        .and_then(|message| message.as_str())
        .unwrap_or("discord turned the activity down");
    Err(Failure::Refused(message.to_owned()))
}

/// The activity to send, or nothing when the presence is to be cleared.
fn activity(shown: &Shown) -> Option<activity::Activity<'_>> {
    let Shown::On(presence) = shown else {
        return None;
    };

    let mut activity = activity::Activity::new()
        .activity_type(activity::ActivityType::Listening)
        .details(presence.details.as_str());
    if let Some(provider) = presence.provider {
        activity = activity.name(provider.name());
    }
    if let Some(state) = presence.state.as_deref() {
        activity = activity.state(state);
    }
    if let Some(assets) = assets(presence) {
        activity = activity.assets(assets);
    }

    let mut timestamps = activity::Timestamps::new().start(presence.started_at);
    if let Some(ends_at) = presence.ends_at {
        timestamps = timestamps.end(ends_at);
    }
    Some(activity.timestamps(timestamps))
}

/// The artwork half of an activity, when there is any.
fn assets(presence: &Presence) -> Option<activity::Assets<'_>> {
    let image = presence.image.as_deref();
    let text = presence.image_text.as_deref();
    if image.is_none() && text.is_none() && presence.provider.is_none() {
        return None;
    }

    let mut assets = activity::Assets::new();
    if let Some(image) = image {
        assets = assets.large_image(image);
    }
    if let Some(text) = text {
        assets = assets.large_text(text);
    }
    if let Some(provider) = presence.provider {
        assets = assets
            .small_image(provider.icon())
            .small_text(provider.name());
    }
    Some(assets)
}

/// What the status says when the track is deliberately left out of it.
fn anonymous_details() -> String {
    let text = t!("discord-listening").to_string();
    fit_text(&text).unwrap_or(text)
}

/// Cuts a value to what Discord accepts in a text field, or drops it if nothing is left.
/// A lone character is padded with a non-breaking space, which Discord counts as the second
/// unit it insists on and draws as nothing.
fn fit_text(value: &str) -> Option<String> {
    let mut units = 0;
    let mut fitted = String::new();
    for character in value.trim().chars() {
        units += character.len_utf16();
        if units > MAX_TEXT_UTF16_UNITS {
            break;
        }
        fitted.push(character);
    }

    let mut fitted = fitted.trim_end().to_owned();
    match fitted.chars().count() {
        0 => None,
        1 => {
            fitted.push('\u{a0}');
            Some(fitted)
        }
        _ => Some(fitted),
    }
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
