//! Reads what's playing from every app on Windows (Apple Music, Spotify, browsers, ...)
//! through Windows' system media controls, the same source as the volume flyout.
//!
//! All of it runs on one background thread:
//! - Windows tells us when anything changes (song, play/pause, position, apps opening or
//!   closing); we then build a fresh `MediaSnapshot` and post it to the UI thread.
//! - The UI sends back `Command`s (play/pause, next, seek, ...), which we forward to the app.
//! - Album art is decoded and resized here, so the UI thread never blocks.

use std::collections::HashMap;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{Interface, IUnknown, HSTRING};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::*;
use windows::Media::MediaPlaybackAutoRepeatMode;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::WinRT::{CreateStreamOverRandomAccessStream, RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;


/// Posted to the notch window whenever a new snapshot is ready.
pub const WM_MEDIA_UPDATE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;

/// Album art sizes, in pixels. Big enough for the hover card at 300% scaling.
const ART_BIG: u32 = 192;
const ART_SMALL: u32 = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Playing,
    Paused,
    Stopped,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Repeat {
    Off,
    All,
    One,
}

/// Decoded album art: square, premultiplied BGRA pixels.
pub struct Art {
    pub key: u64,
    pub big: Vec<u8>,
    pub big_size: u32,
    pub small: Vec<u8>,
    pub small_size: u32,
    /// A bright colour taken from the artwork (for the waveform), RGB 0..1.
    pub accent: [f32; 3],
}

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub app_name: String,
    pub title: String,
    pub artist: String,
    pub status: Status,
    pub can_play_pause: bool,
    pub can_next: bool,
    pub can_prev: bool,
    pub can_seek: bool,
    pub shuffle: Option<bool>,
    pub repeat: Option<Repeat>,
    /// Playback position (seconds) at `sampled_at`, and total length (0 = unknown).
    pub position: f64,
    pub duration: f64,
    pub sampled_at: Instant,
    pub art: Option<Arc<Art>>,
}

impl Session {
    /// Position right now, moving forward on its own while playing.
    pub fn position_now(&self) -> f64 {
        let mut p = self.position;
        if self.status == Status::Playing {
            p += self.sampled_at.elapsed().as_secs_f64();
        }
        if self.duration > 0.0 {
            p = p.min(self.duration);
        }
        p.max(0.0)
    }
}

#[derive(Default)]
pub struct MediaSnapshot {
    pub sessions: Vec<Session>,
    /// The session Windows itself considers current (what media keys control).
    pub system_current: Option<String>,
    /// A short message for the user, e.g. when an app refuses a shuffle/repeat change.
    pub notice: Option<String>,
}

pub enum Command {
    TogglePlayPause(String),
    Next(String),
    Previous(String),
    Seek(String, f64),
    ToggleShuffle(String),
    CycleRepeat(String),
}

enum Msg {
    Changed,
    /// A session's song details changed (title, artist or cover): decode that app's art again.
    PropsChanged(String),
    Command(Command),
}

/// Handle the UI keeps: send commands, and pick up the latest snapshot.
pub struct Media {
    tx: Sender<Msg>,
    latest: Arc<Mutex<Option<MediaSnapshot>>>,
}

impl Media {
    /// Start the background reader. `notify` gets `WM_MEDIA_UPDATE` on every change.
    pub fn start(notify: HWND) -> Media {
        let (tx, rx) = channel::<Msg>();
        let latest = Arc::new(Mutex::new(None));
        let worker_tx = tx.clone();
        let worker_latest = latest.clone();
        let notify = notify.0 as isize; // HWND isn't Send; pass the raw value

        std::thread::Builder::new()
            .name("media".into())
            .spawn(move || unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
                let notify = HWND(notify as *mut _);
                let Ok(mut reader) = Reader::new(worker_tx) else { return }; // no media controls on this PC
                reader.publish(&worker_latest, notify);
                loop {
                    // Wake on a change, or every few seconds to stay in sync regardless.
                    let first = match rx.recv_timeout(Duration::from_secs(4)) {
                        Ok(msg) => Some(msg),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    };
                    let mut changed = first.is_none();
                    if let Some(msg) = first {
                        changed |= reader.handle(msg);
                    }
                    // Apps often fire several events at once: gather them for a moment.
                    while let Ok(msg) = rx.recv_timeout(Duration::from_millis(25)) {
                        changed |= reader.handle(msg);
                    }
                    if changed {
                        reader.publish(&worker_latest, notify);
                    }
                }
            })
            .expect("start media thread");

        Media { tx, latest }
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(Msg::Command(command));
    }

    pub fn take_snapshot(&self) -> Option<MediaSnapshot> {
        self.latest.lock().ok()?.take()
    }
}

/// A song's decoded cover, the size of the picture it came from, and when it was read.
struct Cached {
    art: Arc<Art>,
    size: u64,
    at: Instant,
    /// Play state when the watch (re)started: see `rewatch`.
    status: Status,
    /// The app it belongs to (its AppUserModelID).
    app: String,
}

/// A play/pause change starts the cover watch again: Apple Music can open with a song loaded
/// and a placeholder cover, and only fetch the real one when play is pressed.
fn rewatch(c: &mut Cached, status: Status) {
    if c.status != status {
        c.status = status;
        c.at = Instant::now();
    }
}

/// How long after loading a cover it is still checked for a silent swap (see `cover_swapped`).
const COVER_WATCH: Duration = Duration::from_secs(30);

/// Just after Apple Music opens, its first song gets a placeholder cover that the app later
/// replaces with the real one without signalling anything. While a cover is young, a
/// different picture size means it was swapped and should be loaded again.
fn cover_swapped(cached_size: u64, now_size: Option<u64>, age: Duration) -> bool {
    age < COVER_WATCH && now_size.is_some_and(|n| n != cached_size)
}

/// The song's cover picture, opened for reading, and its size in bytes.
fn open_thumb(
    props: &GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<(windows::Storage::Streams::IRandomAccessStreamWithContentType, u64)> {
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?;
    Some((stream, size))
}

struct Subscribed {
    session: GlobalSystemMediaTransportControlsSession,
    tokens: [i64; 3],
}

struct Reader {
    notice: Option<String>,
    manager: GlobalSystemMediaTransportControlsSessionManager,
    tx: Sender<Msg>,
    subscribed: Vec<Subscribed>,
    wic: Option<IWICImagingFactory>,
    art_cache: HashMap<u64, Cached>,
    /// The last published snapshot had no sessions (so an empty one changes nothing).
    published_empty: bool,
}

impl Reader {
    unsafe fn new(tx: Sender<Msg>) -> windows::core::Result<Reader> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.join()?;
        let on_change = |tx: &Sender<Msg>| {
            let tx = tx.clone();
            move || {
                let _ = tx.send(Msg::Changed);
            }
        };
        let f = on_change(&tx);
        manager.SessionsChanged(&TypedEventHandler::new(move |_, _| {
            f();
            Ok(())
        }))?;
        let f = on_change(&tx);
        manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
            f();
            Ok(())
        }))?;
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let wic = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
        Ok(Reader {
            notice: None,
            manager,
            tx,
            subscribed: Vec::new(),
            wic,
            art_cache: HashMap::new(),
            published_empty: false,
        })
    }

    /// Act on one message; true if the snapshot needs refreshing.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Changed => {}
            // Apple Music sends a song's title first and its cover a moment later, in a second
            // signal; reusing the cached art would keep a missing or stale cover all song long.
            Msg::PropsChanged(app) => self.art_cache.retain(|_, c| c.app != app),
            Msg::Command(cmd) => self.run(cmd),
        }
        true
    }

    fn sessions(&self) -> Vec<GlobalSystemMediaTransportControlsSession> {
        self.manager
            .GetSessions()
            .map(|list| list.into_iter().collect())
            .unwrap_or_default()
    }

    /// Listen for changes on each session (song, play state, position).
    fn resubscribe(&mut self, sessions: &[GlobalSystemMediaTransportControlsSession]) {
        // Same session objects, not just the same app ids: an app can swap a session for a new one.
        let identity = |s: &GlobalSystemMediaTransportControlsSession| s.cast::<IUnknown>().ok();
        let same = sessions.len() == self.subscribed.len()
            && sessions.iter().zip(&self.subscribed).all(|(a, b)| identity(a) == identity(&b.session));
        if same {
            return;
        }
        for old in self.subscribed.drain(..) {
            let _ = old.session.RemoveMediaPropertiesChanged(old.tokens[0]);
            let _ = old.session.RemovePlaybackInfoChanged(old.tokens[1]);
            let _ = old.session.RemoveTimelinePropertiesChanged(old.tokens[2]);
        }
        for session in sessions {
            let app = session.SourceAppUserModelId().map(|id| id.to_string()).unwrap_or_default();
            let changed = || {
                let tx = self.tx.clone();
                move || {
                    let _ = tx.send(Msg::Changed);
                }
            };
            let tx = self.tx.clone();
            let a = move || {
                let _ = tx.send(Msg::PropsChanged(app.clone()));
            };
            let (b, c) = (changed(), changed());
            let tokens = [
                session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                    a();
                    Ok(())
                })),
                session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                    b();
                    Ok(())
                })),
                session.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                    c();
                    Ok(())
                })),
            ];
            if let [Ok(a), Ok(b), Ok(c)] = tokens {
                self.subscribed.push(Subscribed { session: session.clone(), tokens: [a, b, c] });
            }
        }
    }

    unsafe fn publish(&mut self, latest: &Mutex<Option<MediaSnapshot>>, notify: HWND) {
        let sessions = self.sessions();
        self.resubscribe(&sessions);

        let mut seen: HashMap<String, u32> = HashMap::new();
        let mut out = Vec::new();
        for session in &sessions {
            // Two sessions from one app get distinct ids: "#n" is the session's position among
            // all of that app's sessions (skipped ones included), as run() looks it up.
            let app = session.SourceAppUserModelId().map(|id| id.to_string()).unwrap_or_default();
            let n = seen.entry(app).or_insert(0);
            let index = *n;
            *n += 1;
            if let Some(mut info) = self.read_session(session) {
                if index > 0 {
                    info.id = format!("{}#{}", info.id, index);
                    info.app_name = format!("{} {}", info.app_name, index + 1); // "Chrome 2"
                }
                out.push(info);
            }
        }
        let system_current = self
            .manager
            .GetCurrentSession()
            .ok()
            .and_then(|s| s.SourceAppUserModelId().ok())
            .map(|id| id.to_string());

        // Forget art for songs that are gone.
        let live: Vec<u64> = out.iter().filter_map(|s| s.art.as_ref().map(|a| a.key)).collect();
        self.art_cache.retain(|key, _| live.contains(key));

        // Nothing playing, as before: don't wake the notch for nothing (the 4-second resync).
        let empty = out.is_empty();
        if empty && self.published_empty && self.notice.is_none() {
            return;
        }
        self.published_empty = empty;
        if let Ok(mut slot) = latest.lock() {
            *slot = Some(MediaSnapshot { sessions: out, system_current, notice: self.notice.take() });
        }
        let _ = PostMessageW(Some(notify), WM_MEDIA_UPDATE, WPARAM(0), LPARAM(0));
    }

    unsafe fn read_session(&mut self, session: &GlobalSystemMediaTransportControlsSession) -> Option<Session> {
        let id = session.SourceAppUserModelId().ok()?.to_string();
        let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
        let title = props.Title().map(|s| s.to_string()).unwrap_or_default().trim().to_string();
        let artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
        let album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
        let mut artist = clean_artist(&artist, &album);
        let mut title = title;
        if title.is_empty() {
            // Some browsers put the only name they have in the artist field.
            title = std::mem::take(&mut artist);
        }
        if title.is_empty() && artist.is_empty() {
            return None; // nothing loaded yet
        }

        let playback = session.GetPlaybackInfo().ok()?;
        let status = match playback.PlaybackStatus().ok()? {
            // "Changing" (between tracks) and "Opened" are brief in-between states: counting
            // them as stopped would drop the source you picked (see pick_source).
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing
            | GlobalSystemMediaTransportControlsSessionPlaybackStatus::Changing => Status::Playing,
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused
            | GlobalSystemMediaTransportControlsSessionPlaybackStatus::Opened => Status::Paused,
            _ => Status::Stopped,
        };
        let controls = playback.Controls().ok()?;
        let flag = |r: windows::core::Result<bool>| r.unwrap_or(false);
        let can_play_pause = flag(controls.IsPlayPauseToggleEnabled())
            || flag(controls.IsPlayEnabled())
            || flag(controls.IsPauseEnabled());
        let shuffle = if flag(controls.IsShuffleEnabled()) {
            playback.IsShuffleActive().ok().and_then(|v| v.Value().ok())
        } else {
            None
        };
        let repeat = if flag(controls.IsRepeatEnabled()) {
            playback.AutoRepeatMode().ok().and_then(|v| v.Value().ok()).map(|m| match m {
                MediaPlaybackAutoRepeatMode::Track => Repeat::One,
                MediaPlaybackAutoRepeatMode::List => Repeat::All,
                _ => Repeat::Off,
            })
        } else {
            None
        };

        // Timeline: work out where playback is right now.
        let (mut position, mut duration) = (0.0, 0.0);
        if let Ok(timeline) = session.GetTimelineProperties() {
            let ticks = |t: windows::core::Result<windows::Foundation::TimeSpan>| {
                t.map(|t| t.Duration as f64 / 1e7).unwrap_or(0.0)
            };
            let start = ticks(timeline.StartTime());
            duration = (ticks(timeline.EndTime()) - start).max(0.0);
            position = ticks(timeline.Position()) - start;
            if status == Status::Playing {
                if let Ok(updated) = timeline.LastUpdatedTime() {
                    let now = GetSystemTimeAsFileTime();
                    let now = ((now.dwHighDateTime as i64) << 32) | now.dwLowDateTime as i64;
                    let since = (now - updated.UniversalTime) as f64 / 1e7;
                    if (0.0..86_400.0).contains(&since) {
                        position += since;
                    }
                }
            }
            if duration > 0.0 {
                position = position.clamp(0.0, duration);
            }
        }

        // Album art: cached per song, but re-checked while young (see cover_swapped).
        let key = hash(&[&id, &title, &artist, &album]);
        if let Some(c) = self.art_cache.get_mut(&key) {
            rewatch(c, status);
        }
        let cached = self.art_cache.get(&key).map(|c| (c.art.clone(), c.size, c.at));
        let art = match cached {
            Some((art, _, at)) if at.elapsed() >= COVER_WATCH => Some(art),
            cached => {
                let thumb = open_thumb(&props);
                let now_size = thumb.as_ref().map(|(_, n)| *n);
                match cached {
                    Some((art, size, at)) if !cover_swapped(size, now_size, at.elapsed()) => Some(art),
                    cached => {
                        let fresh = thumb.and_then(|(stream, size)| Some((Arc::new(self.decode_art(&stream, key)?), size)));
                        match fresh {
                            Some((art, size)) => {
                                let at = cached.map(|(_, _, at)| at).unwrap_or_else(Instant::now);
                                self.art_cache.insert(key, Cached { art: art.clone(), size, at, status, app: id.clone() });
                                Some(art)
                            }
                            None => cached.map(|(art, _, _)| art), // keep what we had
                        }
                    }
                }
            }
        };

        Some(Session {
            app_name: app_name(&id),
            id,
            title,
            artist,
            status,
            can_play_pause,
            can_next: flag(controls.IsNextEnabled()),
            can_prev: flag(controls.IsPreviousEnabled()),
            can_seek: flag(controls.IsPlaybackPositionEnabled()) && duration > 0.0,
            shuffle,
            repeat,
            position,
            duration,
            sampled_at: Instant::now(),
            art,
        })
    }

    unsafe fn decode_art(
        &self,
        stream: &windows::Storage::Streams::IRandomAccessStreamWithContentType,
        key: u64,
    ) -> Option<Art> {
        let wic = self.wic.as_ref()?;
        let stream: IStream = CreateStreamOverRandomAccessStream(stream).ok()?;
        let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        // Centre-crop to a square (browser thumbnails are often 16:9).
        let side = w.min(h);
        let clipper = wic.CreateBitmapClipper().ok()?;
        let rect = WICRect { X: ((w - side) / 2) as i32, Y: ((h - side) / 2) as i32, Width: side as i32, Height: side as i32 };
        clipper.Initialize(&frame, &rect).ok()?;

        let resize = |size: u32| -> Option<Vec<u8>> {
            let scaler = wic.CreateBitmapScaler().ok()?;
            scaler.Initialize(&clipper, size, size, WICBitmapInterpolationModeHighQualityCubic).ok()?;
            let converter = wic.CreateFormatConverter().ok()?;
            converter
                .Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
                .ok()?;
            let mut pixels = vec![0u8; (size * size * 4) as usize];
            converter.CopyPixels(std::ptr::null(), size * 4, &mut pixels).ok()?;
            Some(pixels)
        };
        let big = resize(ART_BIG)?;
        let small = resize(ART_SMALL)?;
        let accent = accent_colour(&small);
        Some(Art { key, big, big_size: ART_BIG, small, small_size: ART_SMALL, accent })
    }

    /// Forward a button press to the app.
    fn run(&mut self, command: Command) {
        let id = match &command {
            Command::TogglePlayPause(id)
            | Command::Next(id)
            | Command::Previous(id)
            | Command::Seek(id, _)
            | Command::ToggleShuffle(id)
            | Command::CycleRepeat(id) => id.clone(),
        };
        // Ids may carry a "#n" suffix for a second session from the same app.
        let (base, index) = match id.rsplit_once('#') {
            Some((base, n)) => (base.to_string(), n.parse::<usize>().unwrap_or(0)),
            None => (id, 0),
        };
        let base = HSTRING::from(base);
        let Some(s) = self
            .sessions()
            .into_iter()
            .filter(|s| s.SourceAppUserModelId().map(|x| x == base).unwrap_or(false))
            .nth(index)
        else {
            return;
        };
        // Each call returns an async "did it work" result; wait for it so the
        // snapshot we publish next already reflects the change.
        let refused = match &command {
            Command::ToggleShuffle(_) => Some("shuffle"),
            Command::CycleRepeat(_) => Some("repeat"),
            _ => None,
        };
        let worked = match command {
            Command::TogglePlayPause(_) => s.TryTogglePlayPauseAsync().and_then(|op| op.join()),
            Command::Next(_) => s.TrySkipNextAsync().and_then(|op| op.join()),
            Command::Previous(_) => s.TrySkipPreviousAsync().and_then(|op| op.join()),
            Command::Seek(_, secs) => {
                let start = s.GetTimelineProperties().and_then(|t| t.StartTime()).map(|t| t.Duration).unwrap_or(0);
                s.TryChangePlaybackPositionAsync(start + (secs * 1e7) as i64).and_then(|op| op.join())
            }
            Command::ToggleShuffle(_) => {
                let on = s.GetPlaybackInfo().and_then(|p| p.IsShuffleActive()).and_then(|v| v.Value()).unwrap_or(false);
                s.TryChangeShuffleActiveAsync(!on).and_then(|op| op.join())
            }
            Command::CycleRepeat(_) => {
                let mode = s.GetPlaybackInfo().and_then(|p| p.AutoRepeatMode()).and_then(|v| v.Value());
                let next = match mode {
                    Ok(MediaPlaybackAutoRepeatMode::None) => MediaPlaybackAutoRepeatMode::List,
                    Ok(MediaPlaybackAutoRepeatMode::List) => MediaPlaybackAutoRepeatMode::Track,
                    _ => MediaPlaybackAutoRepeatMode::None,
                };
                s.TryChangeAutoRepeatModeAsync(next).and_then(|op| op.join())
            }
        };
        // Shuffle/repeat are only shown for apps that report them, but an app can still refuse.
        if let Some(what) = refused {
            if !worked.unwrap_or(false) {
                let app = s.SourceAppUserModelId().map(|id| app_name(&id.to_string())).unwrap_or_default();
                self.notice = Some(format!("{app} didn't change {what}"));
            }
        }
    }
}

/// Apple Music reports the artist as "Artist — Album". The design shows the artist alone.
fn clean_artist(artist: &str, album: &str) -> String {
    for dash in [" \u{2014} ", " \u{2013} ", " - "] {
        if !album.is_empty() {
            if let Some(name) = artist.strip_suffix(&format!("{dash}{album}")) {
                return name.to_string();
            }
        }
    }
    // Album unknown but the "Artist — Album" shape is there: keep the artist part.
    match artist.split_once(" \u{2014} ") {
        Some((name, _)) if !name.is_empty() => name.to_string(),
        _ => artist.to_string(),
    }
}

/// A friendly name for the app behind a session.
fn app_name(id: &str) -> String {
    let lower = id.to_lowercase();
    let known = [
        ("applemusic", "Apple Music"),
        ("itunes", "iTunes"),
        ("spotify", "Spotify"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
        ("firefox", "Firefox"),
        ("opera", "Opera"),
        ("brave", "Brave"),
        ("vivaldi", "Vivaldi"),
        ("zunemusic", "Media Player"),
        ("zunevideo", "Films & TV"),
        ("vlc", "VLC"),
        ("tidal", "TIDAL"),
        ("deezer", "Deezer"),
        ("amazonmusic", "Amazon Music"),
        ("youtubemusic", "YouTube Music"),
        ("foobar2000", "foobar2000"),
        ("musicbee", "MusicBee"),
        ("discord", "Discord"),
    ];
    if let Some((_, name)) = known.iter().find(|(needle, _)| lower.contains(needle)) {
        return name.to_string();
    }
    // Fallback: "Company.AppName_hash!App" or "C:\...\app.exe" -> "AppName" / "app"
    let mut name = id.rsplit(['\\', '/']).next().unwrap_or(id);
    if let Some((package, _)) = name.split_once('!') {
        name = package.split('_').next().unwrap_or(package); // Store app: drop the "_hash" part
    }
    name = name.trim_end_matches(".exe").trim_end_matches(".EXE");
    name = name.rsplit('.').next().unwrap_or(name);
    if name.is_empty() { "Media".into() } else { name.to_string() }
}

fn hash(parts: &[&str]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

/// Pick a lively colour from the artwork: average the most colourful pixels, then make
/// sure it's bright enough to read on black.
fn accent_colour(pixels: &[u8]) -> [f32; 3] {
    let (mut r, mut g, mut b, mut weight) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for px in pixels.as_chunks::<4>().0 {
        let (pb, pg, pr) = (px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0);
        let max = pr.max(pg).max(pb);
        let min = pr.min(pg).min(pb);
        let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
        let w = saturation * saturation * max;
        r += pr * w;
        g += pg * w;
        b += pb * w;
        weight += w;
    }
    if weight < 1.0 {
        return [0.95, 0.65, 0.25]; // mostly grey art: warm amber
    }
    let (r, g, b) = (r / weight, g / weight, b / weight);
    let max = r.max(g).max(b).max(1e-3);
    let lift = (0.85 / max).max(1.0); // brighten dark colours
    [(r * lift).min(1.0), (g * lift).min(1.0), (b * lift).min(1.0)]
}

#[cfg(test)]
#[path = "tests/media.rs"]
mod tests;
