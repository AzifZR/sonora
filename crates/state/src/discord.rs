use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use gpui::{App, AppContext as _, Context, Entity, Global};

use crate::{Playback, PlaybackState, Sonora};

// Sonora Discord Application ID
const DISCORD_APP_ID: &str = "1547582803313561642";

/// Back off this long between Discord connects while it is unreachable.
const RECONNECT_COOLDOWN: Duration = Duration::from_secs(5);
/// Re-apply the current activity this often so a dropped frame never sticks.
const REFRESH_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone)]
enum Command {
    Update {
        title: String,
        artist: String,
        album: String,
        pos_secs: u64,
        dur_secs: u64,
        playing: bool,
        loading: bool,
    },
    Clear,
}

/// What Discord currently shows. Position is deliberately excluded: the progress
/// bar moves on its own from the timestamps, so only content, duration, or
/// state changes need a new frame.
#[derive(PartialEq, Eq)]
struct Shown {
    id: Option<String>,
    name: String,
    artists: String,
    album: String,
    dur_secs: u64,
    class: u8,
}

struct Attached {
    _discord: Entity<DiscordRpc>,
}

impl Global for Attached {}

pub fn attach(cx: &mut App) {
    if cx.has_global::<Attached>() {
        return;
    }
    let playback = Sonora::global(cx).playback.clone();
    let discord = cx.new(|cx| DiscordRpc::new(playback, cx));
    cx.set_global(Attached { _discord: discord });
}

pub struct DiscordRpc {
    playback: Entity<Playback>,
    sender: Sender<Command>,
    shown: Option<Shown>,
}

impl DiscordRpc {
    fn new(playback: Entity<Playback>, cx: &mut Context<Self>) -> Self {
        let (sender, receiver) = mpsc::channel();

        std::thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || rpc_worker(receiver))
            .ok();

        cx.observe(&playback, |this, _, cx| this.publish(cx))
            .detach();

        Self {
            playback,
            sender,
            shown: None,
        }
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        let playback = self.playback.read(cx);
        let class = match playback.state() {
            PlaybackState::Playing => 0,
            PlaybackState::Paused => 1,
            PlaybackState::Loading => 2,
            PlaybackState::Idle | PlaybackState::Failed(_) => 3,
        };

        // Idle/Failed always clear, even when a track is still retained.
        if class == 3 {
            let key = Shown {
                id: None,
                name: String::new(),
                artists: String::new(),
                album: String::new(),
                dur_secs: 0,
                class,
            };
            if self.shown.as_ref() == Some(&key) {
                return;
            }
            self.shown = Some(key);
            let _ = self.sender.send(Command::Clear);
            return;
        }
        let Some(track) = playback.track() else {
            return;
        };

        // Covers and reuploads (plain YouTube videos outside the Music catalog)
        // often carry no artist or album metadata. Fall back so Discord never
        // shows a blank line.
        let artist = match track.artists.trim().is_empty() {
            false => track.artists.clone(),
            true if !track.album.trim().is_empty() => track.album.clone(),
            true => String::from("Unknown Artist"),
        };

        let key = Shown {
            id: track.id.clone(),
            name: track.name.clone(),
            artists: track.artists.clone(),
            album: track.album.clone(),
            dur_secs: track.duration.as_secs(),
            class,
        };
        if self.shown.as_ref() == Some(&key) {
            return;
        }
        self.shown = Some(key);

        let _ = self.sender.send(Command::Update {
            title: track.name.clone(),
            artist,
            album: track.album.clone(),
            pos_secs: playback.live_position().as_secs(),
            dur_secs: track.duration.as_secs(),
            playing: class == 0,
            loading: class == 2,
        });
    }
}

fn rpc_worker(receiver: Receiver<Command>) {
    let mut client = DiscordIpcClient::new(DISCORD_APP_ID);
    let mut connected = false;
    let mut last_attempt = Instant::now() - REFRESH_EVERY;
    let mut pending: Option<Command> = None;
    let mut applied: Option<Command> = None;

    loop {
        if pending.is_none() {
            match receiver.recv_timeout(REFRESH_EVERY) {
                Ok(cmd) => pending = Some(cmd),
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    // Periodic refresh: re-apply the current activity so a
                    // frame Discord dropped never sticks around.
                    if connected && let Some(Command::Update { .. }) = &applied {
                        pending = applied.clone();
                    }
                }
            }
        }
        while let Ok(newer) = receiver.try_recv() {
            pending = Some(newer);
        }

        if !connected {
            if last_attempt.elapsed() < RECONNECT_COOLDOWN {
                // Keep pending for the next attempt; sleep so a queued retry
                // does not spin while the cooldown runs out.
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            last_attempt = Instant::now();
            if client.connect().is_ok() {
                connected = true;
                log::info!("discord-rpc: connected to Discord");
            } else {
                continue;
            }
        }

        if let Some(cmd) = pending.take() {
            if apply(&mut client, &cmd) {
                applied = Some(cmd);
            } else {
                log::debug!("discord-rpc: lost Discord connection, will retry");
                let _ = client.close();
                connected = false;
                pending = Some(cmd);
            }
        }
    }

    let _ = client.close();
}

fn apply(client: &mut DiscordIpcClient, cmd: &Command) -> bool {
    let result = match cmd {
        Command::Update {
            title,
            artist,
            album,
            pos_secs,
            dur_secs,
            playing,
            loading,
        } => {
            let mut assets = activity::Assets::new()
                .large_image("sonora")
                .large_text(album.as_str());
            if *playing {
                assets = assets.small_image("play").small_text("Playing");
            } else if *loading {
                assets = assets.small_image("pause").small_text("Loading");
            } else {
                assets = assets.small_image("pause").small_text("Paused");
            }

            let mut act = activity::Activity::new()
                .details(title.as_str())
                .state(artist.as_str())
                .assets(assets)
                .activity_type(activity::ActivityType::Listening);

            if *playing && *dur_secs > 0 {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let start = now - *pos_secs as i64;
                act = act.timestamps(
                    activity::Timestamps::new()
                        .start(start)
                        .end(start + *dur_secs as i64),
                );
            }
            client.set_activity(act)
        }
        Command::Clear => client.clear_activity(),
    };
    result.is_ok()
}
