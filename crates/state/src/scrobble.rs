use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use gpui::{App, AppContext as _, Context, Entity, Global};
use music::Track;

use crate::{AppSettings, Io, Playback, PlaybackState, Sonora};

const API_KEY: &str = "229cba3a244d51d26d5ab920456fe323";
const API_SECRET: &str = "9757bb1a40e08f6155e02eba3f3d3881";
const API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const AUTH_URL: &str = "http://www.last.fm/api/auth/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Browser approvals are polled this often, up to this long.
const APPROVAL_POLL: Duration = Duration::from_secs(5);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// A play counts after 30 seconds and half the track (at most 4 minutes in).
const MIN_SCROBBLE_SECS: u64 = 30;
const SCROBBLE_FRACTION_DIVISOR: u64 = 2;
const SCROBBLE_CAP_SECS: u64 = 4 * 60;
/// The API error for "user has not approved yet": keep polling.
const NOT_AUTHORIZED_YET: u32 = 14;

#[derive(Debug)]
struct ApiError {
    code: u32,
    message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "last.fm error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

fn sign(params: &[(String, String)]) -> String {
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut base = String::new();
    for (name, value) in &sorted {
        base.push_str(name);
        base.push_str(value);
    }
    base.push_str(API_SECRET);
    format!("{:x}", md5::compute(base))
}

async fn call(
    http: &reqwest::Client,
    method: &str,
    mut params: Vec<(String, String)>,
    session: Option<&str>,
) -> Result<serde_json::Value> {
    params.push(("method".to_owned(), method.to_owned()));
    params.push(("api_key".to_owned(), API_KEY.to_owned()));
    if let Some(key) = session {
        params.push(("sk".to_owned(), key.to_owned()));
    }
    let signature = sign(&params);
    params.push(("api_sig".to_owned(), signature));
    // `format` stays out of the signature by API contract.
    params.push(("format".to_owned(), "json".to_owned()));
    let reply: serde_json::Value = http
        .post(API_URL)
        .timeout(REQUEST_TIMEOUT)
        .form(&params)
        .send()
        .await
        .context("last.fm request failed")?
        .json()
        .await
        .context("last.fm answered outside JSON")?;
    if let Some(error) = reply.get("error") {
        let code = error
            .get("code")
            .and_then(|code| code.as_u64())
            .unwrap_or_default() as u32;
        let message = error
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("unknown failure")
            .to_owned();
        return Err(ApiError { code, message }.into());
    }
    Ok(reply)
}

/// Runs the browser approval flow: opens the approval page, then polls until
/// the user approves and a session key arrives.
pub async fn link() -> Result<(String, String)> {
    let http = reqwest::Client::new();
    let reply = call(&http, "auth.getToken", Vec::new(), None).await?;
    let token = reply
        .get("token")
        .and_then(|token| token.as_str())
        .context("last.fm withheld the auth token")?;
    open::that_in_background(format!("{AUTH_URL}?api_key={API_KEY}&token={token}"));
    let deadline = tokio::time::Instant::now() + APPROVAL_TIMEOUT;
    loop {
        match session(&http, token).await {
            Ok(linked) => return Ok(linked),
            Err(error) => {
                let waiting = error
                    .downcast_ref::<ApiError>()
                    .is_some_and(|api| api.code == NOT_AUTHORIZED_YET);
                if !waiting || tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
                tokio::time::sleep(APPROVAL_POLL).await;
            }
        }
    }
}

async fn session(http: &reqwest::Client, token: &str) -> Result<(String, String)> {
    let reply = call(
        http,
        "auth.getSession",
        vec![("token".to_owned(), token.to_owned())],
        None,
    )
    .await?;
    let session = reply
        .get("session")
        .context("last.fm withheld the session")?;
    let key = session
        .get("key")
        .and_then(|key| key.as_str())
        .context("last.fm withheld the session key")?
        .to_owned();
    let user = session
        .get("name")
        .and_then(|name| name.as_str())
        .unwrap_or_default()
        .to_owned();
    Ok((key, user))
}

struct Meta {
    title: String,
    artist: String,
    album: String,
    duration: u64,
}

fn meta_of(track: &Track) -> Meta {
    Meta {
        title: track.name.clone(),
        artist: track.artists.clone(),
        album: track.album.clone(),
        duration: track.duration.as_secs(),
    }
}

async fn now_playing(http: &reqwest::Client, session: &str, meta: &Meta) -> Result<()> {
    let mut params = vec![
        ("artist".to_owned(), meta.artist.clone()),
        ("track".to_owned(), meta.title.clone()),
    ];
    if !meta.album.is_empty() {
        params.push(("album".to_owned(), meta.album.clone()));
    }
    if meta.duration > 0 {
        params.push(("duration".to_owned(), meta.duration.to_string()));
    }
    call(http, "track.updateNowPlaying", params, Some(session)).await?;
    Ok(())
}

async fn scrobble(
    http: &reqwest::Client,
    session: &str,
    meta: &Meta,
    started_unix: u64,
) -> Result<()> {
    let mut params = vec![
        ("artist".to_owned(), meta.artist.clone()),
        ("track".to_owned(), meta.title.clone()),
        ("timestamp".to_owned(), started_unix.to_string()),
    ];
    if !meta.album.is_empty() {
        params.push(("album".to_owned(), meta.album.clone()));
    }
    if meta.duration > 0 {
        params.push(("duration".to_owned(), meta.duration.to_string()));
    }
    call(http, "track.scrobble", params, Some(session)).await?;
    Ok(())
}

struct Play {
    track: Track,
    started_unix: u64,
    eligible: bool,
}

fn play_key(track: &Track) -> (Option<String>, String, String) {
    (track.id.clone(), track.name.clone(), track.artists.clone())
}

/// Tracks without a title or an artist would pollute the profile, so they are
/// announced and scrobbled never.
fn scrobblable(track: &Track) -> bool {
    !track.name.trim().is_empty() && !track.artists.trim().is_empty()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

struct Attached {
    _scrobbler: Entity<Scrobbler>,
}

impl Global for Attached {}

pub fn attach(cx: &mut App) {
    if cx.has_global::<Attached>() {
        return;
    }
    let playback = Sonora::global(cx).playback.clone();
    let settings = Sonora::global(cx).settings.clone();
    let io = Io::global(cx);
    let scrobbler = cx.new(|cx| Scrobbler::new(playback, settings, io, cx));
    cx.set_global(Attached {
        _scrobbler: scrobbler,
    });
}

pub struct Scrobbler {
    playback: Entity<Playback>,
    settings: Entity<AppSettings>,
    io: Io,
    http: reqwest::Client,
    current: Option<Play>,
}

impl Scrobbler {
    fn new(
        playback: Entity<Playback>,
        settings: Entity<AppSettings>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&playback, |this, _, cx| this.publish(cx))
            .detach();
        cx.observe(&settings, |this, _, cx| this.publish(cx))
            .detach();
        Self {
            playback,
            settings,
            io,
            http: reqwest::Client::new(),
            current: None,
        }
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        let playback = self.playback.read(cx);
        let settings = self.settings.read(cx);
        let session = match settings.lastfm_enabled() {
            true => settings.lastfm_session(),
            false => None,
        };
        let track = playback.track().cloned();
        let key = track.as_ref().map(play_key);

        if key != self.current.as_ref().map(|play| play_key(&play.track)) {
            if let Some(play) = self.current.take()
                && play.eligible
                && let Some(session) = session.clone()
            {
                self.submit(play, session);
            }
            let Some(track) = track.filter(|track| session.is_some() && scrobblable(track)) else {
                return;
            };
            let session = session.expect("a session was just checked");
            let meta = meta_of(&track);
            let http = self.http.clone();
            self.io.spawn(async move {
                if let Err(error) = now_playing(&http, &session, &meta).await {
                    log::debug!("last.fm: cannot announce now playing: {error:#}");
                }
            });
            self.current = Some(Play {
                track,
                started_unix: unix_now(),
                eligible: false,
            });
        } else if let Some(play) = self.current.as_mut()
            && !play.eligible
            && matches!(playback.state(), PlaybackState::Playing)
        {
            let duration = play.track.duration.as_secs();
            let position = playback.live_position().as_secs();
            if duration > MIN_SCROBBLE_SECS
                && position >= (duration / SCROBBLE_FRACTION_DIVISOR).min(SCROBBLE_CAP_SECS)
            {
                play.eligible = true;
            }
        }
    }

    fn submit(&self, play: Play, session: String) {
        let http = self.http.clone();
        let meta = meta_of(&play.track);
        let started = play.started_unix;
        self.io.spawn(async move {
            if let Err(error) = scrobble(&http, &session, &meta, started).await {
                log::warn!("last.fm: cannot scrobble {}: {error:#}", meta.title);
            }
        });
    }
}
