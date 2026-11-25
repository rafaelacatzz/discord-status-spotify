#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const APP_NAME: &str = "SpotifySync";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const LEGACY_CONFIG_DIR: &str = "discord-lyrics-status";
const DISCORD_SETTINGS_URL: &str = "https://discord.com/api/v9/users/@me/settings";
const DISCORD_ME_URL: &str = "https://discord.com/api/v9/users/@me";
/// Public GitHub releases of this app, used by the auto-updater.
const UPDATE_REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const UPDATE_ASSET_NAME: &str = "SpotifySync.exe";
const UPDATE_FILE_NAME: &str = "SpotifySync.update.exe";
const UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// `CREATE_NO_WINDOW`: the updater helper must not flash a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DEFAULT_SYNC_LEAD_MS: u16 = 500;
const BLACK: egui::Color32 = egui::Color32::from_rgb(0, 0, 0);
const BG: egui::Color32 = egui::Color32::from_rgb(18, 18, 18);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(24, 24, 24);
const SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(40, 40, 40);
const INSET: egui::Color32 = egui::Color32::from_rgb(12, 12, 12);
const BORDER: egui::Color32 = egui::Color32::from_rgb(48, 48, 48);
const BORDER_SOFT: egui::Color32 = egui::Color32::from_rgb(36, 36, 36);
const RAIL: egui::Color32 = egui::Color32::from_rgb(77, 77, 77);
const TEXT: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(210, 210, 210);
const MUTED: egui::Color32 = egui::Color32::from_rgb(164, 164, 164);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(30, 215, 96);
const ACCENT_SOFT: egui::Color32 = egui::Color32::from_rgb(30, 70, 45);
const WARNING: egui::Color32 = egui::Color32::from_rgb(240, 180, 90);
const DANGER: egui::Color32 = egui::Color32::from_rgb(240, 110, 110);
const RADIUS_MD: u8 = 8;
const RADIUS_SM: u8 = 4;
const PLAYER_BAR_HEIGHT: f32 = 90.0;
const EMOTE_PRESETS: [&str; 5] = ["🎵", "🎧", "🎤", "🎶", "🎸"];
const DEFAULT_EMOTE: &str = "🎵";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Track {
    title: String,
    artist: String,
    album: String,
    is_playing: bool,
    position_ms: u64,
    duration_ms: u64,
    last_updated_ms: u64,
    platform: String,
}

impl Track {
    /// Stable identity used to detect track changes. Duration is intentionally
    /// left out: Spotify sometimes reports it a second off (or zero for a
    /// moment), which used to look like a new track and reset the lyrics.
    fn id(&self) -> String {
        format!(
            "{}::{}",
            self.title.trim().to_lowercase(),
            self.artist.trim().to_lowercase()
        )
    }

    fn estimated_position(&self) -> u64 {
        if self.is_playing && self.last_updated_ms > 0 {
            self.position_ms
                .saturating_add(now_ms().saturating_sub(self.last_updated_ms))
                .min(self.duration_ms.max(self.position_ms))
        } else {
            self.position_ms
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
struct HistoryEntry {
    title: String,
    artist: String,
    album: String,
    duration_ms: u64,
    played_at: u64,
    /// File name of the cached album cover inside the app's `covers` folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cover: Option<String>,
}

const MAX_HISTORY_ENTRIES: usize = 50;

fn record_history(history: &mut Vec<HistoryEntry>, track: &Track, played_at: u64) {
    let title = track.title.trim();
    if title.is_empty() {
        return;
    }
    let artist = track.artist.trim();
    // A track played again keeps a single row: the previous one is pulled out
    // and re-inserted on top with the fresh play time, so the list never shows
    // the same song twice and the cached cover is not lost.
    let previous = history
        .iter()
        .position(|entry| {
            entry.title.eq_ignore_ascii_case(title) && entry.artist.eq_ignore_ascii_case(artist)
        })
        .map(|index| history.remove(index));
    let (album, duration_ms, cover) = match previous {
        Some(entry) => (
            if track.album.trim().is_empty() {
                entry.album
            } else {
                track.album.trim().to_owned()
            },
            if track.duration_ms == 0 {
                entry.duration_ms
            } else {
                track.duration_ms
            },
            entry.cover,
        ),
        None => (track.album.trim().to_owned(), track.duration_ms, None),
    };
    history.insert(
        0,
        HistoryEntry {
            title: title.to_owned(),
            artist: artist.to_owned(),
            album,
            duration_ms,
            played_at,
            cover,
        },
    );
    history.truncate(MAX_HISTORY_ENTRIES);
}

fn favorite_artist(history: &[HistoryEntry]) -> Option<String> {
    let mut artists: HashMap<String, (usize, String)> = HashMap::new();
    for entry in history {
        let name = entry.artist.trim();
        if name.is_empty() {
            continue;
        }
        let slot = artists
            .entry(name.to_lowercase())
            .or_insert((0, name.to_owned()));
        slot.0 += 1;
        if name < slot.1.as_str() {
            slot.1 = name.to_owned();
        }
    }

    artists
        .values()
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.to_lowercase().cmp(&a.1.to_lowercase())))
        .map(|(_, name)| name.clone())
}

/// Older versions of the app stored one row per playback. This collapses those
/// files to a single row per track, keeping the most recent play and merging the
/// covers that were already downloaded.
fn dedupe_history(history: &mut Vec<HistoryEntry>) {
    let mut index = 0;
    while index < history.len() {
        let mut other = index + 1;
        while other < history.len() {
            let same_track = history[other]
                .title
                .eq_ignore_ascii_case(&history[index].title)
                && history[other]
                    .artist
                    .eq_ignore_ascii_case(&history[index].artist);
            if same_track {
                let removed = history.remove(other);
                if history[index].cover.is_none() {
                    history[index].cover = removed.cover;
                }
            } else {
                other += 1;
            }
        }
        index += 1;
    }
}

/// Folder where album covers are cached, next to the config file.
fn cover_cache_dir() -> Option<PathBuf> {
    Some(config_dir()?.join("covers"))
}

/// Saves the downloaded artwork so the history keeps showing the real cover
/// after a restart. The file is content addressed by title + artist, so the
/// same song never writes twice.
fn cache_history_cover(title: &str, artist: &str, bytes: &[u8]) -> Option<String> {
    let dir = cover_cache_dir()?;
    fs::create_dir_all(&dir).ok()?;
    let name = format!(
        "{:016x}.img",
        stable_hash(format!("{title}::{artist}").to_lowercase().as_bytes())
    );
    let path = dir.join(&name);
    if !path.exists() {
        fs::write(&path, bytes).ok()?;
    }
    Some(name)
}

/// Points the history entry for `title` / `artist` at a cached cover.
fn attach_cover(history: &mut [HistoryEntry], title: &str, artist: &str, name: &str) -> bool {
    for entry in history.iter_mut() {
        if entry.title.eq_ignore_ascii_case(title.trim())
            && entry.artist.eq_ignore_ascii_case(artist.trim())
        {
            if entry.cover.as_deref() == Some(name) {
                return false;
            }
            entry.cover = Some(name.to_owned());
            return true;
        }
    }
    false
}

fn relative_played_at(played_at: u64, now: u64) -> String {
    let elapsed = now.saturating_sub(played_at);
    if elapsed < 60 {
        "Agora".to_owned()
    } else if elapsed < 3_600 {
        format!("há {} min", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("há {} h", elapsed / 3_600)
    } else if elapsed < 172_800 {
        "há 1 dia".to_owned()
    } else {
        format!("há {} dias", elapsed / 86_400)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct LyricLine {
    time_ms: u64,
    text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AuthState {
    #[default]
    SignedOut,
    Validating,
    SignedIn,
    Failed,
}

#[derive(Clone, Debug, Default)]
struct Account {
    id: String,
    name: String,
    avatar_url: Option<String>,
}

/// Splits an emote setting into the `emoji_name` / `emoji_id` pair Discord
/// expects. Accepts a unicode emoji, `nome:id` or `<:nome:id>` / `<a:nome:id>`.
fn parse_emoji(raw: &str) -> (Option<String>, Option<String>) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (None, None);
    }
    let inner = trimmed.trim_start_matches('<').trim_end_matches('>');
    let parts: Vec<&str> = inner.split(':').collect();
    if parts.len() >= 2 {
        let id = parts[parts.len() - 1];
        let name = parts[parts.len() - 2];
        if id.len() >= 5 && id.chars().all(|value| value.is_ascii_digit()) && !name.is_empty() {
            return (Some(name.to_owned()), Some(id.to_owned()));
        }
    }
    (Some(trimmed.to_owned()), None)
}

#[derive(Clone, Debug, Default)]
struct ViewState {
    track: Option<Track>,
    recent_tracks: Vec<HistoryEntry>,
    lyrics: Vec<LyricLine>,
    lyric_index: Option<usize>,
    auto_bpm: Option<u16>,
    auto_lead_ms: Option<u16>,
    artwork: Option<Vec<u8>>,
    discord_status: String,
    lyrics_status: String,
    token_status: String,
    account: Option<Account>,
    avatar: Option<Vec<u8>>,
    emote_lyric: Option<Vec<u8>>,
    emote_track: Option<Vec<u8>>,
    auth: AuthState,
    last_error: Option<String>,
    update: UpdateStatus,
}

/// Progress of the background self-updater, shown in Settings > Sobre.
#[derive(Clone, Debug)]
struct UpdateStatus {
    message: String,
    ready: bool,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            message: "Ainda não verificado".to_owned(),
            ready: false,
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct Config {
    /// Legacy plaintext token, read from old configuration files and migrated to
    /// the encrypted local store. It is never written again.
    #[serde(rename = "userToken", skip_serializing)]
    user_token: Option<String>,
    #[serde(rename = "syncLeadMs")]
    sync_lead_ms: Option<u16>,
    #[serde(rename = "presenceStatus")]
    presence_status: Option<String>,
    #[serde(rename = "trackEmoji")]
    track_emoji: Option<String>,
    #[serde(rename = "lyricsEmoji")]
    lyrics_emoji: Option<String>,
    #[serde(rename = "ecoMode")]
    eco_mode: Option<bool>,
    #[serde(rename = "statusMode")]
    status_mode: Option<String>,
    #[serde(rename = "statusEmoji")]
    status_emoji: Option<String>,
    #[serde(rename = "showTimestamps")]
    show_timestamps: Option<bool>,
    #[serde(rename = "syncDiscordStatus")]
    sync_discord_status: Option<bool>,
    #[serde(rename = "syncLeagueStatus")]
    sync_league_status: Option<bool>,
    #[serde(rename = "autoUpdate")]
    auto_update: Option<bool>,
    /// Optional public feed with the newest release, for repositories that are
    /// not public: a GitHub release JSON or `{ "version", "url" }`.
    #[serde(rename = "updateFeed")]
    update_feed: Option<String>,
    #[serde(rename = "accountId")]
    account_id: Option<String>,
    #[serde(rename = "accountName")]
    account_name: Option<String>,
    #[serde(rename = "accountAvatar")]
    account_avatar: Option<String>,
    #[serde(rename = "recentTracks", default)]
    recent_tracks: Vec<HistoryEntry>,
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

fn normalize_presence(value: Option<&str>) -> String {
    match value {
        Some("idle") => "idle",
        Some("dnd") => "dnd",
        Some("invisible") => "invisible",
        _ => "online",
    }
    .to_owned()
}

fn presence_label(value: &str) -> &'static str {
    match value {
        "idle" => "Ausente",
        "dnd" => "Não perturbe",
        "invisible" => "Invisível",
        _ => "Online",
    }
}

/// The status always follows the music automatically: a synced lyric line when
/// the current track has one, title — artist otherwise.
fn has_active_lyric(state: &ViewState) -> bool {
    state
        .lyric_index
        .and_then(|index| state.lyrics.get(index))
        .map(|line| !line.text.trim().is_empty())
        .unwrap_or(false)
}

/// Tolerance between the interpolated clock and Spotify's own timeline. SMTC
/// only refreshes that timeline every few seconds, so a read can sit slightly
/// behind the interpolated clock; that is normal lag and must not rewind the
/// status.
const POSITION_DRIFT_TOLERANCE_MS: u64 = 1_500;
/// How far Spotify's own position must move for it to be a real seek instead of
/// a stale or frozen timeline read.
const SEEK_JUMP_MS: u64 = 1_500;

/// Spotify reports its position in bursts, so reading it once per tick makes
/// the lyric line lag behind. The clock keeps interpolating between reads and
/// re-anchors when Spotify catches up, drifts away (pause/buffer) or jumps.
/// Returns `(position, reanchor, jumped)`: only `jumped` means a real seek, so
/// a late read is never allowed to move the lyrics backwards.
fn advance_position(
    anchor_ms: u64,
    elapsed_ms: u64,
    raw_ms: u64,
    last_raw_ms: u64,
) -> (u64, bool, bool) {
    let projected = anchor_ms.saturating_add(elapsed_ms);
    let drift = raw_ms.abs_diff(projected);
    if drift <= POSITION_DRIFT_TOLERANCE_MS {
        if raw_ms >= projected {
            return (raw_ms, true, false);
        }
        // Normal SMTC lag: keep interpolating instead of stepping back.
        return (projected, false, false);
    }
    if raw_ms == last_raw_ms {
        // The timeline did not advance at all, so the read is stale.
        return (projected, false, false);
    }
    (raw_ms, true, raw_ms.abs_diff(last_raw_ms) > SEEK_JUMP_MS)
}

#[derive(Clone, Debug)]
struct PlaybackClock {
    anchor_ms: u64,
    anchor_at: Instant,
    known: bool,
    last_raw_ms: u64,
}

impl PlaybackClock {
    fn new() -> Self {
        Self {
            anchor_ms: 0,
            anchor_at: Instant::now(),
            known: false,
            last_raw_ms: 0,
        }
    }

    fn reset(&mut self) {
        self.known = false;
    }

    /// Interpolated position plus whether the track really jumped (a seek),
    /// which is the only case where the lyrics may move backwards.
    fn position(&mut self, raw_ms: u64, playing: bool) -> (u64, bool) {
        if !self.known {
            self.known = true;
            self.anchor_ms = raw_ms;
            self.anchor_at = Instant::now();
            self.last_raw_ms = raw_ms;
            return (raw_ms, false);
        }
        let elapsed = if playing {
            self.anchor_at.elapsed().as_millis() as u64
        } else {
            0
        };
        let (value, reanchor, jumped) =
            advance_position(self.anchor_ms, elapsed, raw_ms, self.last_raw_ms);
        self.last_raw_ms = raw_ms;
        if reanchor {
            self.anchor_ms = raw_ms;
            self.anchor_at = Instant::now();
        }
        (value, jumped)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Settings {
    sync_lead_ms: u16,
    presence: String,
    eco_mode: bool,
    track_emoji: String,
    lyric_emoji: String,
    show_timestamps: bool,
    sync_discord_status: bool,
    sync_league_status: bool,
    auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sync_lead_ms: DEFAULT_SYNC_LEAD_MS,
            presence: "online".to_owned(),
            eco_mode: false,
            track_emoji: DEFAULT_EMOTE.to_owned(),
            lyric_emoji: DEFAULT_EMOTE.to_owned(),
            show_timestamps: true,
            sync_discord_status: true,
            sync_league_status: true,
            auto_update: true,
        }
    }
}

impl Settings {
    fn from_config(config: &Config) -> Self {
        let legacy = config.status_emoji.clone();
        Self {
            sync_lead_ms: config
                .sync_lead_ms
                .filter(|value| *value <= 1_500)
                .unwrap_or(DEFAULT_SYNC_LEAD_MS),
            presence: normalize_presence(config.presence_status.as_deref()),
            eco_mode: config.eco_mode.unwrap_or(false),
            track_emoji: config
                .track_emoji
                .clone()
                .or_else(|| legacy.clone())
                .unwrap_or_else(|| DEFAULT_EMOTE.to_owned()),
            lyric_emoji: config
                .lyrics_emoji
                .clone()
                .or_else(|| legacy.clone())
                .unwrap_or_else(|| DEFAULT_EMOTE.to_owned()),
            show_timestamps: config.show_timestamps.unwrap_or(true),
            sync_discord_status: config.sync_discord_status.unwrap_or(true),
            sync_league_status: config.sync_league_status.unwrap_or(true),
            auto_update: config.auto_update.unwrap_or(true),
        }
    }

    fn emoji_for_lyric(&self, has_lyric: bool) -> &str {
        if has_lyric {
            &self.lyric_emoji
        } else {
            &self.track_emoji
        }
    }

    fn apply(&self, config: &mut Config) {
        config.sync_lead_ms = Some(self.sync_lead_ms);
        config.presence_status = Some(self.presence.clone());
        config.eco_mode = Some(self.eco_mode);
        // Kept for configuration compatibility with older versions.
        config.status_mode = Some("auto".to_owned());
        config.track_emoji = Some(self.track_emoji.clone());
        config.lyrics_emoji = Some(self.lyric_emoji.clone());
        config.status_emoji = None;
        config.show_timestamps = Some(self.show_timestamps);
        config.sync_discord_status = Some(self.sync_discord_status);
        config.sync_league_status = Some(self.sync_league_status);
        config.auto_update = Some(self.auto_update);
    }
}

enum Command {
    SaveToken(String),
    SignOut,
    Seek(u64),
    Media(MediaAction),
    UpdateSettings(Settings),
}

#[derive(Clone, Copy)]
enum MediaAction {
    TogglePlayPause,
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Player,
    History,
    Lyrics,
    Settings,
}

struct NativeApp {
    state: Arc<Mutex<ViewState>>,
    command_tx: mpsc::Sender<Command>,
    page: Page,
    last_page: Page,
    search_query: String,
    search_active: bool,
    history_filter: String,
    settings_tab: usize,
    history_covers: HashMap<String, Option<egui::TextureHandle>>,
    fullscreen: bool,
    token_input: String,
    show_token: bool,
    settings: Settings,
    logo_texture: egui::TextureHandle,
    texture: Option<egui::TextureHandle>,
    texture_hash: u64,
    art_tint: egui::Color32,
    art_tint_target: egui::Color32,
    art_tint_from: egui::Color32,
    theme_accent: egui::Color32,
    theme_target: egui::Color32,
    theme_from: egui::Color32,
    theme_mix: f32,
    avatar_texture: Option<egui::TextureHandle>,
    avatar_hash: u64,
    emote_lyric_texture: Option<egui::TextureHandle>,
    emote_lyric_hash: u64,
    emote_track_texture: Option<egui::TextureHandle>,
    emote_track_hash: u64,
    last_sent_settings: Settings,
    saved_flash: Option<Instant>,
    last_scrolled_lyric: Option<usize>,
    ui_error: Option<String>,
    update_check: Arc<AtomicBool>,
}

impl NativeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_visuals(&cc.egui_ctx);
        let mut config = load_config();
        migrate_legacy_token(&mut config);
        let token = configured_token().or_else(load_local_token);
        let settings = Settings::from_config(&config);
        let mut recent_tracks = config.recent_tracks.clone();
        recent_tracks.truncate(MAX_HISTORY_ENTRIES);
        dedupe_history(&mut recent_tracks);
        let account = match (
            token.as_ref(),
            config.account_id.clone(),
            config.account_name.clone(),
        ) {
            (Some(_), Some(id), Some(name)) => Some(Account {
                id,
                name,
                avatar_url: config.account_avatar.clone(),
            }),
            _ => None,
        };
        let auth = if account.is_some() {
            AuthState::SignedIn
        } else if token.is_some() {
            AuthState::Validating
        } else {
            AuthState::SignedOut
        };
        let state = Arc::new(Mutex::new(ViewState {
            discord_status: if token.is_some() {
                "checking".to_owned()
            } else {
                "no-token".to_owned()
            },
            lyrics_status: "Aguardando música".to_owned(),
            token_status: match (&account, token.is_some()) {
                (Some(account), _) => format!("Sessão de {} restaurada", account.name),
                (None, true) => "Verificando token salvo…".to_owned(),
                (None, false) => "Nenhuma conta conectada".to_owned(),
            },
            account,
            recent_tracks,
            auth,
            ..ViewState::default()
        }));
        let (command_tx, command_rx) = mpsc::channel();
        let logo = image::load_from_memory(include_bytes!("../../build/icon.png"))
            .expect("Failed to load the SpotifySync brand icon")
            .to_rgba8();
        let logo_size = [logo.width() as usize, logo.height() as usize];
        let logo_image = egui::ColorImage::from_rgba_unmultiplied(logo_size, logo.as_raw());
        let logo_texture =
            cc.egui_ctx
                .load_texture("brand-logo", logo_image, egui::TextureOptions::LINEAR);
        let update_check = Arc::new(AtomicBool::new(false));
        spawn_worker(
            state.clone(),
            command_rx,
            config,
            token,
            settings.clone(),
            update_check.clone(),
        );

        Self {
            state,
            command_tx,
            page: Page::Player,
            last_page: Page::Player,
            search_query: String::new(),
            search_active: false,
            history_filter: String::new(),
            settings_tab: 0,
            history_covers: HashMap::new(),
            fullscreen: false,
            token_input: String::new(),
            show_token: false,
            last_sent_settings: settings.clone(),
            settings,
            logo_texture,
            texture: None,
            texture_hash: 0,
            art_tint: theme_tint(ACCENT),
            art_tint_target: theme_tint(ACCENT),
            art_tint_from: theme_tint(ACCENT),
            theme_accent: ACCENT,
            theme_target: ACCENT,
            theme_from: ACCENT,
            theme_mix: 1.0,
            avatar_texture: None,
            avatar_hash: 0,
            emote_lyric_texture: None,
            emote_lyric_hash: 0,
            emote_track_texture: None,
            emote_track_hash: 0,
            saved_flash: None,
            last_scrolled_lyric: None,
            ui_error: None,
            update_check,
        }
    }

    fn submit_token(&mut self) {
        let candidate = self.token_input.trim().to_owned();
        if candidate.len() < 20 {
            self.ui_error = Some("Informe um token válido do Discord.".to_owned());
            return;
        }
        match self.command_tx.send(Command::SaveToken(candidate)) {
            Ok(()) => {
                // Do not keep the token in the input buffer after handing it over.
                wipe_secret(&mut self.token_input);
                self.ui_error = None;
            }
            Err(error) => {
                self.ui_error = Some(format!("Não foi possível iniciar a validação: {error}"));
            }
        }
    }

    fn sign_out(&mut self) {
        match self.command_tx.send(Command::SignOut) {
            Ok(()) => {
                self.ui_error = None;
                self.page = Page::Player;
            }
            Err(_) => self.ui_error = Some("Não foi possível encerrar a sessão.".to_owned()),
        }
    }

    fn queue_seek(&mut self, position_ms: u64) {
        if let Err(error) = self.command_tx.send(Command::Seek(position_ms)) {
            self.ui_error = Some(format!("Falha ao buscar faixa: {error}"));
        }
    }

    fn queue_media_command(&mut self, action: MediaAction) {
        if let Err(error) = self.command_tx.send(Command::Media(action)) {
            self.ui_error = Some(format!("Falha ao controlar o Spotify: {error}"));
        }
    }

    fn flush_settings(&mut self) {
        if self.settings != self.last_sent_settings {
            match self
                .command_tx
                .send(Command::UpdateSettings(self.settings.clone()))
            {
                Ok(()) => {
                    self.last_sent_settings = self.settings.clone();
                    self.saved_flash = Some(Instant::now());
                    self.ui_error = None;
                }
                Err(error) => {
                    self.ui_error = Some(format!("Falha ao salvar configurações: {error}"));
                }
            }
        }
    }

    fn sync_avatar_texture(&mut self, ctx: &egui::Context, state: &ViewState) {
        let hash = state.avatar.as_deref().map(stable_hash).unwrap_or(0);
        if hash != self.avatar_hash {
            self.avatar_texture = state
                .avatar
                .as_deref()
                .and_then(|bytes| load_texture(ctx, "account-avatar", bytes));
            self.avatar_hash = hash;
        }
    }

    fn sync_artwork_texture(&mut self, ctx: &egui::Context, state: &ViewState) {
        let artwork_hash = state.artwork.as_deref().map(stable_hash).unwrap_or(0);
        if artwork_hash == self.texture_hash {
            return;
        }

        self.texture = None;
        let mut palette = CoverPalette::fallback();
        if let Some(bytes) = &state.artwork {
            if let Ok(image) = image::load_from_memory(bytes) {
                let rgba = image.to_rgba8();
                palette = artwork_palette(&rgba);
                let size = [rgba.width() as usize, rgba.height() as usize];
                let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                self.texture =
                    Some(ctx.load_texture("album-art", color_image, egui::TextureOptions::LINEAR));
            }
        }
        if palette.accent != self.theme_target || palette.tint != self.art_tint_target {
            // Start a crossfade from whatever is on screen right now, so changing
            // tracks never snaps the whole window to a different colour.
            self.theme_from = self.theme_accent;
            self.art_tint_from = self.art_tint;
            self.theme_mix = 0.0;
            self.theme_target = palette.accent;
            self.art_tint_target = palette.tint;
            ctx.request_repaint();
        }
        self.texture_hash = artwork_hash;
    }

    /// Eases the palette towards the cover's colours instead of switching them
    /// in one frame.
    fn animate_theme(&mut self, ctx: &egui::Context) {
        if self.theme_mix >= 1.0 {
            return;
        }
        let dt = ctx.input(|input| input.stable_dt).clamp(0.0, 0.1);
        self.theme_mix = (self.theme_mix + dt * 5.5).min(1.0);
        let eased = 1.0 - (1.0 - self.theme_mix).powi(3);
        self.theme_accent = blend(self.theme_from, self.theme_target, eased);
        self.art_tint = blend(self.art_tint_from, self.art_tint_target, eased);
        apply_theme_visuals(ctx, self.theme_accent);
        if self.theme_mix < 1.0 {
            ctx.request_repaint();
        }
    }

    /// Loads (once) the cached cover of a history entry.
    fn history_cover(
        &mut self,
        ctx: &egui::Context,
        entry: &HistoryEntry,
    ) -> Option<egui::TextureHandle> {
        let name = entry.cover.as_deref().filter(|name| !name.is_empty())?;
        if let Some(cached) = self.history_covers.get(name) {
            return cached.clone();
        }
        let loaded = cover_cache_dir()
            .map(|dir| dir.join(name))
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| load_texture(ctx, &format!("history-cover-{name}"), &bytes));
        self.history_covers.insert(name.to_owned(), loaded.clone());
        loaded
    }

    fn sync_emote_textures(&mut self, ctx: &egui::Context, state: &ViewState) {
        let lyric_hash = state.emote_lyric.as_deref().map(stable_hash).unwrap_or(0);
        if lyric_hash != self.emote_lyric_hash {
            self.emote_lyric_texture = state
                .emote_lyric
                .as_deref()
                .and_then(|bytes| load_texture(ctx, "emote-lyric", bytes));
            self.emote_lyric_hash = lyric_hash;
        }
        let track_hash = state.emote_track.as_deref().map(stable_hash).unwrap_or(0);
        if track_hash != self.emote_track_hash {
            self.emote_track_texture = state
                .emote_track
                .as_deref()
                .and_then(|bytes| load_texture(ctx, "emote-track", bytes));
            self.emote_track_hash = track_hash;
        }
    }

    /// Global shortcuts, ignored while a text field has the keyboard.
    fn handle_shortcuts(&mut self, ctx: &egui::Context, state: &ViewState) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let pressed = |key| ctx.input(|input| input.key_pressed(key));
        let control = ctx.input(|input| input.modifiers.ctrl);

        if control && pressed(egui::Key::F) {
            self.page = Page::Player;
            self.search_active = true;
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("spotify-search")));
        } else if pressed(egui::Key::F) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if pressed(egui::Key::Escape) && self.search_active {
            self.search_query.clear();
            self.search_active = false;
        }
        if pressed(egui::Key::L) {
            self.page = if self.page == Page::Lyrics {
                Page::Player
            } else {
                Page::Lyrics
            };
            self.search_active = false;
        }
        if pressed(egui::Key::H) {
            self.page = if self.page == Page::History {
                Page::Player
            } else {
                Page::History
            };
            self.search_active = false;
        }
        if let Some(track) = state.track.as_ref() {
            if pressed(egui::Key::Space) {
                self.queue_media_command(MediaAction::TogglePlayPause);
            }
            let position = track.estimated_position().min(track.duration_ms);
            if pressed(egui::Key::ArrowLeft) {
                self.queue_seek(position.saturating_sub(SEEK_JUMP_MS as u64));
            }
            if pressed(egui::Key::ArrowRight) {
                self.queue_seek((position + SEEK_JUMP_MS as u64).min(track.duration_ms));
            }
        }
    }

    fn draw_player_bar(&mut self, ctx: &egui::Context, state: &ViewState) {
        let width = ctx.content_rect().width();
        let artwork_size = if width < 760.0 { 48.0 } else { 56.0 };
        egui::TopBottomPanel::bottom("player-bar")
            .exact_height(PLAYER_BAR_HEIGHT)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(BLACK)
                    .inner_margin(egui::Margin::symmetric(16, 0))
                    .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(40, 40, 40))),
            )
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                let (left_width, center_width, right_width) =
                    player_bar_column_widths(bar.width());
                let left_rect =
                    egui::Rect::from_min_size(bar.min, egui::vec2(left_width, bar.height()));
                let center_rect = egui::Rect::from_min_size(
                    egui::pos2(bar.left() + left_width, bar.top()),
                    egui::vec2(center_width, bar.height()),
                );
                let right_rect = egui::Rect::from_min_size(
                    egui::pos2(bar.right() - right_width, bar.top()),
                    egui::vec2(right_width, bar.height()),
                );

                // Track block: artwork, title/artist and like button, all
                // vertically centered like Spotify's now-playing block.
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(left_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if let Some(texture) = &self.texture {
                            ui.add(
                                egui::Image::new(texture)
                                    .fit_to_exact_size(egui::vec2(artwork_size, artwork_size))
                                    .corner_radius(egui::CornerRadius::same(4)),
                            );
                        } else {
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(artwork_size, artwork_size),
                                egui::Sense::hover(),
                            );
                            paint_gradient_rect(
                                ui.painter(),
                                rect,
                                self.theme_accent,
                                blend(self.theme_accent, BG, 0.38),
                                4,
                            );
                            paint_note_at(ui.painter(), rect.center(), 26.0, TEXT);
                        }
                        ui.add_space(14.0);
                        let text_width = (left_width - artwork_size - 20.0).max(48.0);
                        let (title, artist) = match &state.track {
                            Some(track) => (track.title.as_str(), track.artist.as_str()),
                            None => ("Nenhuma música", "—"),
                        };
                        let title_chars = ((text_width / 7.0) as usize).max(6);
                        let artist_chars = ((text_width / 5.4) as usize).max(8);
                        ui.allocate_ui_with_layout(
                            egui::vec2(text_width, 36.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(
                                    egui::RichText::new(truncate(title, title_chars))
                                        .size(13.0)
                                        .strong()
                                        .color(TEXT),
                                );
                                ui.label(
                                    egui::RichText::new(truncate(artist, artist_chars))
                                        .size(11.0)
                                        .color(MUTED),
                                );
                            },
                        );
                    },
                );

                // Transport controls with the seek row directly underneath.
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(center_rect)
                        .layout(egui::Layout::top_down(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                        let controls_height = 32.0;
                        let progress_height = 18.0;
                        let row_gap = 8.0;
                        let content_height = controls_height + row_gap + progress_height;
                        ui.add_space(((center_rect.height() - content_height) * 0.5).max(0.0));

                        let has_track = state.track.is_some();
                        let playing = state.track.as_ref().is_some_and(|track| track.is_playing);
                        let controls_width = 32.0 * 3.0 + 8.0 * 2.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2(controls_width, controls_height),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                if icon_button(
                                    ui,
                                    Glyph::Prev,
                                    32.0,
                                    false,
                                    false,
                                    "Anterior",
                                    has_track,
                                ) {
                                    self.queue_media_command(MediaAction::Previous);
                                }
                                if icon_button(
                                    ui,
                                    if playing { Glyph::Pause } else { Glyph::Play },
                                    32.0,
                                    false,
                                    true,
                                    if playing { "Pausar" } else { "Reproduzir" },
                                    has_track,
                                ) {
                                    self.queue_media_command(MediaAction::TogglePlayPause);
                                }
                                if icon_button(
                                    ui,
                                    Glyph::Next,
                                    32.0,
                                    false,
                                    false,
                                    "Próxima",
                                    has_track,
                                ) {
                                    self.queue_media_command(MediaAction::Next);
                                }
                            },
                        );
                        ui.add_space(row_gap);

                        let (position, duration) = state
                            .track
                            .as_ref()
                            .map(|track| {
                                (
                                    track.estimated_position().min(track.duration_ms),
                                    track.duration_ms,
                                )
                            })
                            .unwrap_or((0, 0));
                        let time_width = 40.0;
                        let seek_width =
                            (center_width - time_width * 2.0 - 12.0).clamp(120.0, 520.0);
                        let row_width = time_width * 2.0 + seek_width + 12.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2(row_width, progress_height),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                time_label(ui, time_width, &format_time(position), true);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(seek_width, progress_height),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        if let Some(target) = seek_bar(ui, position, duration) {
                                            if let Err(error) =
                                                self.command_tx.send(Command::Seek(target))
                                            {
                                                self.ui_error = Some(format!(
                                                    "Falha ao buscar faixa: {error}"
                                                ));
                                            }
                                        }
                                    },
                                );
                                time_label(ui, time_width, &format_time(duration), false);
                            },
                        );
                    },
                );

                // Right side: only the destinations that really exist.
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(right_rect)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if icon_button(
                            ui,
                            Glyph::Fullscreen,
                            32.0,
                            false,
                            false,
                            "Tela cheia",
                            true,
                        ) {
                            self.fullscreen = !self.fullscreen;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                                self.fullscreen,
                            ));
                        }
                        ui.add_space(6.0);
                        if icon_button(
                            ui,
                            Glyph::Settings,
                            32.0,
                            self.page == Page::Settings,
                            false,
                            "Configurações",
                            true,
                        ) {
                            self.page = if self.page == Page::Settings {
                                Page::Player
                            } else {
                                Page::Settings
                            };
                            self.search_active = false;
                        }
                        ui.add_space(6.0);
                        if icon_button(
                            ui,
                            Glyph::Lyrics,
                            32.0,
                            self.page == Page::Lyrics,
                            false,
                            "Letras",
                            true,
                        ) {
                            self.page = Page::Lyrics;
                            self.search_active = false;
                        }
                    },
                );
            });
    }

    fn draw_sidebar(&mut self, ctx: &egui::Context, state: &ViewState) {
        egui::SidePanel::left("navigation")
            .exact_width(292.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(BLACK).inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 8,
                bottom: 8,
            }))
            .show(ctx, |ui| {
                let panel_rect = ui.max_rect();
                ui.set_clip_rect(panel_rect);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                // Navigation card: brand plus only the destinations that exist.
                let navigation = egui::Frame::new()
                    .fill(BG)
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin {
                        left: 8,
                        right: 8,
                        top: 10,
                        bottom: 10,
                    })
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(24.0, 24.0),
                                egui::Sense::hover(),
                            );
                            paint_gradient_rect(
                                ui.painter(),
                                rect,
                                self.theme_accent,
                                blend(self.theme_accent, BG, 0.35),
                                6,
                            );
                            paint_note_at(
                                ui.painter(),
                                rect.center(),
                                14.0,
                                contrast_text(self.theme_accent),
                            );
                            ui.add_space(9.0);
                            ui.label(
                                egui::RichText::new(APP_NAME).size(14.0).strong().color(TEXT),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("v{APP_VERSION}"))
                                            .size(9.5)
                                            .color(MUTED),
                                    );
                                },
                            );
                        });
                        ui.add_space(10.0);
                        soft_divider(ui);
                        ui.add_space(6.0);

                        if nav_item(ui, Icon::Home, "Início", self.page == Page::Player, "Início") {
                            self.page = Page::Player;
                            self.search_active = false;
                            self.search_query.clear();
                        }
                        if nav_item(
                            ui,
                            Icon::History,
                            "Tocadas recentemente",
                            self.page == Page::History,
                            "Tocadas recentemente · atalho H",
                        ) {
                            self.page = Page::History;
                            self.search_active = false;
                            self.search_query.clear();
                        }
                        if nav_item(
                            ui,
                            Icon::Lyrics,
                            "Letras",
                            self.page == Page::Lyrics,
                            "Letras · atalho L",
                        ) {
                            self.page = Page::Lyrics;
                            self.search_active = false;
                            self.search_query.clear();
                        }
                        if nav_item(
                            ui,
                            Icon::Settings,
                            "Configurações",
                            self.page == Page::Settings,
                            "Configurações",
                        ) {
                            self.page = Page::Settings;
                            self.search_active = false;
                            self.search_query.clear();
                        }
                    });

                // Bottom stack anchored by explicit rectangles. The previous
                // bottom-up layout let the now-playing card slide over the
                // Discord row, hiding it.
                let (recents_rect, now_playing_rect, footer_rect) =
                    sidebar_stack_rects(panel_rect, navigation.response.rect.bottom());
                let recents_top = recents_rect.top();

                let mut open_settings = false;
                let mut open_player = false;
                if recents_rect.height() >= 60.0 {
                    self.draw_sidebar_recents(ui, state, recents_rect);
                }
                if now_playing_rect.top() >= recents_top {
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(now_playing_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                        |ui| {
                            ui.set_clip_rect(now_playing_rect);
                            open_player = self.draw_sidebar_now_playing(ui, state);
                        },
                    );
                }
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(footer_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                    |ui| {
                        ui.set_clip_rect(footer_rect);
                        open_settings = self.draw_sidebar_footer(ui, state);
                    },
                );
                if open_settings {
                    self.page = Page::Settings;
                    self.search_active = false;
                    self.search_query.clear();
                } else if open_player {
                    self.page = Page::Player;
                    self.search_active = false;
                }
            });
    }

    /// Compact list of the latest tracks filling the space under the
    /// navigation. A row opens the history filtered to that track; the header
    /// opens the unfiltered history page.
    fn draw_sidebar_recents(&mut self, ui: &mut egui::Ui, state: &ViewState, rect: egui::Rect) {
        let mut picked: Option<String> = None;
        let mut open_all = false;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_clip_rect(rect);
                egui::Frame::new()
                    .fill(BG)
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(10, 10))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.set_min_height(ui.available_height());
                        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                        ui.horizontal(|ui| {
                            ui.label(kicker("RECENTES"));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let response = ui
                                        .add(
                                            egui::Label::new(
                                                egui::RichText::new("Ver tudo")
                                                    .size(9.5)
                                                    .color(MUTED),
                                            )
                                            .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text("Abrir tocadas recentemente");
                                    if response.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    open_all = response.clicked();
                                },
                            );
                        });
                        ui.add_space(6.0);
                        let row_height = 38.0;
                        let rows = ((ui.available_height() - 2.0) / row_height).floor().max(0.0)
                            as usize;
                        if state.recent_tracks.is_empty() {
                            ui.label(
                                egui::RichText::new("As músicas que você ouvir aparecem aqui.")
                                    .size(9.5)
                                    .color(MUTED),
                            );
                            return;
                        }
                        if rows == 0 {
                            return;
                        }
                        let inner_width = ui.available_width();
                        let current = state.track.as_ref();
                        for entry in state.recent_tracks.iter().take(rows.min(9)) {
                            let cover = self.history_cover(ui.ctx(), entry);
                            let (row, response) = ui.allocate_exact_size(
                                egui::vec2(inner_width, row_height),
                                egui::Sense::click(),
                            );
                            if response.hovered() {
                                ui.painter().rect_filled(
                                    row.shrink2(egui::vec2(0.0, 1.0)),
                                    egui::CornerRadius::same(6),
                                    blend(BG, SURFACE_HOVER, 0.45),
                                );
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            let is_current = current.is_some_and(|track| {
                                track.title.trim().eq_ignore_ascii_case(&entry.title)
                                    && track.artist.trim().eq_ignore_ascii_case(&entry.artist)
                            });
                            let thumb = egui::Rect::from_min_size(
                                egui::pos2(row.left() + 4.0, row.center().y - 14.0),
                                egui::vec2(28.0, 28.0),
                            );
                            if let Some(texture) = &cover {
                                egui::Image::new(texture)
                                    .fit_to_exact_size(egui::vec2(28.0, 28.0))
                                    .corner_radius(egui::CornerRadius::same(4))
                                    .paint_at(ui, thumb);
                            } else {
                                paint_gradient_rect(
                                    ui.painter(),
                                    thumb,
                                    blend(SURFACE, self.theme_accent, 0.22),
                                    SURFACE,
                                    4,
                                );
                                paint_note_at(ui.painter(), thumb.center(), 13.0, MUTED);
                            }
                            let text_left = thumb.right() + 9.0;
                            let text_width = (row.right() - text_left - 12.0).max(40.0);
                            let chars = ((text_width / 6.2) as usize).max(5);
                            ui.painter().text(
                                egui::pos2(text_left, row.center().y - 7.0),
                                egui::Align2::LEFT_CENTER,
                                truncate(&entry.title, chars),
                                egui::FontId::proportional(11.5),
                                if is_current { self.theme_accent } else { TEXT },
                            );
                            ui.painter().text(
                                egui::pos2(text_left, row.center().y + 8.0),
                                egui::Align2::LEFT_CENTER,
                                truncate(&entry.artist, chars),
                                egui::FontId::proportional(9.5),
                                MUTED,
                            );
                            if is_current {
                                ui.painter().circle_filled(
                                    egui::pos2(row.right() - 8.0, row.center().y),
                                    2.5,
                                    self.theme_accent,
                                );
                            }
                            if response.on_hover_text("Ver no histórico").clicked() {
                                picked = Some(entry.title.clone());
                            }
                        }
                    });
            },
        );
        if let Some(title) = picked {
            self.history_filter = title;
            self.page = Page::History;
            self.search_active = false;
        } else if open_all {
            self.history_filter.clear();
            self.page = Page::History;
            self.search_active = false;
        }
    }

    /// Compact now-playing card: cover, current state, title/artist and the
    /// progress rail. Clicking it opens the full player page.
    fn draw_sidebar_now_playing(&mut self, ui: &mut egui::Ui, state: &ViewState) -> bool {
        let width = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 96.0), egui::Sense::click());
        let hover = ui.ctx().animate_bool_with_time(
            egui::Id::new("sidebar-now-playing"),
            response.hovered(),
            0.12,
        );
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(8),
            blend(BG, SURFACE_HOVER, hover * 0.32),
        );

        let inner = rect.shrink2(egui::vec2(12.0, 12.0));
        let cover_size = 48.0;
        let cover_rect = egui::Rect::from_min_size(inner.min, egui::vec2(cover_size, cover_size));
        if let Some(texture) = &self.texture {
            ui.put(
                cover_rect,
                egui::Image::new(texture)
                    .fit_to_exact_size(egui::vec2(cover_size, cover_size))
                    .corner_radius(egui::CornerRadius::same(4)),
            );
        } else {
            paint_gradient_rect(
                ui.painter(),
                cover_rect,
                blend(self.theme_accent, BG, 0.35),
                SURFACE,
                4,
            );
            paint_note_at(ui.painter(), cover_rect.center(), 20.0, MUTED);
        }

        let (kicker_text, title, artist, playing, position, duration) = match &state.track {
            Some(track) => (
                if track.is_playing { "TOCANDO" } else { "PAUSADO" },
                track.title.as_str(),
                track.artist.as_str(),
                track.is_playing,
                track.estimated_position(),
                track.duration_ms,
            ),
            None => (
                "FILA VAZIA",
                "Nada tocando",
                "Abra o Spotify para começar",
                false,
                0,
                0,
            ),
        };

        let text_left = cover_rect.right() + 12.0;
        let text_width = (inner.right() - text_left).max(48.0);
        let text_chars = ((text_width / 6.4) as usize).max(6);
        let painter = ui.painter();
        painter.text(
            egui::pos2(text_left, inner.top() + 5.0),
            egui::Align2::LEFT_CENTER,
            kicker_text,
            egui::FontId::proportional(9.0),
            if playing { self.theme_accent } else { MUTED },
        );
        painter.text(
            egui::pos2(text_left, inner.top() + 23.0),
            egui::Align2::LEFT_CENTER,
            truncate(title, text_chars),
            egui::FontId::proportional(12.5),
            TEXT,
        );
        painter.text(
            egui::pos2(text_left, inner.top() + 40.0),
            egui::Align2::LEFT_CENTER,
            truncate(artist, text_chars),
            egui::FontId::proportional(10.5),
            MUTED,
        );

        let rail = egui::Rect::from_min_size(
            egui::pos2(inner.left(), inner.bottom() - 3.0),
            egui::vec2(inner.width(), 3.0),
        );
        painter.rect_filled(rail, egui::CornerRadius::same(2), RAIL);
        if duration > 0 {
            let ratio = (position as f32 / duration as f32).clamp(0.0, 1.0);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    rail.min,
                    egui::vec2(rail.width() * ratio, rail.height()),
                ),
                egui::CornerRadius::same(2),
                self.theme_accent,
            );
        }

        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response.on_hover_text("Abrir o player").clicked()
    }

    /// Discord status and account, both leading to Settings.
    fn draw_sidebar_footer(&mut self, ui: &mut egui::Ui, state: &ViewState) -> bool {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 104.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(8), BG);
        let inner = rect.shrink2(egui::vec2(12.0, 12.0));

        let connected = state.discord_status == "connected";
        let failed = state.discord_status.starts_with("Erro");
        let detail = if connected {
            "Discord conectado"
        } else if failed {
            "Falha ao publicar o status"
        } else if state.track.is_some() {
            "Sincronizando status"
        } else {
            "Aguardando música"
        };
        let detail_color = if connected {
            self.theme_accent
        } else if failed {
            DANGER
        } else {
            MUTED
        };

        let discord_rect = egui::Rect::from_min_size(inner.min, egui::vec2(inner.width(), 22.0));
        let discord = ui.interact(
            discord_rect,
            ui.id().with("sidebar-discord"),
            egui::Sense::click(),
        );
        if discord.hovered() {
            ui.painter().rect_filled(
                discord_rect.expand2(egui::vec2(4.0, 2.0)),
                egui::CornerRadius::same(4),
                blend(BG, SURFACE, 0.9),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.painter().circle_filled(
            egui::pos2(discord_rect.left() + 3.0, discord_rect.center().y),
            3.0,
            detail_color,
        );
        ui.painter().text(
            egui::pos2(discord_rect.left() + 13.0, discord_rect.center().y),
            egui::Align2::LEFT_CENTER,
            detail,
            egui::FontId::proportional(10.5),
            detail_color,
        );
        let discord = discord.on_hover_text("Abrir configurações do Discord");

        let divider_y = discord_rect.bottom() + 8.0;
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(inner.left(), divider_y),
                egui::vec2(inner.width(), 1.0),
            ),
            egui::CornerRadius::same(0),
            BORDER_SOFT,
        );

        let account_rect = egui::Rect::from_min_size(
            egui::pos2(inner.left(), divider_y + 10.0),
            egui::vec2(inner.width(), 26.0),
        );
        let account = ui.interact(
            account_rect,
            ui.id().with("sidebar-account"),
            egui::Sense::click(),
        );
        if account.hovered() {
            ui.painter().rect_filled(
                account_rect.expand2(egui::vec2(4.0, 1.0)),
                egui::CornerRadius::same(4),
                blend(BG, SURFACE, 0.9),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let account_name = state
            .account
            .as_ref()
            .map(|account| account.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or("Sua conta");
        let name_chars = (((inner.width() - 64.0) / 6.2) as usize).max(6);
        let avatar_rect = egui::Rect::from_center_size(
            egui::pos2(account_rect.left() + 13.0, account_rect.center().y),
            egui::vec2(26.0, 26.0),
        );
        if let Some(texture) = &self.avatar_texture {
            ui.put(
                avatar_rect,
                egui::Image::new(texture)
                    .fit_to_exact_size(egui::vec2(26.0, 26.0))
                    .corner_radius(egui::CornerRadius::same(13)),
            );
        } else {
            ui.painter()
                .circle_filled(
                    avatar_rect.center(),
                    13.0,
                    blend(SURFACE, self.theme_accent, 0.18),
                );
            let initial: String = account_name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            ui.painter().text(
                avatar_rect.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                egui::FontId::proportional(11.0),
                TEXT,
            );
        }
        ui.painter().text(
            egui::pos2(avatar_rect.right() + 9.0, account_rect.center().y),
            egui::Align2::LEFT_CENTER,
            truncate(account_name, name_chars),
            egui::FontId::proportional(11.5),
            TEXT_DIM,
        );
        paint_glyph(
            ui.painter(),
            egui::pos2(account_rect.right() - 9.0, account_rect.center().y),
            15.0,
            Glyph::Settings,
            MUTED,
        );
        let account = account.on_hover_text("Conta e configurações");

        discord.clicked() || account.clicked()
    }

    fn draw_header(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        egui::Frame::new()
            .fill(egui::Color32::TRANSPARENT)
            .inner_margin(egui::Margin {
                left: 20,
                right: 14,
                top: 10,
                bottom: 10,
            })
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.set_min_height(56.0);
                ui.horizontal(|ui| {
                    let can_go_back = self.page != Page::Player || self.search_active;
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(32.0, 32.0),
                        if can_go_back {
                            egui::Sense::click()
                        } else {
                            egui::Sense::hover()
                        },
                    );
                    let fill = if response.hovered() && can_go_back {
                        egui::Color32::from_black_alpha(170)
                    } else {
                        egui::Color32::from_black_alpha(110)
                    };
                    ui.painter().circle_filled(rect.center(), 16.0, fill);
                    paint_glyph(
                        ui.painter(),
                        rect.center(),
                        16.0,
                        Glyph::ChevronLeft,
                        if can_go_back { TEXT } else { MUTED },
                    );
                    let go_back = can_go_back && response.clicked();
                    response.on_hover_text("Voltar");
                    if go_back {
                        self.page = Page::Player;
                        self.search_active = false;
                        self.search_query.clear();
                    }
                    ui.add_space(6.0);
                    if self.page != Page::Player {
                        let title = match self.page {
                            Page::History => "Tocadas recentemente",
                            Page::Lyrics => "Letras",
                            Page::Settings => "Configurações",
                            Page::Player => "",
                        };
                        ui.label(egui::RichText::new(title).size(13.5).strong().color(TEXT_DIM));
                        ui.add_space(10.0);
                    }
                    if self.page == Page::Player {
                        // Keep room for the search icon, the clear button and the
                        // avatar cluster so the pill is never clipped or overflows.
                        let pill_width = (ui.available_width() - 148.0).clamp(148.0, 364.0);
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(36, 36, 36))
                            .corner_radius(egui::CornerRadius::same(24))
                            .inner_margin(egui::Margin::symmetric(14, 4))
                            .show(ui, |ui| {
                                ui.set_min_size(egui::vec2(pill_width, 40.0));
                                ui.horizontal(|ui| {
                                    let (icon_rect, _) = ui.allocate_exact_size(
                                        egui::vec2(24.0, 30.0),
                                        egui::Sense::hover(),
                                    );
                                    paint_icon(
                                        ui.painter(),
                                        icon_rect.center(),
                                        Icon::Search,
                                        MUTED,
                                    );
                                    let response = ui.add(
                                        egui::TextEdit::singleline(&mut self.search_query)
                                            .id(egui::Id::new("spotify-search"))
                                            .hint_text("Buscar no seu histórico")
                                            .desired_width((pill_width - 74.0).max(96.0))
                                            .frame(false),
                                    );
                                    self.search_active |= response.changed();
                                    if !self.search_query.is_empty()
                                        && ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("×").size(17.0).color(TEXT),
                                                )
                                                .fill(egui::Color32::TRANSPARENT)
                                                .stroke(egui::Stroke::NONE),
                                            )
                                            .clicked()
                                    {
                                        self.search_query.clear();
                                    }
                                });
                            });
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let name = state
                            .account
                            .as_ref()
                            .map(|account| account.name.as_str())
                            .unwrap_or("SpotifySync");
                        let avatar_response = if let Some(texture) = &self.avatar_texture {
                            ui.add(
                                egui::Image::new(texture)
                                    .fit_to_exact_size(egui::vec2(34.0, 34.0))
                                    .corner_radius(egui::CornerRadius::same(17))
                                    .sense(egui::Sense::click()),
                            )
                        } else {
                            let (avatar_rect, response) = ui
                                .allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::click());
                            ui.painter().circle_filled(
                                avatar_rect.center(),
                                16.0,
                                egui::Color32::from_rgb(83, 83, 83),
                            );
                            ui.painter().text(
                                avatar_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                name.chars().next().unwrap_or('S').to_ascii_uppercase(),
                                egui::FontId::proportional(14.0),
                                TEXT,
                            );
                            response
                        };
                        if avatar_response.on_hover_text(name).clicked() {
                            self.page = if self.page == Page::Settings {
                                Page::Player
                            } else {
                                Page::Settings
                            };
                        }
                        ui.add_space(6.0);
                        if icon_button(
                            ui,
                            Glyph::Settings,
                            32.0,
                            self.page == Page::Settings,
                            false,
                            "Configurações",
                            true,
                        ) {
                            self.page = if self.page == Page::Settings {
                                Page::Player
                            } else {
                                Page::Settings
                            };
                            self.search_active = false;
                        }
                    });
                });
            });
    }

    fn draw_login(&mut self, ctx: &egui::Context, state: &ViewState) {
        let time = ctx.input(|input| input.time) as f32;
        let accent = self.theme_accent;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BLACK))
            .show(ctx, |ui| {
                paint_login_backdrop(ui.painter(), ui.max_rect(), accent, time);
                egui::ScrollArea::vertical()
                    .id_salt("login-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let full_width = ui.available_width();
                        let wide = full_width >= 900.0;
                        let gap = if wide { 44.0 } else { 28.0 };
                        let card_width = if wide {
                            (full_width * 0.4).clamp(380.0, 460.0)
                        } else {
                            full_width.min(520.0).max(300.0)
                        };
                        let hero_width = if wide {
                            (full_width - card_width - gap - 56.0).clamp(320.0, 560.0)
                        } else {
                            card_width
                        };
                        let content_width = if wide {
                            hero_width + card_width + gap
                        } else {
                            card_width
                        };
                        let side = ((full_width - content_width) * 0.5).max(14.0);
                        ui.add_space(if wide { 54.0 } else { 34.0 });
                        ui.horizontal_top(|ui| {
                            ui.add_space(side);
                            ui.allocate_ui_with_layout(
                                egui::vec2(content_width, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    if wide {
                                        ui.horizontal_top(|ui| {
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(hero_width, 0.0),
                                                egui::Layout::top_down(egui::Align::Min),
                                                |ui| self.draw_login_hero(ui, accent, time),
                                            );
                                            ui.add_space(gap);
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(card_width, 0.0),
                                                egui::Layout::top_down(egui::Align::Min),
                                                |ui| self.draw_login_card(ui, state, accent, time),
                                            );
                                        });
                                    } else {
                                        self.draw_login_hero(ui, accent, time);
                                        ui.add_space(gap);
                                        self.draw_login_card(ui, state, accent, time);
                                    }
                                },
                            );
                        });
                        ui.add_space(32.0);
                    });
            });
        // Drives the drifting glow, the pulse and the equalizer animation.
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    fn draw_login_hero(&mut self, ui: &mut egui::Ui, accent: egui::Color32, time: f32) {
        let title_size = (ui.available_width() / 11.0).clamp(28.0, 44.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 22.0, blend(SURFACE, accent, 0.12));
            ui.painter().circle_stroke(
                rect.center(),
                22.0,
                egui::Stroke::new(1.0_f32, blend(BORDER, accent, 0.42)),
            );
            ui.painter().image(
                self.logo_texture.id(),
                egui::Rect::from_center_size(rect.center(), egui::vec2(32.0, 32.0)),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.label(bold(APP_NAME, 19.0).color(TEXT));
                    ui.label(
                        egui::RichText::new("STATUS DO DISCORD · LETRAS EM TEMPO REAL")
                            .size(8.5)
                            .strong()
                            .color(blend(MUTED, accent, 0.55)),
                    );
                },
            );
        });
        ui.add_space(28.0);
        ui.label(
            egui::RichText::new("Sua música,")
                .size(title_size)
                .strong()
                .color(TEXT),
        );
        ui.label(
            egui::RichText::new("linha por linha.")
                .size(title_size)
                .strong()
                .color(accent),
        );
        ui.add_space(14.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Conecte sua conta do Discord e a letra que está tocando no Spotify aparece no seu perfil — com controle de reprodução, histórico e letras sincronizadas.",
                )
                .size(12.5)
                .color(MUTED),
            )
            .wrap(),
        );
        ui.add_space(26.0);
        for (feature, title, detail) in [
            (
                LoginFeature::Wave,
                "Status em tempo real",
                "A linha da letra que está tocando aparece no seu perfil.",
            ),
            (
                LoginFeature::Lyric,
                "Letras sincronizadas",
                "Acompanhe a música com a letra no ritmo certo.",
            ),
            (
                LoginFeature::Shield,
                "Token só neste PC",
                "Criptografado com DPAPI e nunca enviado para outro lugar.",
            ),
        ] {
            login_feature(ui, feature, title, detail, accent);
            ui.add_space(14.0);
        }
        ui.add_space(6.0);
        login_status_preview(ui, accent, time, &self.logo_texture);
    }

    fn draw_login_card(
        &mut self,
        ui: &mut egui::Ui,
        state: &ViewState,
        accent: egui::Color32,
        time: f32,
    ) {
        let width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::Frame::new()
                    .fill(blend(INSET, accent, 0.05))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        blend(BORDER_SOFT, accent, 0.32),
                    ))
                    .corner_radius(egui::CornerRadius::same(22))
                    .inner_margin(egui::Margin::same(3))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        egui::Frame::new()
                            .fill(blend(SURFACE, accent, 0.045))
                            .corner_radius(egui::CornerRadius::same(20))
                            .inner_margin(egui::Margin::symmetric(24, 24))
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                self.login_form(ui, state, accent, time);
                            });
                    });
            },
        );
    }

    fn login_form(
        &mut self,
        ui: &mut egui::Ui,
        state: &ViewState,
        accent: egui::Color32,
        time: f32,
    ) {
        ui.horizontal(|ui| {
            ui.label(kicker("CONEXÃO SEGURA"));
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    status_chip(
                        ui,
                        match state.auth {
                            AuthState::Validating => "Verificando",
                            AuthState::Failed => "Falhou",
                            _ => "Desconectado",
                        },
                        match state.auth {
                            AuthState::Validating => "waiting",
                            AuthState::Failed => "error",
                            _ => "no-token",
                        },
                    );
                },
            );
        });
        ui.add_space(14.0);
        ui.label(bold("Conectar sua conta", 21.0).color(TEXT));
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Cole o token do Discord para sincronizar seu status. Validamos antes de guardar e criptografamos com o Windows (DPAPI).",
                )
                .size(11.0)
                .color(MUTED),
            )
            .wrap(),
        );
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.label(kicker("TOKEN DE ACESSO"));
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} caracteres",
                            self.token_input.trim().chars().count()
                        ))
                        .size(9.0)
                        .color(blend(MUTED, accent, 0.35)),
                    );
                },
            );
        });
        ui.add_space(7.0);
        let mut submitted = false;
        egui::Frame::new()
            .fill(INSET)
            .stroke(egui::Stroke::new(
                1.0_f32,
                if self.token_input.trim().is_empty() {
                    blend(BORDER_SOFT, accent, 0.22)
                } else {
                    blend(BORDER, accent, 0.5)
                },
            ))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(11, 9))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.token_input)
                        .password(!self.show_token)
                        .frame(false)
                        .hint_text("Cole seu token do Discord")
                        .font(egui::FontId::proportional(12.0))
                        .desired_width(f32::INFINITY),
                );
                if response.changed() {
                    self.ui_error = None;
                }
                submitted = response.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
            });
        ui.add_space(7.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
            paint_login_feature(
                ui.painter(),
                rect.center(),
                11.0,
                LoginFeature::Shield,
                blend(MUTED, accent, 0.45),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new("Fica apenas neste computador. Nunca compartilhe seu token.")
                        .size(9.5)
                        .color(MUTED),
                )
                .wrap(),
            );
        });
        ui.add_space(16.0);
        let ready = !self.token_input.trim().is_empty() && state.auth != AuthState::Validating;
        let mut connect = false;
        ui.horizontal(|ui| {
            if ghost_button(
                ui,
                if self.show_token {
                    "Ocultar token"
                } else {
                    "Mostrar token"
                },
                118.0,
            ) {
                self.show_token = !self.show_token;
            }
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    connect = filled_button(
                        ui,
                        if state.auth == AuthState::Validating {
                            "Verificando…"
                        } else {
                            "Conectar"
                        },
                        150.0,
                        ready,
                    );
                },
            );
        });
        if connect || submitted {
            self.submit_token();
        }
        if let Some(error) = &self.ui_error {
            ui.add_space(12.0);
            error_banner(ui, error);
        }
        ui.add_space(18.0);
        soft_divider(ui);
        ui.add_space(14.0);
        let status_color = match state.auth {
            AuthState::SignedIn => accent,
            AuthState::Validating => WARNING,
            AuthState::Failed => DANGER,
            _ => MUTED,
        };
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            if state.auth == AuthState::Validating {
                let start = time * 5.0;
                let arc = login_arc(rect.center(), 4.0, start, start + 4.2);
                ui.painter()
                    .add(egui::Shape::line(arc, egui::Stroke::new(1.6_f32, status_color)));
            } else {
                let pulse = ((time * 2.2).sin() * 0.5 + 0.5) * 0.9;
                ui.painter()
                    .circle_filled(rect.center(), 3.0 + pulse, status_color);
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&state.token_status)
                        .size(10.5)
                        .color(if state.auth == AuthState::Failed {
                            DANGER
                        } else {
                            MUTED
                        }),
                )
                .wrap(),
            );
        });
        ui.add_space(18.0);
        ui.label(kicker("COMO OBTER SEU TOKEN"));
        ui.add_space(9.0);
        for (index, step) in [
            "Abra o Discord no navegador e entre na sua conta.",
            "Pressione F12 e abra a aba Rede (Network).",
            "Procure qualquer requisição para discord.com/api.",
            "Copie o valor do cabeçalho authorization.",
        ]
        .iter()
        .enumerate()
        {
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(19.0, 19.0), egui::Sense::hover());
                ui.painter().circle_filled(
                    rect.center(),
                    9.0,
                    blend(SURFACE, accent, 0.18),
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    (index + 1).to_string(),
                    egui::FontId::new(
                        9.0,
                        egui::FontFamily::Name("SpotifyBold".into()),
                    ),
                    accent,
                );
                ui.add_space(3.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(*step).size(10.0).color(MUTED),
                    )
                    .wrap(),
                );
            });
            ui.add_space(6.0);
        }
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Aviso: usar token de usuário pode violar os Termos de Serviço do Discord.",
                )
                .size(9.5)
                .color(WARNING),
            )
            .wrap(),
        );
        ui.add_space(16.0);
        ui.horizontal_wrapped(|ui| {
            for label in ["DPAPI", "100% local", "Sem senha"] {
                login_pill(ui, label, accent);
                ui.add_space(5.0);
            }
        });
    }

    fn draw_home(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        let query = self.search_query.trim().to_lowercase();
        if !query.is_empty() {
            let results: Vec<&HistoryEntry> = state
                .recent_tracks
                .iter()
                .filter(|entry| history_matches(entry, &query))
                .collect();
            ui.label(
                egui::RichText::new(format!("Resultados para “{}”", self.search_query.trim()))
                    .size(24.0)
                    .strong()
                    .color(TEXT),
            );
            ui.add_space(8.0);
            if results.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(48.0);
                    paint_note(ui, 34.0, blend(MUTED, self.theme_accent, 0.4));
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("Nada no seu histórico corresponde à busca")
                            .size(17.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        egui::RichText::new(
                            "A busca cobre as músicas que você já ouviu neste computador.",
                        )
                        .size(11.0)
                        .color(MUTED),
                    );
                });
            } else {
                ui.label(
                    egui::RichText::new(format!(
                        "{} de {} faixas do histórico",
                        results.len(),
                        state.recent_tracks.len()
                    ))
                    .size(11.0)
                    .color(MUTED),
                );
                ui.add_space(12.0);
                history_header(ui);
                soft_divider(ui);
                ui.add_space(4.0);
                for entry in results {
                    let cover = self.history_cover(ui.ctx(), entry);
                    history_row(ui, entry, state.track.as_ref(), cover.as_ref());
                }
            }
            return;
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(greeting_text())
                    .size(32.0)
                    .strong()
                    .color(TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let playing = state.track.as_ref().is_some_and(|track| track.is_playing);
                status_chip(
                    ui,
                    if state.track.is_some() {
                        if playing {
                            "TOCANDO"
                        } else {
                            "PAUSADO"
                        }
                    } else {
                        "AGUARDANDO"
                    },
                    if playing {
                        "connected"
                    } else {
                        "waiting"
                    },
                );
            });
        });
        ui.add_space(18.0);
        self.draw_listening_stats(ui, state);
        ui.add_space(22.0);
        self.draw_home_hero(ui, state);
        ui.add_space(30.0);
        self.draw_recent_history(ui, state, 5);
        ui.add_space(30.0);
        self.draw_recent_albums(ui, state);
    }

    /// Numbers computed from the saved history. They change on every new
    /// track instead of showing a fixed catalogue.
    fn draw_listening_stats(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        let entries = &state.recent_tracks;
        let day_start = now_ms().saturating_sub(local_midnight_offset_ms());
        let today = entries
            .iter()
            .filter(|entry| entry.played_at >= day_start)
            .count();
        let total_ms: u64 = entries.iter().map(|entry| entry.duration_ms).sum();
        let favorite = favorite_artist(entries)
            .map(|name| truncate(&name, 18))
            .unwrap_or_else(|| "—".to_owned());
        let mut unique = HashSet::new();
        for entry in entries {
            unique.insert(format!(
                "{}::{}",
                entry.title.trim().to_lowercase(),
                entry.artist.trim().to_lowercase()
            ));
        }
        let today_value = match today {
            0 => "—".to_owned(),
            1 => "1 faixa".to_owned(),
            count => format!("{count} faixas"),
        };
        let tiles = [
            ("HOJE", today_value, "ouvidas hoje"),
            ("TEMPO SALVO", format_span(total_ms), "somando as faixas"),
            ("ARTISTA", favorite, "mais frequente"),
            (
                "FAIXAS",
                unique.len().to_string(),
                "diferentes no histórico",
            ),
        ];
        let available = ui.available_width();
        let columns = if available < 560.0 { 2 } else { 4 };
        let gap = 10.0;
        let width = ((available - gap * (columns as f32 - 1.0)) / columns as f32).max(130.0);
        for row in tiles.chunks(columns) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (index, (label, value, detail)) in row.iter().enumerate() {
                    stat_tile(ui, width, label, value, detail, self.theme_accent);
                    if index + 1 < row.len() {
                        ui.add_space(gap);
                    }
                }
            });
            ui.add_space(gap);
        }
    }

    /// Album wall built from the real history. Each card opens the history
    /// filtered by that album, and the album currently playing is marked.
    fn draw_recent_albums(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        let mut seen = HashSet::new();
        let albums: Vec<&HistoryEntry> = state
            .recent_tracks
            .iter()
            .filter(|entry| !entry.album.trim().is_empty())
            .filter(|entry| seen.insert(entry.album.trim().to_lowercase()))
            .take(8)
            .collect();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Álbuns recentes")
                    .size(24.0)
                    .strong()
                    .color(TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let button = ui.add(
                    egui::Button::new(egui::RichText::new("Ver histórico").size(10.5).color(MUTED))
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE),
                );
                if button.clicked() {
                    self.page = Page::History;
                }
            });
        });
        ui.add_space(12.0);
        if albums.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Quando o Spotify informar o álbum das faixas, eles aparecem aqui.",
                )
                .size(11.0)
                .color(MUTED),
            );
            return;
        }
        let available = ui.available_width();
        let columns = if available < 470.0 {
            2
        } else if available < 850.0 {
            3
        } else if available < 1_240.0 {
            4
        } else {
            5
        };
        let gap = 16.0;
        let width = ((available - gap * (columns as f32 - 1.0)) / columns as f32).max(110.0);
        for row in albums.chunks(columns) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (index, entry) in row.iter().enumerate() {
                    let cover = self.history_cover(ui.ctx(), entry);
                    let is_current = state.track.as_ref().is_some_and(|track| {
                        track.album.trim().eq_ignore_ascii_case(entry.album.trim())
                    });
                    if album_card(ui, width, entry, cover.as_ref(), self.theme_accent, is_current) {
                        self.history_filter = entry.album.trim().to_owned();
                        self.page = Page::History;
                        self.search_active = false;
                        self.search_query.clear();
                    }
                    if index + 1 < row.len() {
                        ui.add_space(gap);
                    }
                }
            });
            ui.add_space(16.0);
        }
    }

    fn draw_recent_history(&mut self, ui: &mut egui::Ui, state: &ViewState, limit: usize) {
        let mut open_history = false;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Tocadas recentemente")
                    .size(24.0)
                    .strong()
                    .color(TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                open_history = ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Ver histórico").size(10.5).color(MUTED),
                        )
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE),
                    )
                    .clicked();
            });
        });
        if open_history {
            self.page = Page::History;
        }
        ui.add_space(10.0);
        if state.recent_tracks.is_empty() {
            ui.label(
                egui::RichText::new("As músicas que você tocar aparecerão aqui.")
                    .size(11.0)
                    .color(MUTED),
            );
            return;
        }
        for entry in state.recent_tracks.iter().take(limit) {
            let cover = self.history_cover(ui.ctx(), entry);
            history_row(ui, entry, state.track.as_ref(), cover.as_ref());
        }
    }

    fn draw_history_page(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(bold("Tocadas recentemente", 32.0).color(TEXT));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Seu histórico de reprodução neste computador")
                        .size(12.5)
                        .color(MUTED),
                );
            });
        });
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(300.0, 32.0), egui::Sense::hover());
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(16),
                egui::Color32::from_rgb(36, 36, 36),
            );
            paint_icon(
                ui.painter(),
                egui::pos2(rect.left() + 18.0, rect.center().y),
                Icon::Search,
                MUTED,
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(33.0, 4.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.history_filter)
                            .id(egui::Id::new("history-filter"))
                            .hint_text(
                                egui::RichText::new("Filtrar por música, artista ou álbum")
                                    .size(12.0)
                                    .color(MUTED),
                            )
                            .desired_width(ui.available_width())
                            .frame(false),
                    );
                },
            );
            if !self.history_filter.trim().is_empty()
                && ghost_button(ui, "Limpar filtro", 96.0)
            {
                self.history_filter.clear();
            }
        });
        ui.add_space(22.0);
        if state.recent_tracks.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(54.0);
                paint_note(ui, 38.0, blend(MUTED, self.theme_accent, 0.4));
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new("Seu histórico começa com a próxima música")
                        .size(17.0)
                        .strong()
                        .color(TEXT),
                );
                ui.add_space(5.0);
                ui.label(
                    egui::RichText::new("As faixas tocadas ficam salvas localmente.")
                        .size(11.0)
                        .color(MUTED),
                );
            });
            return;
        }

        let filter = self.history_filter.trim().to_lowercase();
        let entries: Vec<&HistoryEntry> = state
            .recent_tracks
            .iter()
            .filter(|entry| history_matches(entry, &filter))
            .collect();
        if entries.len() != state.recent_tracks.len() {
            ui.label(
                egui::RichText::new(format!(
                    "{} de {} faixas",
                    entries.len(),
                    state.recent_tracks.len()
                ))
                .size(11.0)
                .color(MUTED),
            );
            ui.add_space(8.0);
        }
        if entries.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    egui::RichText::new("Nenhuma faixa corresponde ao filtro")
                        .size(15.0)
                        .strong()
                        .color(TEXT),
                );
            });
            return;
        }
        history_header(ui);
        soft_divider(ui);
        ui.add_space(4.0);
        for entry in entries {
            let cover = self.history_cover(ui.ctx(), entry);
            history_row(ui, entry, state.track.as_ref(), cover.as_ref());
        }
    }

    fn draw_home_hero(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        egui::Frame::new()
            .fill(blend(SURFACE, self.art_tint, 0.34))
            .stroke(egui::Stroke::NONE)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if let Some(track) = &state.track {
                    let artwork_size = (ui.available_width() * 0.28).clamp(160.0, 224.0);
                    ui.horizontal_top(|ui| {
                        egui::Frame::new()
                            .fill(INSET)
                            .corner_radius(egui::CornerRadius::same(4))
                            .show(ui, |ui| {
                                if let Some(texture) = &self.texture {
                                    ui.add(
                                        egui::Image::new(texture)
                                            .fit_to_exact_size(egui::vec2(artwork_size, artwork_size))
                                            .corner_radius(egui::CornerRadius::same(4)),
                                    );
                                } else {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(artwork_size, artwork_size),
                                        egui::Layout::centered_and_justified(
                                            egui::Direction::TopDown,
                                        ),
                                        |ui| paint_note(ui, 58.0, MUTED),
                                    );
                                }
                            });
                        ui.add_space(18.0);
                        ui.vertical(|ui| {
                            ui.add_space(2.0);
                            ui.label(kicker(if track.is_playing {
                                "TOCANDO AGORA"
                            } else {
                                "PAUSADO"
                            }));
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new(truncate(&track.title, 30))
                                    .size(38.0)
                                    .strong()
                                    .color(TEXT),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(truncate(&track.artist, 40))
                                    .size(15.5)
                                    .color(TEXT_DIM),
                            );
                            if !track.album.is_empty() {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(truncate(&track.album, 42))
                                        .size(12.0)
                                        .color(MUTED),
                                );
                            }
                        });
                    });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if accent_chip(ui, "Ver letra") {
                            self.page = Page::Lyrics;
                            self.search_active = false;
                        }
                        ui.add_space(4.0);
                        if settings_chip(ui, "Copiar faixa", false) {
                            ui.ctx().copy_text(format!("{} — {}", track.title, track.artist));
                        }
                        if !state.lyrics.is_empty() {
                            ui.add_space(4.0);
                            if settings_chip(ui, "Copiar letra", false) {
                                let all = state
                                    .lyrics
                                    .iter()
                                    .map(|line| line.text.trim())
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                ui.ctx().copy_text(all);
                            }
                        }
                    });
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        status_chip(
                            ui,
                            if track.is_playing { "TOCANDO" } else { "PAUSADO" },
                            if track.is_playing {
                                "connected"
                            } else {
                                "waiting"
                            },
                        );
                        ui.add_space(5.0);
                        playback_equalizer(ui, track.is_playing);
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(&track.platform).size(9.5).color(MUTED));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(&state.lyrics_status)
                                        .size(9.5)
                                        .color(if state.lyrics.is_empty() {
                                            MUTED
                                        } else {
                                            self.theme_accent
                                        }),
                                );
                            },
                        );
                    });
                    ui.add_space(14.0);
                    soft_divider(ui);
                    ui.add_space(12.0);
                    let current = state
                        .lyric_index
                        .and_then(|index| state.lyrics.get(index))
                        .map(|line| line.text.trim().to_owned())
                        .filter(|line| !line.is_empty());
                    match current {
                        Some(line) => {
                            ui.label(egui::RichText::new(line).size(20.0).strong().color(TEXT));
                            if let Some(next) = state
                                .lyric_index
                                .and_then(|index| state.lyrics.get(index + 1))
                                .map(|line| line.text.trim().to_owned())
                                .filter(|line| !line.is_empty())
                            {
                                ui.add_space(5.0);
                                ui.label(egui::RichText::new(next).size(13.5).color(MUTED));
                            }
                        }
                        None => {
                            ui.label(
                                egui::RichText::new(if state.lyrics.is_empty() {
                                    "Sem letra sincronizada para esta faixa."
                                } else {
                                    "A letra começa em instantes."
                                })
                                .size(14.0)
                                .color(MUTED),
                            );
                        }
                    }
                } else {
                    ui.horizontal_top(|ui| {
                        egui::Frame::new()
                            .fill(blend(SURFACE, self.theme_accent, 0.08))
                            .stroke(egui::Stroke::new(
                                1.0_f32,
                                blend(BORDER, self.theme_accent, 0.18),
                            ))
                            .corner_radius(egui::CornerRadius::same(14))
                            .inner_margin(egui::Margin::same(14))
                            .show(ui, |ui| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(58.0, 58.0),
                                    egui::Layout::centered_and_justified(
                                        egui::Direction::TopDown,
                                    ),
                                    |ui| paint_note(ui, 34.0, self.theme_accent),
                                )
                            });
                        ui.add_space(14.0);
                        ui.vertical(|ui| {
                            ui.label(kicker("PRONTO PARA COMEÇAR"));
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new("Sua próxima música está a um play de distância")
                                    .size(18.0)
                                    .strong()
                                    .color(TEXT),
                            );
                            ui.add_space(5.0);
                            ui.label(
                                egui::RichText::new(
                                    "Abra o Spotify e reproduza uma faixa. A música e as letras serão detectadas automaticamente.",
                                )
                                .size(10.5)
                                .color(MUTED),
                            );
                            ui.add_space(10.0);
                            status_chip(ui, "AGUARDANDO SPOTIFY", "waiting");
                        });
                    });
                    ui.add_space(18.0);
                    soft_divider(ui);
                    ui.add_space(14.0);
                    let steps = [
                        ("01", "Inicie uma faixa", "Abra o Spotify e escolha uma música."),
                        ("02", "Acompanhe a letra", "Veja a linha atual avançar em tempo real."),
                        ("03", "Atualize o Discord", "Seu status acompanha a música automaticamente."),
                    ];
                    ui.columns(3, |columns| {
                        for (column, (number, title, description)) in
                            columns.iter_mut().zip(steps.iter())
                        {
                            egui::Frame::new()
                                .fill(blend(SURFACE, self.art_tint, 0.08))
                                .stroke(egui::Stroke::new(1.0_f32, BORDER_SOFT))
                                .corner_radius(egui::CornerRadius::same(RADIUS_MD))
                                .inner_margin(egui::Margin::same(12))
                                .show(column, |ui| {
                                    ui.label(
                                        egui::RichText::new(*number)
                                            .size(10.0)
                                            .strong()
                                            .color(self.theme_accent),
                                    );
                                    ui.add_space(5.0);
                                    ui.label(
                                        egui::RichText::new(*title)
                                            .size(10.5)
                                            .strong()
                                            .color(TEXT),
                                    );
                                    ui.add_space(3.0);
                                    ui.label(
                                        egui::RichText::new(*description)
                                            .size(9.0)
                                            .color(MUTED),
                                    );
                                });
                        }
                    });
                }
                if let Some(error) = state.last_error.as_ref().or(self.ui_error.as_ref()) {
                    ui.add_space(6.0);
                    error_banner(ui, error);
                }
            });
    }

    fn draw_lyrics_page(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        let card_height = ui.available_height().max(240.0);
        egui::Frame::new()
            .fill(SURFACE)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(22))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.set_min_height(card_height - 44.0);
                ui.horizontal(|ui| {
                    if let Some(texture) = &self.texture {
                        ui.add(
                            egui::Image::new(texture)
                                .fit_to_exact_size(egui::vec2(44.0, 44.0))
                                .corner_radius(egui::CornerRadius::same(10)),
                        );
                        ui.add_space(11.0);
                    }
                    ui.vertical(|ui| {
                        ui.label(kicker("AGORA NA FAIXA"));
                        ui.add_space(4.0);
                        if let Some(track) = &state.track {
                            ui.label(
                                egui::RichText::new(truncate(&track.title, 46))
                                    .size(15.0)
                                    .strong()
                                    .color(TEXT),
                            );
                            ui.label(
                                egui::RichText::new(truncate(&track.artist, 52))
                                    .size(10.5)
                                    .color(MUTED),
                            );
                        } else {
                            ui.label(egui::RichText::new("Nada tocando").size(15.0).color(MUTED));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(&state.lyrics_status)
                                .size(9.5)
                                .color(MUTED),
                        );
                    });
                });
                ui.add_space(14.0);
                soft_divider(ui);
                ui.add_space(8.0);
                let body_height = (ui.available_height() - 8.0).max(170.0);
                egui::ScrollArea::vertical()
                    .id_salt("lyrics-scroll")
                    .max_height(body_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if state.lyrics.is_empty() {
                            ui.add_space(28.0);
                            ui.vertical_centered(|ui| {
                                paint_note(ui, 38.0, blend(MUTED, self.theme_accent, 0.4));
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new("As letras aparecerão aqui")
                                        .size(17.0)
                                        .strong()
                                        .color(TEXT),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(&state.lyrics_status)
                                        .size(11.0)
                                        .color(MUTED),
                                );
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(if state.track.is_some() {
                                        "A letra será exibida assim que estiver disponível."
                                    } else {
                                        "Dê play em uma música para começar a sincronização."
                                    })
                                    .size(10.0)
                                    .color(MUTED),
                                );
                            });
                            self.last_scrolled_lyric = None;
                            return;
                        }
                        for (index, line) in state.lyrics.iter().enumerate() {
                            let active = state.lyric_index == Some(index);
                            let row = egui::Frame::new()
                                .fill(if active {
                                    blend(SURFACE, self.theme_accent, 0.10)
                                } else {
                                    egui::Color32::TRANSPARENT
                                })
                                .corner_radius(egui::CornerRadius::same(6))
                                .inner_margin(egui::Margin::symmetric(12, 9))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        if self.settings.show_timestamps {
                                            ui.label(
                                                egui::RichText::new(format_time(line.time_ms))
                                                    .size(9.5)
                                                    .color(if active {
                                                        self.theme_accent
                                                    } else {
                                                        MUTED
                                                    }),
                                            );
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                (if active {
                                                    egui::RichText::new(&line.text).strong()
                                                } else {
                                                    egui::RichText::new(&line.text)
                                                })
                                                .size(if active { 21.0 } else { 16.0 })
                                                .color(if active { TEXT } else { MUTED }),
                                            )
                                            .wrap(),
                                        );
                                    });
                                });
                            if active && self.last_scrolled_lyric != state.lyric_index {
                                row.response.scroll_to_me(Some(egui::Align::Center));
                            }
                            let jump = ui.interact(
                                row.response.rect,
                                ui.id().with(("lyric-jump", index)),
                                egui::Sense::click(),
                            );
                            if jump.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if jump.on_hover_text("Ir para este trecho").clicked() {
                                self.queue_seek(line.time_ms);
                            }
                            ui.add_space(2.0);
                        }
                        self.last_scrolled_lyric = state.lyric_index;
                    });
            });
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        const TABS: [&str; 5] = ["Conta", "Reprodução", "Letras", "Dados", "Sobre"];
        let mut selected = self.settings_tab.min(TABS.len() - 1);
        ui.label(bold("Configurações", 32.0).color(TEXT));
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Conta, sincronia das letras e dados salvos neste computador.",
                )
                .size(12.5)
                .color(MUTED),
            )
            .wrap(),
        );
        ui.add_space(18.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(208.0, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.label(kicker("PREFERÊNCIAS"));
                    ui.add_space(10.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (index, label) in TABS.iter().enumerate() {
                        if settings_nav(ui, label, selected == index) {
                            selected = index;
                        }
                    }
                    ui.add_space(20.0);
                    soft_divider(ui);
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new(format!("{APP_NAME} v{APP_VERSION}"))
                            .size(11.0)
                            .color(MUTED),
                    );
                },
            );
            ui.add_space(32.0);
            ui.vertical(|ui| {
                ui.set_min_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(780.0));
                        match selected {
                    0 => {
                        settings_panel_title(
                            ui,
                            "Conta",
                            "A sessão do Discord usada para publicar o status.",
                        );
                        let name = state
                            .account
                            .as_ref()
                            .map(|account| account.name.clone())
                            .unwrap_or_else(|| "Nenhuma conta".to_owned());
                        ui.horizontal(|ui| {
                            draw_avatar(ui, self.avatar_texture.as_ref(), &name, 56.0);
                            ui.add_space(14.0);
                            ui.vertical(|ui| {
                                ui.label(bold(&name, 20.0).color(TEXT));
                                ui.add_space(3.0);
                                ui.label(
                                    egui::RichText::new(&state.token_status)
                                        .size(12.5)
                                        .color(MUTED),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if state.account.is_some() && ghost_button(ui, "Sair", 74.0) {
                                        self.sign_out();
                                    }
                                },
                            );
                        });
                        ui.add_space(18.0);
                        soft_divider(ui);
                        ui.add_space(14.0);
                        ui.label(kicker("SESSÃO"));
                        ui.add_space(10.0);
                        settings_value_row(
                            ui,
                            "Identificador",
                            state
                                .account
                                .as_ref()
                                .map(|account| account.id.clone())
                                .unwrap_or_else(|| "—".to_owned()),
                        );
                        settings_value_row(
                            ui,
                            "Token",
                            "Salvo em token.bin com Windows DPAPI".to_owned(),
                        );
                        settings_value_row(
                            ui,
                            "Presença publicada",
                            presence_label(&self.settings.presence).to_owned(),
                        );
                        settings_status_row(
                            ui,
                            "Conexão com o Discord",
                            if state.discord_status.trim().is_empty() {
                                "Aguardando reprodução"
                            } else {
                                state.discord_status.as_str()
                            },
                            !state.discord_status.trim().is_empty(),
                        );
                        settings_status_row(
                            ui,
                            "Letras",
                            if state.lyrics_status.trim().is_empty() {
                                "Sem faixa"
                            } else {
                                state.lyrics_status.as_str()
                            },
                            has_active_lyric(state),
                        );
                        ui.add_space(12.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "O token nunca sai deste computador: ele é criptografado pelo Windows e só a sua conta consegue ler.",
                                )
                                .size(11.5)
                                .color(MUTED),
                            )
                            .wrap(),
                        );
                    }
                    1 => {
                        settings_panel_title(
                            ui,
                            "Reprodução",
                            "Como o status reage à música que está tocando.",
                        );
                        ui.label(kicker("STATUS"));
                        ui.add_space(10.0);
                        settings_row(
                            ui,
                            "Presença no Discord",
                            "Status exibido no seu perfil enquanto ouve.",
                            |ui| {
                                ui.scope(|ui| {
                                    let accent = ui.visuals().selection.stroke.color;
                                    let visuals = ui.visuals_mut();
                                    visuals.selection.bg_fill = blend(BG, accent, 0.65);
                                    visuals.selection.stroke =
                                        egui::Stroke::new(1.0_f32, accent);
                                    visuals.widgets.inactive.bg_fill =
                                        egui::Color32::from_rgb(38, 38, 38);
                                    visuals.widgets.inactive.weak_bg_fill =
                                        egui::Color32::from_rgb(38, 38, 38);
                                    visuals.widgets.hovered.bg_fill =
                                        egui::Color32::from_rgb(50, 50, 50);
                                    visuals.widgets.hovered.weak_bg_fill =
                                        egui::Color32::from_rgb(50, 50, 50);
                                    visuals.widgets.active.bg_fill =
                                        egui::Color32::from_rgb(50, 50, 50);
                                    visuals.widgets.active.weak_bg_fill =
                                        egui::Color32::from_rgb(50, 50, 50);
                                    egui::ComboBox::from_id_salt("presence")
                                        .width(160.0)
                                        .selected_text(presence_label(&self.settings.presence))
                                        .show_ui(ui, |ui| {
                                            for option in
                                                ["online", "idle", "dnd", "invisible"]
                                            {
                                                ui.selectable_value(
                                                    &mut self.settings.presence,
                                                    option.to_owned(),
                                                    presence_label(option),
                                                );
                                            }
                                        });
                                });
                            },
                        );
                        settings_row(
                            ui,
                            "Adiantar letra",
                            "Base do adiantamento aplicada a todas as faixas.",
                            |ui| {
                                ui.scope(|ui| {
                                    let accent = ui.visuals().selection.stroke.color;
                                    let visuals = ui.visuals_mut();
                                    visuals.selection.bg_fill = blend(BG, accent, 0.65);
                                    visuals.selection.stroke =
                                        egui::Stroke::new(1.0_f32, accent);
                                    visuals.widgets.inactive.bg_fill =
                                        egui::Color32::from_rgb(82, 82, 82);
                                    visuals.widgets.inactive.weak_bg_fill =
                                        egui::Color32::from_rgb(82, 82, 82);
                                    ui.add_sized(
                                        [200.0, 20.0],
                                        egui::Slider::new(
                                            &mut self.settings.sync_lead_ms,
                                            0..=1_500,
                                        )
                                        .suffix(" ms"),
                                    );
                                });
                            },
                        );
                        settings_row(
                            ui,
                            "Predefinições de sincronia",
                            "Valores rápidos para acertar o atraso da letra.",
                            |ui| {
                                // The row lays its controls out right to left, so the
                                // presets are added from the largest to the smallest.
                                for ms in [750_u16, 500, 250, 0] {
                                    let label = if ms == 0 {
                                        "Sem atraso".to_owned()
                                    } else {
                                        format!("{ms} ms")
                                    };
                                    if settings_chip(ui, &label, self.settings.sync_lead_ms == ms)
                                    {
                                        self.settings.sync_lead_ms = ms;
                                    }
                                }
                            },
                        );
                        if let (Some(bpm), Some(lead)) = (state.auto_bpm, state.auto_lead_ms) {
                            ui.horizontal(|ui| {
                                let (dot, _) = ui.allocate_exact_size(
                                    egui::vec2(7.0, 7.0),
                                    egui::Sense::hover(),
                                );
                                                                ui.painter()
                                        .circle_filled(dot.center(), 3.0, ui_theme_accent(ui));
                                    ui.add_space(2.0);
                                    let accent = ui_theme_accent(ui);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "≈ {bpm} BPM estimado · adiantando {lead} ms"
                                        ))
                                        .size(11.5)
                                        .color(accent),
                                    );
                            });
                            ui.add_space(8.0);
                        }
                        ui.add_space(16.0);
                        ui.label(kicker("DESEMPENHO"));
                        ui.add_space(10.0);
                        settings_toggle_row(
                            ui,
                            "Modo econômico",
                            "Lê a reprodução menos vezes em segundo plano.",
                            &mut self.settings.eco_mode,
                        );
                        ui.add_space(22.0);
                        ui.label(kicker("PRÉVIA DO STATUS"));
                        ui.add_space(8.0);
                        let has_lyric = has_active_lyric(state);
                        let sample = status_sample(state);
                        let emote_source = self.settings.emoji_for_lyric(has_lyric).to_owned();
                        let (emote_name, emote_id) = parse_emoji(&emote_source);
                        let emote_texture = if emote_id.is_some() {
                            if has_lyric {
                                self.emote_lyric_texture.clone()
                            } else {
                                self.emote_track_texture.clone()
                            }
                        } else {
                            None
                        };
                        status_preview(
                            ui,
                            emote_texture.as_ref(),
                            &emote_name.unwrap_or_default(),
                            &sample,
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(if has_lyric {
                                "Letra sincronizada com a música."
                            } else {
                                "Sem letra: título e artista."
                            })
                            .size(11.0)
                            .color(MUTED),
                        );
                    }
                    2 => {
                        settings_panel_title(
                            ui,
                            "Letras",
                            "Emojis e leitura das letras sincronizadas.",
                        );
                        ui.label(kicker("EMOJIS"));
                        ui.add_space(10.0);
                        emote_editor(
                            ui,
                            "emote-lyric",
                            "Letra da música",
                            &mut self.settings.lyric_emoji,
                            self.emote_lyric_texture.as_ref(),
                            has_active_lyric(state),
                        );
                        ui.add_space(14.0);
                        soft_divider(ui);
                        ui.add_space(14.0);
                        emote_editor(
                            ui,
                            "emote-track",
                            "Título e artista",
                            &mut self.settings.track_emoji,
                            self.emote_track_texture.as_ref(),
                            !has_active_lyric(state),
                        );
                        ui.add_space(16.0);
                        ui.label(kicker("EXIBIÇÃO"));
                        ui.add_space(10.0);
                        settings_toggle_row(
                            ui,
                            "Mostrar horário das linhas",
                            "Exibe o tempo ao lado de cada linha da letra.",
                            &mut self.settings.show_timestamps,
                        );
                        ui.add_space(10.0);
                        settings_toggle_row(
                            ui,
                            "Sincronizar status do Discord",
                            "Mostra a música como status personalizado no seu perfil do Discord.",
                            &mut self.settings.sync_discord_status,
                        );
                        ui.add_space(10.0);
                        settings_toggle_row(
                            ui,
                            "Sincronizar status do League",
                            "Atualiza a mensagem de status do cliente do LoL com a música.",
                            &mut self.settings.sync_league_status,
                        );
                        ui.add_space(10.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "Aceita emoji (🎵) ou emote de servidor no formato nome:123456789012345678. Emotes de servidor só aparecem se a sua conta puder usá-los.",
                                )
                                .size(11.5)
                                .color(MUTED),
                            )
                            .wrap(),
                        );
                    }
                    3 => {
                        settings_panel_title(
                            ui,
                            "Dados",
                            "Onde ficam suas preferências, o histórico e as capas.",
                        );
                        ui.label(kicker("ARQUIVO DE CONFIGURAÇÃO"));
                        ui.add_space(8.0);
                        let path_text = config_path().map(|path| path.display().to_string());
                        if let Some(path) = &path_text {
                            path_box(ui, path);
                        }
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            if ghost_button(ui, "Abrir pasta", 96.0) {
                                open_config_dir();
                            }
                            if ghost_button(ui, "Copiar caminho", 112.0) {
                                if let Some(path) = &path_text {
                                    ui.ctx().copy_text(path.clone());
                                }
                            }
                        });
                        ui.add_space(20.0);
                        ui.label(kicker("HISTÓRICO"));
                        ui.add_space(10.0);
                        settings_value_row(
                            ui,
                            "Tocadas recentemente",
                            format!("{} faixas", state.recent_tracks.len()),
                        );
                        settings_value_row(
                            ui,
                            "Capas em cache",
                            format!("{} arquivos", cover_cache_count()),
                        );
                        ui.add_space(20.0);
                        ui.label(kicker("PREFERÊNCIAS"));
                        ui.add_space(10.0);
                        settings_row(
                            ui,
                            "Restaurar preferências",
                            "Volta presença, emotes e sincronização para o padrão.",
                            |ui| {
                                if ghost_button(ui, "Restaurar", 92.0) {
                                    self.settings = Settings::default();
                                }
                            },
                        );
                        if self.settings != self.last_sent_settings {
                            ui.label(
                                egui::RichText::new("Salvando…").size(11.5).color(MUTED),
                            );
                        } else if self
                            .saved_flash
                            .map(|at| at.elapsed() < Duration::from_secs(3))
                            .unwrap_or(false)
                        {
                            ui.label(
                                egui::RichText::new("Preferências salvas")
                                    .size(11.5)
                                    .color(ui_theme_accent(ui)),
                            );
                        }
                        ui.add_space(10.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "O histórico de músicas e as capas em cache ficam na mesma pasta, em covers/.",
                                )
                                .size(11.5)
                                .color(MUTED),
                            )
                            .wrap(),
                        );
                    }
                    _ => {
                        settings_panel_title(
                            ui,
                            "Sobre",
                            "Status do Discord sincronizado com as letras do Spotify.",
                        );
                        ui.label(bold(APP_NAME, 22.0).color(TEXT));
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "Versão {APP_VERSION} · Windows SMTC · egui"
                            ))
                            .size(12.5)
                            .color(MUTED),
                        );
                        ui.add_space(18.0);
                        soft_divider(ui);
                        ui.add_space(14.0);
                        ui.label(kicker("ATUALIZAÇÕES"));
                        ui.add_space(10.0);
                        settings_toggle_row(
                            ui,
                            "Atualização automática",
                            "Verifica o GitHub a cada 6 horas e reinicia já na versão nova.",
                            &mut self.settings.auto_update,
                        );
                        settings_row(ui, "Status", "", |ui| {
                            ui.label(
                                egui::RichText::new(&state.update.message)
                                    .size(12.5)
                                    .color(if state.update.ready {
                                        ui_theme_accent(ui)
                                    } else {
                                        TEXT_DIM
                                    }),
                            );
                        });
                        ui.horizontal(|ui| {
                            if ghost_button(ui, "Verificar agora", 108.0) {
                                self.update_check.store(true, Ordering::Relaxed);
                                if let Ok(mut view) = self.state.lock() {
                                    view.update.message = "Verificando atualizações…".to_owned();
                                    view.update.ready = false;
                                }
                            }
                            if state.update.ready
                                && filled_button(ui, "Reiniciar e atualizar", 150.0, true)
                            {
                                match restart_to_apply_update() {
                                    Ok(()) => std::process::exit(0),
                                    Err(error) => {
                                        self.ui_error = Some(format!(
                                            "Não foi possível aplicar a atualização: {error}"
                                        ));
                                    }
                                }
                            }
                        });
                        ui.add_space(16.0);
                        soft_divider(ui);
                        ui.add_space(14.0);
                        ui.label(kicker("COMPONENTES"));
                        ui.add_space(10.0);
                        settings_value_row(
                            ui,
                            "Reprodução",
                            "Windows SMTC (controle de mídia do sistema)".to_owned(),
                        );
                        settings_value_row(ui, "Letras", "LRCLIB".to_owned());
                        settings_value_row(ui, "Capas", "Deezer e iTunes".to_owned());
                        settings_value_row(ui, "Interface", "egui + eframe".to_owned());
                        ui.add_space(20.0);
                        ui.label(kicker("ATALHOS"));
                        ui.add_space(10.0);
                        for (action, keys) in [
                            ("Reproduzir ou pausar", "Espaço"),
                            ("Voltar 5 segundos", "←"),
                            ("Avançar 5 segundos", "→"),
                            ("Abrir ou fechar as letras", "L"),
                            ("Abrir ou fechar o histórico", "H"),
                            ("Ir para a busca", "Ctrl + F"),
                            ("Limpar a busca", "Esc"),
                            ("Tela cheia", "F"),
                        ] {
                            settings_value_row(ui, action, keys.to_owned());
                        }
                        ui.add_space(12.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "Aviso: usar token de usuário do Discord pode violar os Termos de Serviço.",
                                )
                                .size(11.5)
                                .color(WARNING),
                            )
                            .wrap(),
                        );
                    }
                }
                    });
            });
        });
        self.settings_tab = selected;
        self.flush_settings();
    }
}

impl eframe::App for NativeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(200));
        let state = self
            .state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default();

        self.sync_avatar_texture(ctx, &state);
        self.sync_artwork_texture(ctx, &state);
        self.sync_emote_textures(ctx, &state);
        self.animate_theme(ctx);

        if state.account.is_none() && state.auth != AuthState::SignedIn {
            self.draw_login(ctx, &state);
            return;
        }

        self.handle_shortcuts(ctx, &state);

        let page_changed = self.last_page != self.page;
        self.last_page = self.page;

        self.draw_player_bar(ctx, &state);
        self.draw_sidebar(ctx, &state);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BLACK).inner_margin(egui::Margin {
                left: 0,
                right: 8,
                top: 8,
                bottom: 8,
            }))
            .show(ctx, |ui| {
                let panel_size = ui.available_size();
                egui::Frame::new()
                    .fill(BG)
                    .corner_radius(egui::CornerRadius::same(8))
                    .show(ui, |ui| {
                        ui.set_min_size(panel_size);
                        ui.set_clip_rect(ui.max_rect());
                        paint_spotify_header_gradient(
                            ui.painter(),
                            ui.max_rect(),
                            self.art_tint,
                        );
                        self.draw_header(ui, &state);
                        let mut content_scroll = egui::ScrollArea::vertical()
                            .id_salt("main-content")
                            .auto_shrink([false, false]);
                        if page_changed {
                            content_scroll = content_scroll.vertical_scroll_offset(0.0);
                        }
                        content_scroll.show(ui, |ui| {
                                egui::Frame::new()
                                    .inner_margin(egui::Margin {
                                        left: 24,
                                        right: 24,
                                        top: 0,
                                        bottom: 16,
                                    })
                                    .show(ui, |ui| {
                                        if let Some(error) =
                                            state.last_error.as_ref().or(self.ui_error.as_ref())
                                        {
                                            error_banner(ui, error);
                                            ui.add_space(12.0);
                                        }
                                        match self.page {
                                            Page::Settings => {
                                                ui.add_space(20.0);
                                                self.draw_settings(ui, &state);
                                            }
                                            Page::Lyrics => {
                                                ui.add_space(16.0);
                                                ui.label(bold("Letras", 32.0).color(TEXT));
                                                ui.add_space(16.0);
                                                self.draw_lyrics_page(ui, &state);
                                            }
                                            Page::History => self.draw_history_page(ui, &state),
                                            Page::Player => self.draw_home(ui, &state),
                                        }
                                        ui.add_space(16.0);
                                    });
                            });
                    });
            });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 860.0])
            .with_min_inner_size([940.0, 620.0])
            .with_title(APP_NAME)
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(NativeApp::new(cc)))),
    )
}

fn spawn_worker(
    state: Arc<Mutex<ViewState>>,
    command_rx: mpsc::Receiver<Command>,
    mut config: Config,
    mut token: Option<String>,
    mut settings: Settings,
    update_check: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        #[cfg(windows)]
        let _winrt_apartment = match WinRtApartment::initialize() {
            Ok(apartment) => apartment,
            Err(error) => {
                set_error(
                    &state,
                    format!("Não foi possível inicializar o WinRT: {error}"),
                );
                return;
            }
        };

        // The token is only ever sent to discord.com over HTTPS: the client
        // refuses plain HTTP and does not follow redirects, so the
        // `Authorization` header cannot be forwarded to another host.
        let client = match Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent(concat!(
                "SpotifySync/",
                env!("CARGO_PKG_VERSION"),
                " (native Rust)"
            ))
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                set_error(&state, format!("Não foi possível iniciar HTTP: {error}"));
                return;
            }
        };
        // The League client uses a localhost-only HTTPS API with a
        // self-signed certificate. It has its own isolated client: credentials
        // and the permissive certificate setting are never used for Discord.
        let league_status_tx = match Client::builder()
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(true)
            .build()
        {
            Ok(client) => Some(spawn_league_status_worker(client, state.clone())),
            Err(_) => None,
        };
        // The updater runs beside the playback loop, so a slow release
        // download never delays the Discord status.
        let auto_update = Arc::new(AtomicBool::new(settings.auto_update));
        spawn_updater(state.clone(), auto_update.clone(), update_check);
        let mut current_track_id = String::new();
        let mut recent_tracks = config.recent_tracks.clone();
        dedupe_history(&mut recent_tracks);
        let mut smtc_reader: Option<SmtcReader> = None;
        let mut lyrics = Vec::new();
        let mut lyrics_synced = false;
        let mut lyrics_cache: HashMap<String, (Vec<LyricLine>, &'static str)> = HashMap::new();
        let mut last_lyric_text = String::new();
        let mut lyric_index = None;
        let mut lyrics_attempts = 0u32;
        let mut next_lyrics_retry: Option<Instant> = None;
        let mut smtc_error_reported = false;
        let mut last_status_text = String::new();
        let mut last_status_sent_at = Instant::now() - Duration::from_secs(10);
        let mut next_status_attempt = Instant::now();
        let mut last_league_status_text = String::new();
        // Adaptive gap between updates: tight while Discord accepts them, backed
        // off as soon as it throttles.
        let mut status_interval = Duration::from_millis(900);
        let mut cached_lyric_emote_id: Option<String> = None;
        let mut cached_track_emote_id: Option<String> = None;
        let mut clock = PlaybackClock::new();
        let mut cover_attempts: HashSet<String> = HashSet::new();
        let mut next_cover_backfill = Instant::now() + Duration::from_secs(2);

        if let Some(saved_token) = token.as_deref() {
            match validate_token(&client, saved_token) {
                Ok(account) => {
                    config.account_id = Some(account.id.clone());
                    config.account_name = Some(account.name.clone());
                    config.account_avatar = account.avatar_url.clone();
                    let _ = save_config(&config);
                    let avatar = account
                        .avatar_url
                        .as_deref()
                        .and_then(|url| fetch_bytes(&client, url));
                    if let Ok(mut view) = state.lock() {
                        view.token_status = "Conectado ao Discord".to_owned();
                        view.account = Some(account);
                        view.avatar = avatar;
                        view.auth = AuthState::SignedIn;
                        view.discord_status = "connected".to_owned();
                    }
                }
                Err(error) => {
                    if let Ok(mut view) = state.lock() {
                        view.token_status =
                            format!("Não foi possível validar a sessão salva: {error}");
                        if view.account.is_none() {
                            view.auth = AuthState::Failed;
                        }
                        view.discord_status = "no-token".to_owned();
                    }
                }
            }
        }

        loop {
            while let Ok(command) = command_rx.try_recv() {
                match command {
                    Command::SaveToken(mut candidate) => {
                        set_token_status(&state, "Validando token com o Discord…".to_owned());
                        if let Ok(mut view) = state.lock() {
                            view.auth = AuthState::Validating;
                        }
                        match validate_token(&client, &candidate) {
                            Ok(account) => {
                                token = Some(candidate.clone());
                                // The token never goes into the JSON config: it is
                                // encrypted with Windows DPAPI in its own file.
                                let token_saved = save_local_token(&candidate);
                                wipe_secret(&mut candidate);
                                config.user_token = None;
                                config.account_id = Some(account.id.clone());
                                config.account_name = Some(account.name.clone());
                                config.account_avatar = account.avatar_url.clone();
                                let avatar = account
                                    .avatar_url
                                    .as_deref()
                                    .and_then(|url| fetch_bytes(&client, url));
                                if let Err(error) = token_saved {
                                    set_error(
                                        &state,
                                        format!(
                                            "Sessão iniciada, mas o token não pôde ser guardado com segurança: {error}"
                                        ),
                                    );
                                } else if let Err(error) = save_config(&config) {
                                    set_error(
                                        &state,
                                        format!(
                                            "Sessão iniciada, mas não foi possível salvar: {error}"
                                        ),
                                    );
                                }
                                if let Ok(mut view) = state.lock() {
                                    view.token_status = "Conectado ao Discord".to_owned();
                                    view.account = Some(account);
                                    view.avatar = avatar;
                                    view.auth = AuthState::SignedIn;
                                    view.discord_status = "connected".to_owned();
                                    view.last_error = None;
                                }
                            }
                            Err(error) => {
                                if let Ok(mut view) = state.lock() {
                                    view.auth = AuthState::Failed;
                                }
                                set_token_status(&state, format!("Falha na validação: {error}"));
                            }
                        }
                    }
                    Command::SignOut => {
                        if let Some(mut current) = token.take() {
                            let _ =
                                clear_discord_status(&client, &current, settings.presence.as_str());
                            wipe_secret(&mut current);
                        }
                        clear_local_token();
                        config.user_token = None;
                        config.account_id = None;
                        config.account_name = None;
                        config.account_avatar = None;
                        last_status_text.clear();
                        let _ = save_config(&config);
                        if let Ok(mut view) = state.lock() {
                            view.account = None;
                            view.avatar = None;
                            view.auth = AuthState::SignedOut;
                            view.token_status = "Sessão encerrada".to_owned();
                            view.discord_status = "no-token".to_owned();
                            view.last_error = None;
                        }
                    }
                    Command::Seek(position_ms) => {
                        if let Err(error) = seek_smtc(position_ms) {
                            set_error(&state, error);
                        }
                    }
                    Command::Media(action) => {
                        if let Err(error) = control_smtc(action) {
                            set_error(&state, error);
                        }
                    }
                    Command::UpdateSettings(next) => {
                        let presence_changed = settings.presence != next.presence;
                        let status_appearance_changed = settings.track_emoji != next.track_emoji
                            || settings.lyric_emoji != next.lyric_emoji
                            || settings.sync_lead_ms != next.sync_lead_ms;
                        let discord_sync_changed =
                            settings.sync_discord_status != next.sync_discord_status;
                        let league_sync_disabled =
                            settings.sync_league_status && !next.sync_league_status;
                        let league_sync_changed =
                            settings.sync_league_status != next.sync_league_status;
                        settings = next;
                        auto_update.store(settings.auto_update, Ordering::Relaxed);
                        settings.apply(&mut config);
                        if presence_changed || status_appearance_changed {
                            last_status_text.clear();
                            last_league_status_text.clear();
                        }
                        if discord_sync_changed {
                            if settings.sync_discord_status {
                                // Force the current track to be published again
                                // immediately after the Discord sync is enabled.
                                last_status_text.clear();
                                next_status_attempt = Instant::now();
                            } else if let Some(token) = token.as_deref() {
                                if Instant::now() >= next_status_attempt {
                                    match clear_discord_status(
                                        &client,
                                        token,
                                        settings.presence.as_str(),
                                    ) {
                                        Ok(()) => {
                                            last_status_text.clear();
                                            set_discord_status(&state, "connected".to_owned());
                                        }
                                        Err(error) => {
                                            next_status_attempt = Instant::now() + error.pause();
                                            set_discord_status(
                                                &state,
                                                format!("Erro: {error}"),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        if league_sync_disabled {
                            if !last_league_status_text.is_empty() {
                                if let Some(league) = league_status_tx.as_ref() {
                                    if send_league_status(league, &state, String::new()) {
                                        last_league_status_text.clear();
                                    }
                                }
                            }
                        }
                        if league_sync_changed && settings.sync_league_status {
                            // Publish the current status as soon as League sync
                            // is enabled instead of waiting for a track change.
                            last_league_status_text.clear();
                        }
                        if let Err(error) = save_config(&config) {
                            set_error(
                                &state,
                                format!("Falha ao salvar configurações: {error}"),
                            );
                        }
                    }
                }
            }

            let lyric_emote_id = parse_emoji(&settings.lyric_emoji).1;
            if lyric_emote_id != cached_lyric_emote_id {
                let bytes = lyric_emote_id.as_deref().and_then(|id| {
                    fetch_bytes(
                        &client,
                        &format!("https://cdn.discordapp.com/emojis/{id}.png?size=64"),
                    )
                });
                cached_lyric_emote_id = lyric_emote_id;
                if let Ok(mut view) = state.lock() {
                    view.emote_lyric = bytes;
                }
            }
            let track_emote_id = parse_emoji(&settings.track_emoji).1;
            if track_emote_id != cached_track_emote_id {
                let bytes = track_emote_id.as_deref().and_then(|id| {
                    fetch_bytes(
                        &client,
                        &format!("https://cdn.discordapp.com/emojis/{id}.png?size=64"),
                    )
                });
                cached_track_emote_id = track_emote_id;
                if let Ok(mut view) = state.lock() {
                    view.emote_track = bytes;
                }
            }

            let smtc_result = if let Some(reader) = smtc_reader.as_ref() {
                reader.read_track()
            } else {
                match SmtcReader::connect() {
                    Ok(reader) => {
                        let result = reader.read_track();
                        smtc_reader = Some(reader);
                        result
                    }
                    Err(error) => Err(error),
                }
            };
            if smtc_result.is_err() {
                smtc_reader = None;
            }

            match smtc_result {
                Ok(track) => {
                    smtc_error_reported = false;
                    if let Some(track) = track {
                        let id = track.id();
                        if id != current_track_id {
                            current_track_id = id.clone();
                            record_history(&mut recent_tracks, &track, now_ms() / 1000);
                            config.recent_tracks = recent_tracks.clone();
                            let history_save_error = save_config(&config).err();
                            if let Ok(mut view) = state.lock() {
                                view.recent_tracks = recent_tracks.clone();
                            }
                            clock.reset();
                            lyrics.clear();
                            lyric_index = None;
                            lyrics_attempts = 0;
                            last_lyric_text.clear();
                            last_status_text.clear();
                            if let Ok(mut view) = state.lock() {
                                view.track = Some(track.clone());
                                view.lyrics.clear();
                                view.lyric_index = None;
                                view.auto_bpm = None;
                                view.auto_lead_ms = None;
                                view.artwork = None;
                                view.lyrics_status = "Buscando letras…".to_owned();
                                view.last_error = None;
                            }
                            let (found, source) = match lyrics_cache.get(&id).cloned() {
                                Some(cached) => cached,
                                None => {
                                    let (found, source) = fetch_lyrics(&client, &track);
                                    if !found.is_empty() {
                                        if lyrics_cache.len() >= 48 {
                                            lyrics_cache.clear();
                                        }
                                        lyrics_cache.insert(id.clone(), (found.clone(), source));
                                    }
                                    (found, source)
                                }
                            };
                            lyrics = found;
                            lyrics_synced = source == "LRCLIB sincronizadas";
                            next_lyrics_retry = if lyrics.is_empty() {
                                Some(Instant::now() + Duration::from_secs(5))
                            } else {
                                None
                            };
                            let artwork = fetch_artwork(&client, &track);
                            let mut covers_changed = false;
                            if let Some(bytes) = artwork.as_deref() {
                                if let Some(cover) =
                                    cache_history_cover(&track.title, &track.artist, bytes)
                                {
                                    covers_changed = attach_cover(
                                        &mut recent_tracks,
                                        &track.title,
                                        &track.artist,
                                        &cover,
                                    );
                                }
                            }
                            if covers_changed {
                                config.recent_tracks = recent_tracks.clone();
                                let _ = save_config(&config);
                            }
                            if let Ok(mut view) = state.lock() {
                                view.lyrics = lyrics.clone();
                                view.lyrics_status = if lyrics.is_empty() {
                                    "Não encontradas".to_owned()
                                } else {
                                    format!("{} linhas ({source})", lyrics.len())
                                };
                                view.artwork = artwork;
                                if covers_changed {
                                    view.recent_tracks = recent_tracks.clone();
                                }
                            }
                            if let Some(error) = history_save_error {
                                set_error(
                                    &state,
                                    format!("Falha ao salvar o histórico de músicas: {error}"),
                                );
                            }
                        } else if let Ok(mut view) = state.lock() {
                            view.track = Some(track.clone());
                        }

                        // Network hiccups or a late LRCLIB index update can leave a
                        // track without lyrics; retry a few times instead of
                        // keeping the status stuck on the title.
                        if lyrics.is_empty() && lyrics_attempts < 4 {
                            let due = next_lyrics_retry.unwrap_or_else(Instant::now);
                            if Instant::now() >= due {
                                lyrics_attempts += 1;
                                let (found, source) = fetch_lyrics(&client, &track);
                                if found.is_empty() {
                                    next_lyrics_retry =
                                        Some(Instant::now() + Duration::from_secs(20));
                                } else {
                                    lyrics = found;
                                    lyrics_synced = source == "LRCLIB sincronizadas";
                                    next_lyrics_retry = None;
                                    if let Ok(mut view) = state.lock() {
                                        view.lyrics = lyrics.clone();
                                        view.lyrics_status =
                                            format!("{} linhas ({source})", lyrics.len());
                                    }
                                }
                            }
                        }

                        let (position, seek) = clock.position(track.position_ms, track.is_playing);
                        // Estimate a tempo from synced timestamps and add a
                        // quarter beat to compensate for Discord update latency.
                        let auto_bpm = lyric_cadence_bpm(&lyrics, lyrics_synced);
                        let base_lead_ms = effective_lead_ms(settings.sync_lead_ms, auto_bpm);
                        // A custom status can only change once per `status_interval`,
                        // so the lyric is read a little ahead: Discord then shows the
                        // line being sung instead of trailing a beat behind.
                        let lead_ms = if base_lead_ms == 0 {
                            0
                        } else {
                            base_lead_ms
                                .saturating_add((status_interval.as_millis() as u16 / 2).min(400))
                        };
                        let lead_ms = cap_lyric_lookahead_ms(&lyrics, lead_ms);
                        if let Ok(mut view) = state.lock() {
                            if view.auto_bpm != auto_bpm || view.auto_lead_ms != Some(lead_ms) {
                                view.auto_bpm = auto_bpm;
                                view.auto_lead_ms = Some(lead_ms);
                            }
                        }
                        let candidate_index =
                            active_lyric(&lyrics, position.saturating_add(lead_ms as u64));
                        // While the track plays the lyric line only moves
                        // forward: a late or frozen SMTC read must never bring
                        // the previous line back. Only a real seek may rewind.
                        let active_index = if track.is_playing && !seek {
                            hold_lyric_floor(lyric_index, candidate_index)
                        } else {
                            candidate_index
                        };
                        if lyric_index != active_index {
                            lyric_index = active_index;
                            if let Ok(mut view) = state.lock() {
                                view.lyric_index = active_index;
                            }
                        }

                        let (status_text, has_lyric) = if track.is_playing {
                            let current_line = active_index
                                .and_then(|index| lyrics.get(index))
                                .map(|line| line.text.trim())
                                .filter(|text| !text.is_empty());
                            if let Some(text) = current_line {
                                last_lyric_text = text.to_owned();
                            }
                            let lyric_text = current_line.map(ToOwned::to_owned).or_else(|| {
                                (!lyrics.is_empty() && !last_lyric_text.is_empty())
                                    .then(|| last_lyric_text.clone())
                            });
                            match lyric_text {
                                Some(text) => (text, true),
                                None => (format!("{} — {}", track.title, track.artist), false),
                            }
                        } else {
                            (String::new(), false)
                        };

                        if settings.sync_league_status {
                            if track.is_playing {
                                if status_text != last_league_status_text {
                                    if let Some(league) = league_status_tx.as_ref() {
                                        if send_league_status(league, &state, status_text.clone()) {
                                            last_league_status_text = status_text.clone();
                                        }
                                    }
                                }
                            } else if !last_league_status_text.is_empty() {
                                if let Some(league) = league_status_tx.as_ref() {
                                    if send_league_status(league, &state, String::new()) {
                                        last_league_status_text.clear();
                                    }
                                }
                            }
                        }

                        if let Some(token) = token.as_deref() {
                            if !settings.sync_discord_status {
                                // Turning the sync off removes the custom status the
                                // app was showing instead of leaving it frozen.
                                if !last_status_text.is_empty()
                                    && Instant::now() >= next_status_attempt
                                {
                                    match clear_discord_status(
                                        &client,
                                        token,
                                        settings.presence.as_str(),
                                    ) {
                                        Ok(()) => {
                                            last_status_text.clear();
                                            set_discord_status(&state, "connected".to_owned());
                                        }
                                        Err(error) => {
                                            next_status_attempt =
                                                Instant::now() + error.pause();
                                            set_discord_status(
                                                &state,
                                                format!("Erro: {error}"),
                                            );
                                        }
                                    }
                                }
                            } else if track.is_playing {
                                let ready = Instant::now() >= next_status_attempt
                                    && last_status_sent_at.elapsed() >= status_interval;
                                if status_text != last_status_text && ready {
                                    if let Err(error) = update_discord_status(
                                        &client,
                                        token,
                                        &status_text,
                                        &settings.presence,
                                        settings.emoji_for_lyric(has_lyric),
                                    ) {
                                        // Discord throttles custom-status edits;
                                        // wait exactly what it asked for instead of
                                        // freezing the sync for a fixed pause.
                                        if error.retry_after.is_some() {
                                            status_interval = (status_interval
                                                + Duration::from_millis(350))
                                            .min(Duration::from_secs(3));
                                        }
                                        next_status_attempt = Instant::now() + error.pause();
                                        set_discord_status(&state, format!("Erro: {error}"));
                                    } else {
                                        last_status_text = status_text;
                                        last_status_sent_at = Instant::now();
                                        // Climb back only slowly: dropping straight
                                        // to a few hundred milliseconds is what made
                                        // Discord throttle and the status stutter.
                                        status_interval = status_interval
                                            .saturating_sub(Duration::from_millis(60))
                                            .max(Duration::from_millis(850));
                                        set_discord_status(&state, "connected".to_owned());
                                    }
                                }
                            } else if !last_status_text.is_empty()
                                && Instant::now() >= next_status_attempt
                            {
                                match clear_discord_status(
                                    &client,
                                    token,
                                    settings.presence.as_str(),
                                ) {
                                    Ok(()) => {
                                        last_status_text.clear();
                                        set_discord_status(&state, "connected".to_owned());
                                    }
                                    Err(error) => {
                                        next_status_attempt = Instant::now() + error.pause();
                                        set_discord_status(&state, format!("Erro: {error}"));
                                    }
                                }
                            }
                        }
                    } else {
                        if settings.sync_league_status && !last_league_status_text.is_empty() {
                            if let Some(league) = league_status_tx.as_ref() {
                                if send_league_status(league, &state, String::new()) {
                                    last_league_status_text.clear();
                                }
                            }
                        }
                        if let Some(token) = token.as_deref() {
                            if !last_status_text.is_empty()
                                && Instant::now() >= next_status_attempt
                            {
                                match clear_discord_status(
                                    &client,
                                    token,
                                    settings.presence.as_str(),
                                ) {
                                    Ok(()) => {
                                        last_status_text.clear();
                                        set_discord_status(&state, "connected".to_owned());
                                    }
                                    Err(error) => {
                                        next_status_attempt = Instant::now() + error.pause();
                                        set_discord_status(&state, format!("Erro: {error}"));
                                    }
                                }
                            }
                        }
                        current_track_id.clear();
                        clock.reset();
                        lyrics.clear();
                        last_lyric_text.clear();
                        lyric_index = None;
                        lyrics_attempts = 0;
                        next_lyrics_retry = None;
                        if let Ok(mut view) = state.lock() {
                            view.track = None;
                            view.lyrics.clear();
                            view.lyric_index = None;
                            view.auto_bpm = None;
                            view.auto_lead_ms = None;
                            view.artwork = None;
                            view.lyrics_status = "Aguardando música".to_owned();
                        }
                    }
                }
                Err(error) => {
                    if !smtc_error_reported {
                        set_error(&state, error);
                        smtc_error_reported = true;
                    }
                }
            }

            // Older history entries have no cached cover yet: fetch them one
            // at a time so the list shows the real album art.
            if Instant::now() >= next_cover_backfill {
                next_cover_backfill = Instant::now() + Duration::from_millis(1_200);
                let candidate = recent_tracks
                    .iter()
                    .find(|entry| {
                        entry.cover.is_none()
                            && !cover_attempts.contains(&format!(
                                "{}::{}",
                                entry.title.to_lowercase(),
                                entry.artist.to_lowercase()
                            ))
                    })
                    .map(|entry| {
                        (
                            entry.title.clone(),
                            entry.artist.clone(),
                            entry.album.clone(),
                            entry.duration_ms,
                        )
                    });
                if let Some((title, artist, album, duration_ms)) = candidate {
                    cover_attempts.insert(format!(
                        "{}::{}",
                        title.to_lowercase(),
                        artist.to_lowercase()
                    ));
                    let probe = Track {
                        title,
                        artist,
                        album,
                        duration_ms,
                        ..Track::default()
                    };
                    if let Some(bytes) = fetch_artwork(&client, &probe) {
                        if let Some(cover) =
                            cache_history_cover(&probe.title, &probe.artist, &bytes)
                        {
                            if attach_cover(
                                &mut recent_tracks,
                                &probe.title,
                                &probe.artist,
                                &cover,
                            ) {
                                config.recent_tracks = recent_tracks.clone();
                                let _ = save_config(&config);
                                if let Ok(mut view) = state.lock() {
                                    view.recent_tracks = recent_tracks.clone();
                                }
                            }
                        }
                    }
                }
            }

            let delay = if settings.eco_mode { 2000 } else { 250 };
            thread::sleep(Duration::from_millis(delay));
        }
    });
}

/// True when `candidate` (for example `v1.8.0`) is newer than `current`.
fn is_newer_version(candidate: &str, current: &str) -> bool {
    let candidate = version_numbers(candidate);
    let current = version_numbers(current);
    if candidate.is_empty() {
        return false;
    }
    for index in 0..candidate.len().max(current.len()) {
        let left = candidate.get(index).copied().unwrap_or(0);
        let right = current.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

fn version_numbers(value: &str) -> Vec<u64> {
    value
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u64>().ok())
        .collect()
}

/// API endpoint of the repository's latest published release.
fn latest_release_endpoint() -> Option<String> {
    let repository = UPDATE_REPOSITORY.trim().trim_end_matches('/');
    let slug = repository
        .strip_prefix("https://github.com/")
        .or_else(|| repository.strip_prefix("http://github.com/"))
        .unwrap_or(repository)
        .trim_end_matches(".git")
        .trim_end_matches('/');
    (!slug.is_empty()).then(|| format!("https://api.github.com/repos/{slug}/releases/latest"))
}

/// Where the updater asks for the newest release. `SPOTIFYSYNC_UPDATE_FEED`
/// wins, then the `updateFeed` key in the configuration file, then the GitHub
/// API of the repository the app was built from.
fn update_feed_url() -> Option<String> {
    let configured = std::env::var("SPOTIFYSYNC_UPDATE_FEED")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            load_config()
                .update_feed
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        });
    configured.or_else(latest_release_endpoint)
}

/// A feed on this machine may be served over plain HTTP so the updater can be
/// exercised locally; every remote feed still requires TLS.
fn is_loopback_url(url: &str) -> bool {
    ["http://127.0.0.1", "http://localhost", "http://[::1]"]
        .iter()
        .any(|prefix| url.starts_with(prefix))
}

/// Reads the newest version and its executable link from either a GitHub
/// release payload (`tag_name` + `assets`) or a small manifest such as
/// `{ "version": "1.8.0", "url": "https://…/SpotifySync.exe" }`.
fn parse_release(value: &serde_json::Value) -> Option<(String, Option<String>)> {
    if let Some(version) = value["version"].as_str() {
        let version = version.trim().trim_start_matches(['v', 'V']).to_owned();
        let url = ["url", "downloadUrl", "download_url"]
            .iter()
            .find_map(|key| value[*key].as_str())
            .map(ToOwned::to_owned);
        return (!version.is_empty()).then_some((version, url));
    }
    let version = value["tag_name"]
        .as_str()?
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_owned();
    if version.is_empty() {
        return None;
    }
    let url = value["assets"].as_array().and_then(|assets| {
        assets.iter().find_map(|asset| {
            let name = asset["name"].as_str()?;
            let url = asset["browser_download_url"].as_str()?;
            name.eq_ignore_ascii_case(UPDATE_ASSET_NAME)
                .then(|| url.to_owned())
        })
    });
    Some((version, url))
}

fn update_dir() -> Option<PathBuf> {
    Some(config_dir()?.join("update"))
}

fn pending_update_path() -> Option<PathBuf> {
    Some(update_dir()?.join(UPDATE_FILE_NAME))
}

/// Marker written right before the app restarts to install a version. If it is
/// still there on the next check, the replacement failed (a read-only folder,
/// for example) and the app must not restart on the same version again.
fn update_attempt_path(version: &str) -> Option<PathBuf> {
    Some(update_dir()?.join(format!("attempted-{version}")))
}

fn set_update(state: &Arc<Mutex<ViewState>>, message: impl Into<String>, ready: bool) {
    if let Ok(mut view) = state.lock() {
        view.update.message = message.into();
        view.update.ready = ready;
    }
}

/// Checks the newest release and, when it is newer, downloads its executable.
/// In release builds with automatic updates enabled the app then restarts
/// already running the new version.
fn spawn_updater(
    state: Arc<Mutex<ViewState>>,
    auto_update: Arc<AtomicBool>,
    check_request: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        // Let the window open and the session load before the first request.
        thread::sleep(Duration::from_secs(4));
        loop {
            run_update_check(&state, auto_update.load(Ordering::Relaxed));
            let deadline = Instant::now() + UPDATE_INTERVAL;
            loop {
                if check_request.swap(false, Ordering::Relaxed) {
                    break;
                }
                if Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_secs(5));
            }
        }
    });
}

fn run_update_check(state: &Arc<Mutex<ViewState>>, auto_install: bool) {
    let Some(endpoint) = update_feed_url() else {
        set_update(state, "Repositório de atualizações não configurado", false);
        return;
    };
    let client = match Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent(concat!(
            "SpotifySync/",
            env!("CARGO_PKG_VERSION"),
            " updater"
        ))
        .https_only(!is_loopback_url(&endpoint))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            set_update(
                state,
                "Não foi possível iniciar a verificação de atualizações",
                false,
            );
            return;
        }
    };
    set_update(state, "Verificando atualizações…", false);
    let response = match client
        .get(&endpoint)
        .header("Accept", "application/vnd.github+json")
        .send()
    {
        Ok(response) => response,
        Err(_) => {
            set_update(state, "Não foi possível verificar atualizações agora", false);
            return;
        }
    };
    if matches!(response.status().as_u16(), 403 | 429) {
        set_update(
            state,
            "Muitas verificações em pouco tempo. Tento mais tarde",
            false,
        );
        return;
    }
    if !response.status().is_success() {
        set_update(
            state,
            format!(
                "Servidor de atualizações respondeu HTTP {} — confira se o repositório é público",
                response.status()
            ),
            false,
        );
        return;
    }
    let release: serde_json::Value = match response.json() {
        Ok(release) => release,
        Err(_) => {
            set_update(state, "Resposta inválida do servidor de atualizações", false);
            return;
        }
    };
    let Some((version, download_url)) = parse_release(&release) else {
        set_update(state, "Release sem tag de versão", false);
        return;
    };
    if !is_newer_version(&version, APP_VERSION) {
        set_update(
            state,
            format!("Você está na versão mais recente (v{APP_VERSION})"),
            false,
        );
        return;
    }
    let Some(download_url) = download_url else {
        set_update(
            state,
            format!("Versão v{version} sem executável no release"),
            false,
        );
        return;
    };
    let attempt_path = update_attempt_path(&version);
    if attempt_path
        .as_deref()
        .map(|path| path.is_file())
        .unwrap_or(false)
    {
        // A previous restart already tried to install this same version: ask
        // the user to reopen the app instead of looping forever.
        set_update(
            state,
            format!("Atualização v{version} pronta — feche e reabra o app para aplicar"),
            true,
        );
        return;
    }
    set_update(state, format!("Baixando atualização v{version}…"), false);
    let downloaded = client
        .get(&download_url)
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.bytes());
    let bytes = match downloaded {
        Ok(bytes) => bytes,
        Err(_) => {
            set_update(state, format!("Falha ao baixar a atualização v{version}"), false);
            return;
        }
    };
    // A Windows executable starts with the `MZ` signature: refuse an HTML
    // error page or a truncated download instead of replacing the app.
    if bytes.len() < 1024 || !bytes.starts_with(b"MZ") {
        set_update(state, format!("Download da atualização v{version} inválido"), false);
        return;
    }
    if let Err(error) = write_pending_update(&bytes) {
        set_update(
            state,
            format!("Não foi possível guardar a atualização: {error}"),
            false,
        );
        return;
    }
    set_update(state, format!("Atualização v{version} baixada"), true);
    if auto_install && !cfg!(debug_assertions) {
        if let Some(path) = attempt_path.as_deref() {
            let _ = fs::write(path, b"");
        }
        match restart_to_apply_update() {
            Ok(()) => std::process::exit(0),
            Err(error) => {
                if let Some(path) = attempt_path.as_deref() {
                    let _ = fs::remove_file(path);
                }
                set_update(
                    state,
                    format!("Atualização pronta, mas não foi possível reiniciar: {error}"),
                    true,
                );
            }
        }
    }
}

fn write_pending_update(bytes: &[u8]) -> Result<(), String> {
    let path =
        pending_update_path().ok_or_else(|| "Pasta de atualização indisponível".to_owned())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Caminho de atualização inválido".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("download");
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    let _ = fs::remove_file(&path);
    fs::rename(&temporary, &path).map_err(|error| error.to_string())
}

/// Hands the downloaded executable to a helper script that waits for this
/// process to exit, replaces the running `.exe` and opens the new version.
#[cfg(windows)]
fn restart_to_apply_update() -> Result<(), String> {
    let source = pending_update_path()
        .filter(|path| path.is_file())
        .ok_or_else(|| "Nenhuma atualização baixada".to_owned())?;
    let target = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = update_dir().ok_or_else(|| "Pasta de atualização indisponível".to_owned())?;
    let script = directory.join("apply-update.ps1");
    let log = directory.join("apply.log");
    // The old executable can stay locked for a moment after the process
    // exits, so the move is retried instead of failing on the first try. The
    // script always reopens the app, even when the replacement failed, so the
    // user is never left without the program.
    let body = format!(
        "$log = '{log}'\r\n\
         function Write-Log($text) {{ \"$(Get-Date -Format o) $text\" | Out-File -LiteralPath $log -Append -Encoding utf8 }}\r\n\
         Write-Log 'aguardando o app fechar'\r\n\
         Wait-Process -Id {pid} -ErrorAction SilentlyContinue\r\n\
         $moved = $false\r\n\
         for ($i = 0; $i -lt 40; $i++) {{\r\n\
             Start-Sleep -Milliseconds 250\r\n\
             Move-Item -Force -LiteralPath '{source}' -Destination '{target}' -ErrorAction SilentlyContinue\r\n\
             if (-not (Test-Path -LiteralPath '{source}')) {{ $moved = $true; break }}\r\n\
         }}\r\n\
         if ($moved) {{ Write-Log 'executável substituído' }} else {{ Write-Log 'falha ao substituir o executável' }}\r\n\
         Start-Process -FilePath '{target}'\r\n\
         Remove-Item -LiteralPath $MyInvocation.MyCommand.Path -Force -ErrorAction SilentlyContinue\r\n",
        pid = std::process::id(),
        log = powershell_literal(&log.display().to_string()),
        source = powershell_literal(&source.display().to_string()),
        target = powershell_literal(&target.display().to_string()),
    );
    fs::write(&script, body).map_err(|error| error.to_string())?;
    use std::os::windows::process::CommandExt;
    std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(not(windows))]
fn restart_to_apply_update() -> Result<(), String> {
    Err("A atualização automática só está disponível no Windows 10/11".to_owned())
}

#[cfg(windows)]
fn powershell_literal(value: &str) -> String {
    value.replace('\'', "''")
}

fn load_config() -> Config {
    config_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn configured_token() -> Option<String> {
    std::env::var("DISCORD_USER_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
        .or_else(|| {
            fs::read_to_string(".env")
                .ok()
                .and_then(|dotenv| parse_dotenv_token(&dotenv))
        })
}

fn parse_dotenv_token(dotenv: &str) -> Option<String> {
    dotenv.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        if key.trim() != "DISCORD_USER_TOKEN" {
            return None;
        }
        let token = value.trim().trim_matches('"').trim_matches('\'').trim();
        (!token.is_empty()).then(|| token.to_owned())
    })
}

const TOKEN_FILE: &str = "token.bin";
/// `CRYPTPROTECT_UI_FORBIDDEN`: never show a Windows prompt for the encryption.
const CRYPT_UI_FORBIDDEN: u32 = 0x1;

fn token_path() -> Option<PathBuf> {
    Some(config_dir()?.join(TOKEN_FILE))
}

/// Reads the token from the encrypted local store. The blob is encrypted with
/// Windows DPAPI, so only this Windows user on this machine can decrypt it.
fn load_local_token() -> Option<String> {
    let blob = fs::read(token_path()?).ok()?;
    let plain = unprotect_secret(&blob).ok()?;
    let token = String::from_utf8(plain).ok()?.trim().to_owned();
    (!token.is_empty()).then_some(token)
}

fn save_local_token(token: &str) -> Result<(), String> {
    let path = token_path().ok_or_else(|| "Pasta de configuração indisponível".to_owned())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Caminho de configuração inválido".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let blob = protect_secret(token.trim().as_bytes())?;
    // Write then rename so an interrupted save never truncates the token.
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, &blob).map_err(|error| error.to_string())?;
    let _ = fs::remove_file(&path);
    fs::rename(&temporary, &path).map_err(|error| error.to_string())
}

fn clear_local_token() {
    let Some(path) = token_path() else {
        return;
    };
    if let Ok(bytes) = fs::read(&path) {
        // Best effort: overwrite before deleting so the blob does not survive a
        // simple undelete.
        let zeros = vec![0u8; bytes.len()];
        let _ = fs::write(&path, zeros);
    }
    let _ = fs::remove_file(&path);
}

/// Older versions stored the token in plain text inside `config.json`. Move it to
/// the encrypted store and drop it from the JSON.
fn migrate_legacy_token(config: &mut Config) {
    let Some(legacy) = config.user_token.take() else {
        return;
    };
    if save_local_token(&legacy).is_ok() {
        let _ = save_config(config);
    }
}

/// Overwrites a secret in memory before the buffer is released.
fn wipe_secret(value: &mut String) {
    // SAFETY: zeroed bytes are valid UTF-8 and the length is cleared right after.
    unsafe {
        value.as_mut_vec().fill(0);
    }
    value.clear();
}

#[cfg(windows)]
fn protect_secret(plain: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    unsafe {
        CryptProtectData(
            &input,
            windows::core::PCWSTR::null(),
            None,
            None,
            None,
            CRYPT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|error| format!("DPAPI: {error}"))?;
        let blob = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        if !output.pbData.is_null() {
            let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
        }
        Ok(blob)
    }
}

#[cfg(windows)]
fn unprotect_secret(blob: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB {
        cbData: blob.len() as u32,
        pbData: blob.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|error| format!("DPAPI: {error}"))?;
        let plain = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        if !output.pbData.is_null() {
            let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
        }
        Ok(plain)
    }
}

#[cfg(not(windows))]
fn protect_secret(_plain: &[u8]) -> Result<Vec<u8>, String> {
    Err(
        "O armazenamento seguro do token usa o Windows DPAPI e só existe no Windows 10/11."
            .to_owned(),
    )
}

#[cfg(not(windows))]
fn unprotect_secret(_blob: &[u8]) -> Result<Vec<u8>, String> {
    Err(
        "O armazenamento seguro do token usa o Windows DPAPI e só existe no Windows 10/11."
            .to_owned(),
    )
}

fn save_config(config: &Config) -> Result<(), String> {
    let path = config_path().ok_or_else(|| "Pasta de configuração indisponível".to_owned())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Caminho de configuração inválido".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let json = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, json).map_err(|error| error.to_string())?;
    let _ = fs::remove_file(&path);
    fs::rename(&temporary, &path).map_err(|error| error.to_string())
}

fn config_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join(LEGACY_CONFIG_DIR))
}

fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("config.json"))
}

fn open_config_dir() {
    let Some(dir) = config_dir() else {
        return;
    };
    let _ = fs::create_dir_all(&dir);
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
    }
}

fn validate_token(client: &Client, token: &str) -> Result<Account, String> {
    let response = client
        .get(DISCORD_ME_URL)
        .header("Authorization", token)
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Discord retornou HTTP {}", response.status()));
    }
    let user: serde_json::Value = response.json().map_err(|error| error.to_string())?;
    let id = user["id"]
        .as_str()
        .ok_or_else(|| "Resposta do Discord sem identificador de usuário".to_owned())?
        .to_owned();
    let name = user["global_name"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| user["username"].as_str())
        .unwrap_or("Conta do Discord")
        .to_owned();
    let avatar_url = user["avatar"]
        .as_str()
        .map(|hash| format!("https://cdn.discordapp.com/avatars/{id}/{hash}.png?size=128"))
        .or_else(|| {
            let index = user["discriminator"]
                .as_str()
                .and_then(|value| value.parse::<u64>().ok())
                .map(|value| value % 5)
                .unwrap_or(0);
            Some(format!(
                "https://cdn.discordapp.com/embed/avatars/{index}.png"
            ))
        });
    Ok(Account {
        id,
        name,
        avatar_url,
    })
}

/// Discord's settings endpoint answers `429` with a `Retry-After` header. Keeping
/// that value with the error lets the worker resume as soon as the server allows
/// instead of freezing the status for a fixed pause.
#[derive(Debug)]
struct StatusError {
    message: String,
    retry_after: Option<Duration>,
}

impl StatusError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after: None,
        }
    }

    fn throttled(wait: Duration) -> Self {
        Self {
            message: format!(
                "limite de atualizações do Discord, aguardando {}s",
                wait.as_secs().max(1)
            ),
            retry_after: Some(wait),
        }
    }

    /// Time to wait before the next request for this endpoint.
    fn pause(&self) -> Duration {
        self.retry_after
            .map(|wait| wait + Duration::from_millis(250))
            .unwrap_or(Duration::from_millis(1_500))
    }
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

fn status_error_from(response: reqwest::blocking::Response) -> StatusError {
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let wait = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(3);
        return StatusError::throttled(Duration::from_secs(wait));
    }
    StatusError::new(format!("Discord retornou HTTP {status}"))
}

fn update_discord_status(
    client: &Client,
    token: &str,
    text: &str,
    presence: &str,
    emoji: &str,
) -> Result<(), StatusError> {
    let (emoji_name, emoji_id) = parse_emoji(emoji);
    let body = serde_json::json!({
        "status": presence,
        "custom_status": {
            "text": text.chars().take(128).collect::<String>(),
            "emoji_name": emoji_name,
            "emoji_id": emoji_id,
            "expires_at": null
        }
    });
    let response = client
        .patch(DISCORD_SETTINGS_URL)
        .header("Authorization", token)
        .json(&body)
        .send()
        .map_err(|error| StatusError::new(error.to_string()))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(status_error_from(response))
    }
}

fn clear_discord_status(
    client: &Client,
    token: &str,
    presence: &str,
) -> Result<(), StatusError> {
    let response = client
        .patch(DISCORD_SETTINGS_URL)
        .header("Authorization", token)
        .json(&serde_json::json!({
            "status": presence,
            "custom_status": null
        }))
        .send()
        .map_err(|error| StatusError::new(error.to_string()))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(status_error_from(response))
    }
}

/// Finds the League of Legends local LCU lockfile and reads only its loopback
/// port and remoting password. It deliberately never sends those credentials
/// outside 127.0.0.1.
fn read_league_lockfile() -> Option<(u16, String)> {
    let mut candidates = Vec::new();
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(local_app_data);
        candidates.push(base.join("Riot Games").join("League of Legends").join("lockfile"));
        candidates.push(base.join("Riot Games").join("League of Legends").join("LeagueClient.lockfile"));
    }
    if let Some(program_data) = std::env::var_os("PROGRAMDATA") {
        candidates.push(PathBuf::from(program_data).join("Riot Games").join("League of Legends").join("lockfile"));
    }
    candidates.push(PathBuf::from(r"C:\Riot Games\League of Legends\lockfile"));
    candidates.push(PathBuf::from(r"C:\Riot Games\League of Legends\LeagueClient.lockfile"));
    candidates.push(PathBuf::from(r"C:\Program Files\Riot Games\League of Legends\lockfile"));

    for path in candidates {
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let parts: Vec<&str> = contents.trim().split(':').collect();
        if parts.len() != 5 || parts[4].trim() != "https" {
            continue;
        }
        let Ok(port) = parts[2].parse::<u16>() else {
            continue;
        };
        let password = parts[3].trim();
        if port != 0 && !password.is_empty() {
            return Some((port, password.to_owned()));
        }
    }
    None
}

fn update_league_status(client: &Client, text: &str) -> Result<(), String> {
    let (port, password) = read_league_lockfile()
        .ok_or_else(|| "Cliente do League of Legends não disponível".to_owned())?;
    let safe_text = text.chars().take(128).collect::<String>();
    let endpoint = format!("https://127.0.0.1:{port}/lol-chat/v1/me");
    let response = client
        .get(&endpoint)
        .basic_auth("riot", Some(password.as_str()))
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "League respondeu HTTP {} ao ler perfil",
            response.status()
        ));
    }
    let profile = response
        .json::<serde_json::Value>()
        .map_err(|error| error.to_string())?;
    let payload = league_status_payload(profile, &safe_text)?;
    let response = client
        .put(endpoint)
        .basic_auth("riot", Some(password.as_str()))
        .json(&payload)
        .send()
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("League respondeu HTTP {}", response.status()))
    }
}

fn spawn_league_status_worker(
    client: Client,
    state: Arc<Mutex<ViewState>>,
) -> mpsc::Sender<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut desired = None;
        let mut published = None;
        let mut next_attempt = Instant::now();

        loop {
            while desired.as_ref() == published.as_ref() {
                desired = match receiver.recv() {
                    Ok(text) => Some(text),
                    Err(_) => return,
                };
                if !drain_latest_league_status(&receiver, &mut desired) {
                    return;
                }
            }

            let wait = next_attempt.saturating_duration_since(Instant::now());
            if !wait.is_zero() {
                match receiver.recv_timeout(wait) {
                    Ok(text) => desired = Some(text),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                if !drain_latest_league_status(&receiver, &mut desired) {
                    return;
                }
                continue;
            }

            let Some(text) = desired.clone() else {
                continue;
            };
            match update_league_status(&client, &text) {
                Ok(()) => {
                    published = Some(text);
                    next_attempt = Instant::now() + Duration::from_secs(3);
                    clear_error(&state);
                }
                Err(error) => {
                    set_error(&state, format!("Falha ao atualizar status do League: {error}"));
                    next_attempt = Instant::now() + Duration::from_secs(5);
                }
            }
        }
    });
    sender
}

fn drain_latest_league_status(
    receiver: &mpsc::Receiver<String>,
    desired: &mut Option<String>,
) -> bool {
    loop {
        match receiver.try_recv() {
            Ok(text) => *desired = Some(text),
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => return false,
        }
    }
}

fn send_league_status(
    sender: &mpsc::Sender<String>,
    state: &Arc<Mutex<ViewState>>,
    text: String,
) -> bool {
    if let Err(error) = sender.send(text) {
        set_error(
            state,
            format!("Sincronização do League indisponível: {error}"),
        );
        false
    } else {
        true
    }
}

fn league_status_payload(
    mut profile: serde_json::Value,
    text: &str,
) -> Result<serde_json::Value, String> {
    let Some(fields) = profile.as_object_mut() else {
        return Err("Resposta de perfil inválida do League".to_owned());
    };
    fields.insert(
        "statusMessage".to_owned(),
        serde_json::Value::String(text.to_owned()),
    );
    Ok(profile)
}

const LRCLIB_GET: &str = "https://lrclib.net/api/get";
const LRCLIB_SEARCH: &str = "https://lrclib.net/api/search";

/// Removes `(feat. …)`, `[Remastered]` and `- Live/Remaster/…` noise that the
/// media session often adds to the title and that LRCLIB does not index.
fn strip_track_suffixes(title: &str) -> String {
    let mut cleaned = title.trim().to_owned();
    for (open, close) in [('(', ')'), ('[', ']')] {
        while let Some(start) = cleaned.find(open) {
            let Some(relative_end) = cleaned[start..].find(close) else {
                break;
            };
            cleaned.replace_range(start..start + relative_end + 1, "");
        }
    }
    for marker in [" - ", " – ", " — "] {
        if let Some(index) = cleaned.find(marker) {
            let tail = cleaned[index + marker.len()..].to_ascii_lowercase();
            let noise = [
                "remaster",
                "live",
                "version",
                "edit",
                "mix",
                "instrumental",
                "karaoke",
                "demo",
                "audio",
                "mono",
                "stereo",
                "single",
                "radio",
                "explicit",
                "clean",
                "from ",
                "feat",
                "with ",
                "deluxe",
                "bonus",
            ];
            if noise.iter().any(|word| tail.contains(word)) {
                cleaned.truncate(index);
                break;
            }
        }
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `Artista A, Artista B feat. C` -> `Artista A`.
fn primary_artist(artist: &str) -> Option<String> {
    let trimmed = artist.trim().trim_end_matches(" - Topic").trim();
    let mut end = trimmed.len();
    for marker in [
        " feat. ",
        " feat ",
        " Feat. ",
        " Feat ",
        " Featuring ",
        " featuring ",
        " & ",
        ", ",
        " x ",
        " X ",
        " vs. ",
        " / ",
    ] {
        if let Some(index) = trimmed.find(marker) {
            end = end.min(index);
        }
    }
    let cut = trimmed[..end].trim().to_owned();
    (!cut.is_empty() && cut != artist.trim()).then_some(cut)
}

fn distinct(values: Vec<String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for value in values {
        let value = value.trim().to_owned();
        if !value.is_empty() && !seen.contains(&value) {
            seen.push(value);
        }
    }
    seen
}

fn lyrics_from_payload(
    data: &serde_json::Value,
    duration_ms: u64,
) -> Option<(Vec<LyricLine>, &'static str)> {
    if let Some(lrc) = data["syncedLyrics"].as_str() {
        let lines = parse_lrc(lrc);
        if !lines.is_empty() {
            return Some((lines, "LRCLIB sincronizadas"));
        }
    }
    if let Some(plain) = data["plainLyrics"].as_str() {
        let lines = distribute_plain_lyrics(plain, duration_ms);
        if !lines.is_empty() {
            return Some((lines, "LRCLIB estimadas"));
        }
    }
    None
}

fn duration_delta(item: &serde_json::Value, duration_ms: u64) -> u64 {
    let seconds = item["duration"].as_f64().unwrap_or(0.0);
    if seconds <= 0.0 {
        return u64::MAX / 2;
    }
    ((seconds * 1000.0) as i64 - duration_ms as i64).unsigned_abs()
}

/// Best candidate for the track: closest duration wins, preferring entries
/// whose duration is within 15 seconds of what is playing.
fn pick_best(
    items: &[serde_json::Value],
    duration_ms: u64,
    synced: bool,
) -> Option<&serde_json::Value> {
    let mut candidates = items
        .iter()
        .filter(|item| {
            if synced {
                item["syncedLyrics"].is_string()
            } else {
                item["plainLyrics"].is_string()
            }
        })
        .map(|item| (item, duration_delta(item, duration_ms)))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(_, delta)| (*delta > 15_000, *delta));
    candidates.first().map(|(item, _)| *item)
}

fn lrclib_get(
    client: &Client,
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration_ms: u64,
) -> Option<(Vec<LyricLine>, &'static str)> {
    if title.trim().is_empty() || artist.trim().is_empty() {
        return None;
    }
    let mut query = vec![
        ("track_name", title.to_owned()),
        ("artist_name", artist.to_owned()),
    ];
    if let Some(album) = album.filter(|album| !album.trim().is_empty()) {
        query.push(("album_name", album.to_owned()));
    }
    let response = client.get(LRCLIB_GET).query(&query).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let data = response.json::<serde_json::Value>().ok()?;
    lyrics_from_payload(&data, duration_ms)
}

fn lrclib_search(
    client: &Client,
    title: &str,
    artist: &str,
    duration_ms: u64,
) -> Option<(Vec<LyricLine>, &'static str)> {
    if title.trim().is_empty() {
        return None;
    }
    let mut query = vec![("track_name", title.to_owned())];
    if !artist.trim().is_empty() {
        query.push(("artist_name", artist.to_owned()));
    }
    let response = client.get(LRCLIB_SEARCH).query(&query).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let items = response.json::<Vec<serde_json::Value>>().ok()?;
    let best =
        pick_best(&items, duration_ms, true).or_else(|| pick_best(&items, duration_ms, false))?;
    lyrics_from_payload(best, duration_ms)
}

fn lrclib_search_free(
    client: &Client,
    text: &str,
    duration_ms: u64,
) -> Option<(Vec<LyricLine>, &'static str)> {
    if text.trim().is_empty() {
        return None;
    }
    let response = client
        .get(LRCLIB_SEARCH)
        .query(&[("q", text.to_owned())])
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let items = response.json::<Vec<serde_json::Value>>().ok()?;
    let best =
        pick_best(&items, duration_ms, true).or_else(|| pick_best(&items, duration_ms, false))?;
    lyrics_from_payload(best, duration_ms)
}

fn fetch_lyrics(client: &Client, track: &Track) -> (Vec<LyricLine>, &'static str) {
    let titles = distinct(vec![
        track.title.clone(),
        strip_track_suffixes(&track.title),
    ]);
    let artists = distinct(vec![
        track.artist.clone(),
        track.artist.replace(" - Topic", ""),
        primary_artist(&track.artist).unwrap_or_default(),
    ]);
    let album = track.album.trim();
    let duration = track.duration_ms;

    // 1. Exact match, with and without the album (Spotify albums often differ
    //    from the ones LRCLIB stored).
    for title in &titles {
        for artist in &artists {
            if let Some(found) = lrclib_get(client, title, artist, Some(album), duration) {
                return found;
            }
            if let Some(found) = lrclib_get(client, title, artist, None, duration) {
                return found;
            }
        }
    }

    // 2. Search by track + artist, then by primary artist only.
    for title in &titles {
        for artist in &artists {
            if let Some(found) = lrclib_search(client, title, artist, duration) {
                return found;
            }
        }
    }

    // 3. Last resort: free text query.
    for title in &titles {
        let text = if let Some(artist) = artists.first() {
            format!("{title} {artist}")
        } else {
            title.clone()
        };
        if let Some(found) = lrclib_search_free(client, &text, duration) {
            return found;
        }
    }

    (Vec::new(), "indisponíveis")
}

fn fetch_artwork(client: &Client, track: &Track) -> Option<Vec<u8>> {
    let response = client
        .get("https://api.deezer.com/search")
        .query(&[
            ("q", format!("{} {}", track.artist, track.title)),
            ("limit", "3".to_owned()),
        ])
        .send()
        .ok()
        .and_then(|response| response.json::<serde_json::Value>().ok())
        .and_then(|data| {
            data["data"]
                .as_array()?
                .iter()
                .find_map(|entry| entry["album"]["cover_big"].as_str().map(ToOwned::to_owned))
        });
    let cover_url = response.or_else(|| {
        client
            .get("https://itunes.apple.com/search")
            .query(&[
                ("term", format!("{} {}", track.artist, track.title)),
                ("limit", "3".to_owned()),
                ("media", "music".to_owned()),
                ("entity", "song".to_owned()),
            ])
            .send()
            .ok()?
            .json::<serde_json::Value>()
            .ok()?["results"]
            .as_array()?
            .iter()
            .find_map(|item| {
                item["artworkUrl100"]
                    .as_str()
                    .map(|url| url.replace("100x100bb", "600x600bb"))
            })
    })?;
    client
        .get(cover_url)
        .send()
        .ok()?
        .bytes()
        .ok()
        .map(|bytes| bytes.to_vec())
}

fn fetch_bytes(client: &Client, url: &str) -> Option<Vec<u8>> {
    client
        .get(url)
        .send()
        .ok()?
        .bytes()
        .ok()
        .map(|bytes| bytes.to_vec())
}

fn parse_lrc(lrc: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim();
        let mut timestamps = Vec::new();
        while let Some(after_open) = rest.strip_prefix('[') {
            let Some((timestamp, after_close)) = after_open.split_once(']') else {
                break;
            };
            if let Some(ms) = parse_timestamp(timestamp) {
                timestamps.push(ms);
                rest = after_close;
            } else {
                break;
            }
        }
        let text = strip_inline_lrc_tags(rest).trim().to_owned();
        for time_ms in timestamps {
            lines.push(LyricLine {
                time_ms,
                text: text.clone(),
            });
        }
    }
    lines.sort_by_key(|line| line.time_ms);
    lines
}

fn parse_timestamp(value: &str) -> Option<u64> {
    let (minutes, seconds) = value.split_once(':')?;
    let (whole_seconds, fraction) = seconds.split_once('.')?;
    let minutes = minutes.parse::<u64>().ok()?;
    let whole_seconds = whole_seconds.parse::<u64>().ok()?;
    if whole_seconds >= 60
        || !(2..=3).contains(&fraction.len())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let digits = fraction.chars().take(3).collect::<String>();
    let fractional_ms = match digits.len() {
        0 => 0,
        1 => digits.parse::<u64>().ok()? * 100,
        2 => digits.parse::<u64>().ok()? * 10,
        _ => digits.parse::<u64>().ok()?,
    };
    Some(minutes * 60_000 + whole_seconds * 1_000 + fractional_ms)
}

fn strip_inline_lrc_tags(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(relative_open) = text[cursor..].find('<') {
        let open = cursor + relative_open;
        output.push_str(&text[cursor..open]);
        let content_start = open + 1;
        if let Some(relative_close) = text[content_start..].find('>') {
            let close = content_start + relative_close;
            if parse_timestamp(&text[content_start..close]).is_some() {
                cursor = close + 1;
                continue;
            }
        }
        output.push('<');
        cursor = content_start;
    }
    output.push_str(&text[cursor..]);
    output
}

fn distribute_plain_lyrics(text: &str, duration_ms: u64) -> Vec<LyricLine> {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.len() < 3 || duration_ms == 0 {
        return Vec::new();
    }
    lines
        .iter()
        .enumerate()
        .map(|(index, text)| LyricLine {
            time_ms: duration_ms.saturating_mul(index as u64) / lines.len() as u64,
            text: (*text).to_owned(),
        })
        .collect()
}

/// LRCLIB lines usually start on a bar, so their median gap gives a rough tempo
/// (a 2 s bar is about 120 BPM). Estimated plain lyrics have no timing signal.
fn lyric_cadence_bpm(lines: &[LyricLine], synchronized: bool) -> Option<u16> {
    if !synchronized {
        return None;
    }
    let mut gaps = lines
        .windows(2)
        .map(|pair| pair[1].time_ms.saturating_sub(pair[0].time_ms))
        .filter(|gap| (700..=12_000).contains(gap))
        .collect::<Vec<_>>();
    if gaps.len() < 4 {
        return None;
    }
    gaps.sort_unstable();
    let middle = gaps.len() / 2;
    let median = if gaps.len() % 2 == 0 {
        (gaps[middle - 1] + gaps[middle]) / 2
    } else {
        gaps[middle]
    }
    .max(1);
    Some((240_000 / median).clamp(50, 220) as u16)
}

/// Add a quarter beat of lookahead to the manual lead. `0` stays `0`, so a user
/// who turned the lead off keeps it off.
fn effective_lead_ms(base_ms: u16, bpm: Option<u16>) -> u16 {
    let Some(bpm) = bpm else {
        return base_ms;
    };
    if base_ms == 0 || bpm == 0 {
        return base_ms;
    }
    base_ms.saturating_add((15_000 / bpm).min(1_500)).min(1_500)
}

/// Keep anticipation below half of a typical short line interval so it cannot
/// jump over a lyric when the configured lead or Discord update interval is high.
fn cap_lyric_lookahead_ms(lines: &[LyricLine], lead_ms: u16) -> u16 {
    let mut gaps = lines
        .windows(2)
        .map(|pair| pair[1].time_ms.saturating_sub(pair[0].time_ms))
        .filter(|gap| (1..=12_000).contains(gap))
        .collect::<Vec<_>>();
    if gaps.is_empty() {
        return lead_ms;
    }
    gaps.sort_unstable();
    let lower_quartile = gaps[(gaps.len() - 1) / 4];
    lead_ms.min((lower_quartile / 2).min(u16::MAX as u64) as u16)
}

fn active_lyric(lines: &[LyricLine], position_ms: u64) -> Option<usize> {
    lines.iter().rposition(|line| line.time_ms <= position_ms)
}

/// Keeps the displayed lyric line from going backwards while the song plays.
fn hold_lyric_floor(previous: Option<usize>, candidate: Option<usize>) -> Option<usize> {
    match (previous, candidate) {
        (Some(previous), Some(candidate)) => Some(candidate.max(previous)),
        (Some(previous), None) => Some(previous),
        (None, candidate) => candidate,
    }
}

#[cfg(windows)]
fn seek_smtc(position_ms: u64) -> Result<(), String> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .map_err(|error| format!("SMTC: {error}"))?
        .get()
        .map_err(|error| format!("SMTC: {error}"))?;
    let sessions = manager
        .GetSessions()
        .map_err(|error| format!("SMTC: {error}"))?;
    let count = sessions.Size().map_err(|error| format!("SMTC: {error}"))?;
    for index in 0..count {
        let session = sessions
            .GetAt(index)
            .map_err(|error| format!("SMTC: {error}"))?;
        if !session
            .SourceAppUserModelId()
            .unwrap_or_default()
            .to_string()
            .to_lowercase()
            .contains("spotify")
        {
            continue;
        }
        let succeeded = session
            .TryChangePlaybackPositionAsync(position_ms as i64)
            .map_err(|error| format!("SMTC: {error}"))?
            .get()
            .map_err(|error| format!("SMTC: {error}"))?;
        return if succeeded {
            Ok(())
        } else {
            Err("O Spotify recusou a alteração de posição.".to_owned())
        };
    }
    Err("Nenhuma sessão do Spotify está disponível para seek.".to_owned())
}

#[cfg(not(windows))]
fn seek_smtc(_position_ms: u64) -> Result<(), String> {
    Err("O seek SMTC está disponível somente no Windows 10/11.".to_owned())
}

#[cfg(windows)]
fn control_smtc(action: MediaAction) -> Result<(), String> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .map_err(|error| format!("SMTC: {error}"))?
        .get()
        .map_err(|error| format!("SMTC: {error}"))?;
    let sessions = manager
        .GetSessions()
        .map_err(|error| format!("SMTC: {error}"))?;
    let count = sessions.Size().map_err(|error| format!("SMTC: {error}"))?;
    for index in 0..count {
        let session = sessions
            .GetAt(index)
            .map_err(|error| format!("SMTC: {error}"))?;
        if !session
            .SourceAppUserModelId()
            .unwrap_or_default()
            .to_string()
            .to_lowercase()
            .contains("spotify")
        {
            continue;
        }
        let succeeded = match action {
            MediaAction::TogglePlayPause => session.TryTogglePlayPauseAsync(),
            MediaAction::Previous => session.TrySkipPreviousAsync(),
            MediaAction::Next => session.TrySkipNextAsync(),
        }
        .map_err(|error| format!("SMTC: {error}"))?
        .get()
        .map_err(|error| format!("SMTC: {error}"))?;
        return if succeeded {
            Ok(())
        } else {
            Err("O Spotify não aceitou o comando de reprodução.".to_owned())
        };
    }
    Err("Nenhuma sessão do Spotify está disponível.".to_owned())
}

#[cfg(not(windows))]
fn control_smtc(_action: MediaAction) -> Result<(), String> {
    Err("Os controles de reprodução estão disponíveis somente no Windows 10/11.".to_owned())
}

fn set_error(state: &Arc<Mutex<ViewState>>, error: String) {
    if let Ok(mut view) = state.lock() {
        view.last_error = Some(error);
    }
}

fn clear_error(state: &Arc<Mutex<ViewState>>) {
    if let Ok(mut view) = state.lock() {
        view.last_error = None;
    }
}

fn set_token_status(state: &Arc<Mutex<ViewState>>, status: String) {
    if let Ok(mut view) = state.lock() {
        view.token_status = status;
    }
}

fn set_discord_status(state: &Arc<Mutex<ViewState>>, status: String) {
    if let Ok(mut view) = state.lock() {
        view.discord_status = status;
        if view.discord_status == "connected" {
            view.last_error = None;
        }
    }
}

fn load_texture(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(name.to_owned(), color_image, egui::TextureOptions::LINEAR))
}

fn blend(base: egui::Color32, tint: egui::Color32, amount: f32) -> egui::Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    egui::Color32::from_rgb(
        mix(base.r(), tint.r()),
        mix(base.g(), tint.g()),
        mix(base.b(), tint.b()),
    )
}

/// Vertical stack of the sidebar blocks: the recent-tracks card fills the
/// middle, and the now-playing card plus the footer stay anchored to the
/// bottom without ever overlapping each other.
fn sidebar_stack_rects(
    panel: egui::Rect,
    nav_bottom: f32,
) -> (egui::Rect, egui::Rect, egui::Rect) {
    let footer = egui::Rect::from_min_size(
        egui::pos2(panel.left(), panel.bottom() - 104.0),
        egui::vec2(panel.width(), 104.0),
    );
    let now_playing = egui::Rect::from_min_size(
        egui::pos2(panel.left(), footer.top() - 8.0 - 96.0),
        egui::vec2(panel.width(), 96.0),
    );
    let recents = egui::Rect::from_min_max(
        egui::pos2(panel.left(), nav_bottom + 8.0),
        egui::pos2(panel.right(), now_playing.top() - 8.0),
    );
    (recents, now_playing, footer)
}

fn ui_theme_accent(ui: &egui::Ui) -> egui::Color32 {
    ui.visuals().selection.stroke.color
}

/// WCAG relative luminance, used to decide between black and white text and
/// to keep cover accents readable on the dark surfaces.
fn relative_luminance(color: egui::Color32) -> f32 {
    let channel = |value: u8| {
        let channel = value as f32 / 255.0;
        if channel <= 0.03928 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

fn contrast_text(color: egui::Color32) -> egui::Color32 {
    let luminance = relative_luminance(color);
    let on_white = 1.05 / (luminance + 0.05);
    let on_black = (luminance + 0.05) / 0.05;
    if on_white >= on_black { TEXT } else { BLACK }
}

/// Hue in degrees plus saturation and value, all normalised to 0..=1.
fn rgb_to_hsv(color: egui::Color32) -> (f32, f32, f32) {
    let red = color.r() as f32 / 255.0;
    let green = color.g() as f32 / 255.0;
    let blue = color.b() as f32 / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == red {
        ((green - blue) / delta).rem_euclid(6.0) * 60.0
    } else if max == green {
        ((blue - red) / delta + 2.0) * 60.0
    } else {
        ((red - green) / delta + 4.0) * 60.0
    };
    let saturation = if max <= f32::EPSILON { 0.0 } else { delta / max };
    (hue, saturation, max)
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> egui::Color32 {
    let hue = hue.rem_euclid(360.0);
    let chroma = value * saturation;
    let second = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let offset = value - chroma;
    let (red, green, blue) = match hue {
        hue if hue < 60.0 => (chroma, second, 0.0),
        hue if hue < 120.0 => (second, chroma, 0.0),
        hue if hue < 180.0 => (0.0, chroma, second),
        hue if hue < 240.0 => (0.0, second, chroma),
        hue if hue < 300.0 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    egui::Color32::from_rgb(
        ((red + offset) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((green + offset) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((blue + offset) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

/// Colours taken from the current cover: a vivid highlight and a dark wash
/// used behind large surfaces.
#[derive(Clone, Copy)]
struct CoverPalette {
    accent: egui::Color32,
    tint: egui::Color32,
}

impl CoverPalette {
    fn fallback() -> Self {
        Self {
            accent: ACCENT,
            tint: theme_tint(ACCENT),
        }
    }
}

fn artwork_palette(image: &image::RgbaImage) -> CoverPalette {
    let accent = artwork_accent(average_color(image));
    CoverPalette {
        accent,
        tint: theme_tint(accent),
    }
}

/// Keeps the cover hue but darkens it, so the header fade and the hero card
/// stay readable with white text no matter how bright the artwork is.
fn theme_tint(accent: egui::Color32) -> egui::Color32 {
    let (hue, saturation, _) = rgb_to_hsv(accent);
    hsv_to_rgb(hue, saturation.clamp(0.45, 0.78), 0.34)
}

fn average_color(image: &image::RgbaImage) -> egui::Color32 {
    let mut totals = [0u64; 3];
    let mut count = 0u64;
    // Weighted by saturation so neutral backgrounds (white, black, skin tones)
    // never win over the colour the cover is actually known for.
    let mut hues = [[0f32; 4]; 12];
    let step = ((image.width() as usize * image.height() as usize) / 512).max(1);
    for (index, pixel) in image.pixels().enumerate() {
        if index % step != 0 {
            continue;
        }
        totals[0] += pixel[0] as u64;
        totals[1] += pixel[1] as u64;
        totals[2] += pixel[2] as u64;
        count += 1;

        let color = egui::Color32::from_rgb(pixel[0], pixel[1], pixel[2]);
        let (hue, saturation, value) = rgb_to_hsv(color);
        if saturation < 0.18 || value < 0.12 {
            continue;
        }
        let weight = saturation * saturation * (0.35 + value);
        let bin = ((hue / 30.0) as usize).min(hues.len() - 1);
        hues[bin][0] += weight;
        hues[bin][1] += weight * pixel[0] as f32;
        hues[bin][2] += weight * pixel[1] as f32;
        hues[bin][3] += weight * pixel[2] as f32;
    }
    if count == 0 {
        return SURFACE;
    }
    let dominant = hues
        .iter()
        .max_by(|left, right| left[0].total_cmp(&right[0]))
        .copied()
        .unwrap_or([0.0; 4]);
    if dominant[0] > 0.5 {
        return egui::Color32::from_rgb(
            (dominant[1] / dominant[0]).round().clamp(0.0, 255.0) as u8,
            (dominant[2] / dominant[0]).round().clamp(0.0, 255.0) as u8,
            (dominant[3] / dominant[0]).round().clamp(0.0, 255.0) as u8,
        );
    }
    egui::Color32::from_rgb(
        (totals[0] / count) as u8,
        (totals[1] / count) as u8,
        (totals[2] / count) as u8,
    )
}

fn artwork_accent(color: egui::Color32) -> egui::Color32 {
    let (hue, saturation, mut value) = rgb_to_hsv(color);
    if saturation < 0.16 || value < 0.10 {
        return ACCENT;
    }

    let saturation = saturation.clamp(0.58, 0.86);
    // Dark and muddy covers would otherwise produce an accent that is almost
    // invisible on the #121212 background, so lift the value until the accent
    // has enough contrast to be used as text and as an icon colour.
    while relative_luminance(hsv_to_rgb(hue, saturation, value)) < 0.16 && value < 0.92 {
        value = (value + 0.03).min(0.92);
    }
    hsv_to_rgb(hue, saturation, value)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

fn kicker(text: &str) -> egui::RichText {
    egui::RichText::new(text).size(9.0).strong().color(MUTED)
}

fn greeting_for_hour(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Bom dia",
        12..=17 => "Boa tarde",
        _ => "Boa noite",
    }
}

fn greeting_text() -> &'static str {
    greeting_for_hour(local_hour())
}

#[cfg(windows)]
fn local_hour() -> u32 {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    // Reads the local system clock; takes no pointers and allocates nothing.
    let local = unsafe { GetLocalTime() };
    local.wHour as u32
}

#[cfg(not(windows))]
fn local_hour() -> u32 {
    12
}

/// Case-insensitive match shared by the history filter and the search box.
fn history_matches(entry: &HistoryEntry, filter: &str) -> bool {
    filter.is_empty()
        || entry.title.to_lowercase().contains(filter)
        || entry.artist.to_lowercase().contains(filter)
        || entry.album.to_lowercase().contains(filter)
}

fn format_span(ms: u64) -> String {
    let minutes = ms / 60_000;
    if minutes == 0 {
        return "—".to_owned();
    }
    let hours = minutes / 60;
    let rest = minutes % 60;
    if hours == 0 {
        format!("{rest} min")
    } else {
        format!("{hours}h {rest:02}")
    }
}

/// Milliseconds elapsed since local midnight, used to count today's tracks.
#[cfg(windows)]
fn local_midnight_offset_ms() -> u64 {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let local = unsafe { GetLocalTime() };
    (local.wHour as u64 * 3_600 + local.wMinute as u64 * 60 + local.wSecond as u64) * 1_000
}

#[cfg(not(windows))]
fn local_midnight_offset_ms() -> u64 {
    0
}

fn stat_tile(
    ui: &mut egui::Ui,
    width: f32,
    label: &str,
    value: &str,
    detail: &str,
    accent: egui::Color32,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 86.0), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(8),
        blend(SURFACE, accent, 0.06),
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.left() + 14.0, rect.top() + 14.0),
            egui::vec2(18.0, 2.0),
        ),
        egui::CornerRadius::same(1),
        accent,
    );
    painter.text(
        egui::pos2(rect.left() + 14.0, rect.top() + 28.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(9.0),
        MUTED,
    );
    let value_chars = ((rect.width() - 28.0) / 10.0).max(4.0) as usize;
    painter.text(
        egui::pos2(rect.left() + 14.0, rect.top() + 50.0),
        egui::Align2::LEFT_CENTER,
        truncate(value, value_chars),
        egui::FontId::new(20.0, egui::FontFamily::Name("SpotifyBold".into())),
        TEXT,
    );
    painter.text(
        egui::pos2(rect.left() + 14.0, rect.top() + 70.0),
        egui::Align2::LEFT_CENTER,
        detail,
        egui::FontId::proportional(9.5),
        MUTED,
    );
}

fn album_card(
    ui: &mut egui::Ui,
    width: f32,
    entry: &HistoryEntry,
    cover: Option<&egui::TextureHandle>,
    accent: egui::Color32,
    is_current: bool,
) -> bool {
    let padding = 14.0;
    let cover_size = (width - padding * 2.0).max(60.0);
    let height = cover_size + padding * 2.0 + 48.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return false;
    }
    let hover = ui.ctx().animate_bool_with_time(
        egui::Id::new(("album", entry.album.as_str(), entry.artist.as_str())),
        response.hovered(),
        0.12,
    );
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(8),
        blend(SURFACE, blend(SURFACE_HOVER, accent, 0.10), hover),
    );
    let cover_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(padding, padding),
        egui::vec2(cover_size, cover_size),
    );
    ui.painter().rect_filled(
        cover_rect.translate(egui::vec2(0.0, 4.0)),
        egui::CornerRadius::same(8),
        egui::Color32::from_black_alpha(90),
    );
    if let Some(texture) = cover {
        egui::Image::new(texture)
            .fit_to_exact_size(cover_rect.size())
            .corner_radius(egui::CornerRadius::same(8))
            .paint_at(ui, cover_rect);
    } else {
        let seed = stable_hash(entry.album.as_bytes()) as u32;
        let start = egui::Color32::from_rgb(
            45 + (seed as u8 % 70),
            45 + ((seed >> 8) as u8 % 70),
            45 + ((seed >> 16) as u8 % 70),
        );
        paint_gradient_rect(ui.painter(), cover_rect, start, blend(start, accent, 0.6), 8);
        paint_note_at(ui.painter(), cover_rect.center(), 22.0, TEXT);
    }
    if hover > 0.0 {
        let center = egui::pos2(
            cover_rect.right() - 24.0,
            cover_rect.bottom() - 24.0 + 8.0 * (1.0 - hover),
        );
        ui.painter().circle_filled(
            center + egui::vec2(0.0, 3.0),
            18.0,
            egui::Color32::from_black_alpha((90.0 * hover) as u8),
        );
        ui.painter().circle_filled(
            center,
            18.0,
            egui::Color32::from_rgba_unmultiplied(
                accent.r(),
                accent.g(),
                accent.b(),
                (245.0 * hover) as u8,
            ),
        );
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            "→",
            egui::FontId::proportional(15.0),
            contrast_text(accent),
        );
    }
    let chars = ((cover_size / 7.4) as usize).max(6);
    ui.painter().text(
        egui::pos2(rect.left() + padding, cover_rect.bottom() + 15.0),
        egui::Align2::LEFT_TOP,
        truncate(entry.album.trim(), chars),
        egui::FontId::new(13.5, egui::FontFamily::Name("SpotifyBold".into())),
        TEXT,
    );
    ui.painter().text(
        egui::pos2(rect.left() + padding, cover_rect.bottom() + 33.0),
        egui::Align2::LEFT_TOP,
        truncate(entry.artist.trim(), chars),
        egui::FontId::proportional(11.0),
        MUTED,
    );
    if is_current {
        ui.painter().circle_filled(
            egui::pos2(rect.right() - padding - 3.0, cover_rect.bottom() + 22.0),
            2.5,
            accent,
        );
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.on_hover_text("Ver no histórico").clicked()
}

fn paint_gradient_rect(
    painter: &egui::Painter,
    rect: egui::Rect,
    start: egui::Color32,
    end: egui::Color32,
    radius: u8,
) {
    painter.rect_filled(rect, egui::CornerRadius::same(radius), start);
    let inset = rect.shrink(1.0);
    let mid = blend(start, end, 0.5);
    let mut mesh = egui::epaint::Mesh::default();
    mesh.colored_vertex(inset.left_top(), start);
    mesh.colored_vertex(inset.right_top(), mid);
    mesh.colored_vertex(inset.right_bottom(), end);
    mesh.colored_vertex(inset.left_bottom(), blend(start, end, 0.72));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Spotify's content panel fades from a soft colour taken from the cover art
/// at the top into the flat #121212 body, with the top corners rounded like the
/// desktop app's panels.
fn paint_spotify_header_gradient(
    painter: &egui::Painter,
    rect: egui::Rect,
    tint: egui::Color32,
) {
    let top = blend(egui::Color32::from_rgb(38, 38, 38), tint, 0.62);
    let gradient_height = rect.height().min(300.0);
    let steps = 40;
    for step in 0..steps {
        let top_ratio = step as f32 / steps as f32;
        let bottom_ratio = (step + 1) as f32 / steps as f32;
        let y = rect.top() + gradient_height * top_ratio;
        let next_y = rect.top() + gradient_height * bottom_ratio;
        let color = blend(top, BG, (top_ratio + bottom_ratio) * 0.5);
        let radius = if step == 0 {
            egui::CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            }
        } else {
            egui::CornerRadius::ZERO
        };
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), y),
                egui::pos2(rect.right(), next_y + 1.0),
            ),
            radius,
            color,
        );
    }
}

/// egui ships no bold face, so titles are painted twice with a hair of offset
/// to get the weight Spotify uses for card and row titles.
/// Semibold text using the Segoe UI Semibold face loaded at startup.
fn bold(text: &str, size: f32) -> egui::RichText {
    egui::RichText::new(text).font(egui::FontId::new(
        size,
        egui::FontFamily::Name("SpotifyBold".into()),
    ))
}

#[derive(Clone, Copy)]
enum LoginFeature {
    Wave,
    Lyric,
    Shield,
}

/// Soft radial glow built from concentric translucent circles, since egui has
/// no gradient brush.
fn login_glow(painter: &egui::Painter, center: egui::Pos2, radius: f32, color: egui::Color32) {
    let layers = 26;
    for index in 0..layers {
        let t = index as f32 / (layers - 1) as f32;
        let alpha = ((1.0 - t).powi(2) * 15.0).round() as u8;
        if alpha == 0 {
            continue;
        }
        painter.circle_filled(
            center,
            radius * (0.25 + 0.75 * t),
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha),
        );
    }
}

fn paint_login_backdrop(
    painter: &egui::Painter,
    rect: egui::Rect,
    accent: egui::Color32,
    time: f32,
) {
    painter.rect_filled(rect, egui::CornerRadius::same(0), BLACK);
    let drift = (time * 0.22).sin() * 34.0;
    login_glow(
        painter,
        egui::pos2(
            rect.left() + rect.width() * 0.22 + drift,
            rect.top() + rect.height() * 0.18,
        ),
        580.0,
        accent,
    );
    login_glow(
        painter,
        egui::pos2(
            rect.right() - rect.width() * 0.14,
            rect.bottom() - rect.height() * 0.14 - drift,
        ),
        520.0,
        blend(accent, egui::Color32::from_rgb(24, 96, 190), 0.55),
    );
    // Faint record grooves so the empty space still has texture.
    let grooves = egui::pos2(
        rect.right() - rect.width() * 0.06,
        rect.top() + rect.height() * 0.1,
    );
    for index in 0..7 {
        painter.circle_stroke(
            grooves,
            130.0 + index as f32 * 46.0,
            egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(5)),
        );
    }
}

fn login_arc(center: egui::Pos2, radius: f32, start: f32, end: f32) -> Vec<egui::Pos2> {
    let steps = 18;
    (0..=steps)
        .map(|index| {
            let t = index as f32 / steps as f32;
            let angle = start + (end - start) * t;
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect()
}

fn paint_login_feature(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    feature: LoginFeature,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new((size * 0.1).max(1.2), color);
    match feature {
        LoginFeature::Wave => {
            for (index, height) in [0.55_f32, 1.0, 0.7].iter().enumerate() {
                let x = center.x + (index as f32 - 1.0) * size * 0.26;
                painter.rect_filled(
                    egui::Rect::from_center_size(
                        egui::pos2(x, center.y),
                        egui::vec2(size * 0.16, size * height),
                    ),
                    egui::CornerRadius::same(2),
                    color,
                );
            }
        }
        LoginFeature::Lyric => {
            for (index, width) in [1.0_f32, 0.78, 0.5].iter().enumerate() {
                let y = center.y + (index as f32 - 1.0) * size * 0.3;
                let half = size * width * 0.5;
                painter.line_segment(
                    [egui::pos2(center.x - half, y), egui::pos2(center.x + half, y)],
                    stroke,
                );
            }
        }
        LoginFeature::Shield => {
            let body = egui::Rect::from_min_size(
                egui::pos2(center.x - size * 0.26, center.y - size * 0.06),
                egui::vec2(size * 0.52, size * 0.4),
            );
            painter.rect_filled(body, egui::CornerRadius::same(2), color);
            let arch = login_arc(
                egui::pos2(center.x, center.y - size * 0.08),
                size * 0.18,
                std::f32::consts::PI,
                std::f32::consts::TAU,
            );
            painter.add(egui::Shape::line(arch, stroke));
        }
    }
}

fn login_feature(
    ui: &mut egui::Ui,
    feature: LoginFeature,
    title: &str,
    detail: &str,
    accent: egui::Color32,
) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 17.0, blend(SURFACE, accent, 0.14));
        ui.painter().circle_stroke(
            rect.center(),
            17.0,
            egui::Stroke::new(1.0_f32, blend(BORDER_SOFT, accent, 0.3)),
        );
        paint_login_feature(ui.painter(), rect.center(), 15.0, feature, accent);
        ui.add_space(5.0);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.label(egui::RichText::new(title).size(12.0).strong().color(TEXT));
                ui.add(
                    egui::Label::new(egui::RichText::new(detail).size(10.5).color(MUTED)).wrap(),
                );
            },
        );
    });
}

fn login_status_preview(
    ui: &mut egui::Ui,
    accent: egui::Color32,
    time: f32,
    logo: &egui::TextureHandle,
) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 104.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(18),
        blend(SURFACE, accent, 0.055),
    );
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(18),
        egui::Stroke::new(1.0_f32, blend(BORDER_SOFT, accent, 0.34)),
        egui::StrokeKind::Inside,
    );
    let avatar = egui::pos2(rect.left() + 34.0, rect.top() + 34.0);
    painter.circle_filled(avatar, 19.0, blend(SURFACE, accent, 0.2));
    painter.image(
        logo.id(),
        egui::Rect::from_center_size(avatar, egui::vec2(26.0, 26.0)),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    painter.text(
        egui::pos2(rect.left() + 62.0, rect.top() + 27.0),
        egui::Align2::LEFT_CENTER,
        "Sua conta",
        egui::FontId::new(12.5, egui::FontFamily::Name("SpotifyBold".into())),
        TEXT,
    );
    painter.text(
        egui::pos2(rect.left() + 62.0, rect.top() + 43.0),
        egui::Align2::LEFT_CENTER,
        "Ouvindo Spotify",
        egui::FontId::proportional(9.5),
        MUTED,
    );
    let pill = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 84.0, rect.top() + 20.0),
        egui::vec2(66.0, 20.0),
    );
    painter.rect_filled(pill, egui::CornerRadius::same(10), blend(SURFACE, accent, 0.16));
    let pulse = (time * 2.4).sin() * 0.5 + 0.5;
    painter.circle_filled(
        egui::pos2(pill.left() + 12.0, pill.center().y),
        2.4 + pulse * 1.0,
        accent,
    );
    painter.text(
        egui::pos2(pill.left() + 21.0, pill.center().y),
        egui::Align2::LEFT_CENTER,
        "AO VIVO",
        egui::FontId::new(8.0, egui::FontFamily::Name("SpotifyBold".into())),
        blend(MUTED, accent, 0.6),
    );
    let strip = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 16.0, rect.top() + 62.0),
        egui::pos2(rect.right() - 16.0, rect.bottom() - 16.0),
    );
    painter.rect_filled(strip, egui::CornerRadius::same(10), INSET);
    for index in 0..4 {
        let phase = time * 7.0 + index as f32 * 0.9;
        let height = 5.0 + (phase.sin() * 0.5 + 0.5) * 11.0;
        painter.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(strip.left() + 17.0 + index as f32 * 5.5, strip.center().y),
                egui::vec2(2.8, height),
            ),
            egui::CornerRadius::same(2),
            accent,
        );
    }
    let sample = truncate(
        "E eu vou dançar com você",
        ((strip.width() - 60.0) / 6.4).max(8.0) as usize,
    );
    painter.text(
        egui::pos2(strip.left() + 46.0, strip.center().y),
        egui::Align2::LEFT_CENTER,
        sample,
        egui::FontId::proportional(11.0),
        TEXT_DIM,
    );
}

fn login_pill(ui: &mut egui::Ui, label: &str, accent: egui::Color32) {
    let color = blend(MUTED, accent, 0.45);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), egui::FontId::proportional(9.0), color);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 18.0, 21.0),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(10), blend(SURFACE, accent, 0.09));
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        egui::Stroke::new(1.0_f32, blend(BORDER_SOFT, accent, 0.22)),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
}

/// Settings navigation entry, Spotify style: flat row, round hover plate and
/// a semibold label while selected.
fn settings_nav(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        if active {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(4),
                egui::Color32::from_rgb(42, 42, 42),
            );
        } else if response.hovered() {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(4),
                egui::Color32::from_rgb(30, 30, 30),
            );
        }
        let font = if active {
            egui::FontId::new(14.0, egui::FontFamily::Name("SpotifyBold".into()))
        } else {
            egui::FontId::proportional(14.0)
        };
        ui.painter().text(
            egui::pos2(rect.left() + 12.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font,
            if active { TEXT } else { MUTED },
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    response.clicked()
}

fn settings_panel_title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(bold(title, 24.0).color(TEXT));
    ui.add_space(6.0);
    ui.add(egui::Label::new(egui::RichText::new(subtitle).size(13.0).color(MUTED)).wrap());
    ui.add_space(18.0);
    soft_divider(ui);
    ui.add_space(18.0);
}

fn settings_row(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    let width = ui.available_width();
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2((width - 230.0).max(150.0), 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.label(bold(title, 14.0).color(TEXT));
                if !description.is_empty() {
                    ui.add_space(3.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(description).size(12.0).color(MUTED),
                        )
                        .wrap(),
                    );
                }
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            add(ui);
        });
    });
    ui.add_space(12.0);
    soft_divider(ui);
    ui.add_space(12.0);
}

fn settings_value_row(ui: &mut egui::Ui, label: &str, value: String) {
    settings_row(ui, label, "", |ui| {
        ui.label(egui::RichText::new(value).size(12.5).color(TEXT_DIM));
    });
}

fn settings_toggle(ui: &mut egui::Ui, id: &str, on: &mut bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(40.0, 24.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let amount = ui
            .ctx()
            .animate_bool_with_time(ui.id().with(id), *on, 0.12);
        let track = blend(
            egui::Color32::from_rgb(76, 76, 76),
            ui_theme_accent(ui),
            amount,
        );
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(12), track);
        let knob_x = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), amount);
        let knob = blend(egui::Color32::from_rgb(232, 232, 232), BLACK, amount);
        ui.painter()
            .circle_filled(egui::pos2(knob_x, rect.center().y), 9.0, knob);
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    let tooltip = if *on {
        "Ativado — clique para desativar"
    } else {
        "Desativado — clique para ativar"
    };
    let response = response.on_hover_text(tooltip);
    if response.clicked() {
        *on = !*on;
        true
    } else {
        false
    }
}

fn settings_toggle_row(ui: &mut egui::Ui, title: &str, description: &str, on: &mut bool) {
    settings_row(ui, title, description, |ui| {
        ui.label(
            egui::RichText::new(if *on { "Ativado" } else { "Desativado" })
                .size(11.5)
                .color(if *on { ui_theme_accent(ui) } else { MUTED }),
        );
        ui.add_space(8.0);
        settings_toggle(ui, title, on);
    });
}

/// Filled pill used for the primary in-page action.
fn accent_chip(ui: &mut egui::Ui, label: &str) -> bool {
    let accent = ui_theme_accent(ui);
    let text_width = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            egui::FontId::proportional(11.5),
            egui::Color32::WHITE,
        )
        .size()
        .x;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(text_width + 28.0, 28.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(14),
            if hovered {
                blend(accent, TEXT, 0.14)
            } else {
                accent
            },
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.5),
            contrast_text(accent),
        );
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    response.clicked()
}

/// Compact pill used for the sync presets, mirroring Spotify's filter chips.
fn settings_chip(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
    let text_width = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            egui::FontId::proportional(11.5),
            egui::Color32::WHITE,
        )
        .size()
        .x;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(text_width + 22.0, 26.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let fill = if active {
            TEXT
        } else if hovered {
            egui::Color32::from_rgb(48, 48, 48)
        } else {
            egui::Color32::from_rgb(34, 34, 34)
        };
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(13), fill);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.5),
            if active { BLACK } else { TEXT_DIM },
        );
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    response.clicked()
}

/// Read-only row with a status dot, used for the live connection diagnostics.
fn settings_status_row(ui: &mut egui::Ui, label: &str, value: &str, ok: bool) {
    settings_row(ui, label, "", |ui| {
        ui.label(egui::RichText::new(value).size(12.5).color(TEXT_DIM));
        ui.add_space(4.0);
        let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(
            dot.center(),
            3.0,
            if ok {
                ui_theme_accent(ui)
            } else {
                egui::Color32::from_rgb(110, 110, 110)
            },
        );
    });
}

/// Number of album covers already cached on disk.
fn cover_cache_count() -> usize {
    cover_cache_dir()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .map(|entries| entries.flatten().count())
        .unwrap_or(0)
}

fn path_box(ui: &mut egui::Ui, path: &str) {
    egui::Frame::new()
        .fill(INSET)
        .stroke(egui::Stroke::new(1.0_f32, BORDER_SOFT))
        .corner_radius(egui::CornerRadius::same(RADIUS_SM))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.add(
                egui::Label::new(egui::RichText::new(path).size(9.5).monospace().color(MUTED))
                    .wrap(),
            );
        });
}

fn filled_button(ui: &mut egui::Ui, label: &str, width: f32, enabled: bool) -> bool {
    let accent = ui_theme_accent(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
    let hovered = enabled && response.hovered();
    let fill = if !enabled {
        blend(SURFACE, accent, 0.12)
    } else if hovered {
        blend(accent, egui::Color32::WHITE, 0.12)
    } else {
        accent
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(16), fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.5),
        if enabled { contrast_text(accent) } else { MUTED },
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && response.clicked()
}

fn ghost_button(ui: &mut egui::Ui, label: &str, width: f32) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(15), SURFACE_HOVER);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.5),
        if hovered { TEXT } else { TEXT_DIM },
    );
    response.clicked()
}

fn emote_hint(raw: &str, accent: egui::Color32) -> (&'static str, egui::Color32) {
    let (name, id) = parse_emoji(raw);
    match (name, id) {
        (None, _) => ("Sem emote neste modo.", MUTED),
        (Some(_), Some(_)) => ("Emote de servidor reconhecido.", accent),
        (Some(_), None) => ("Emoji unicode.", MUTED),
    }
}

fn emote_editor(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    value: &mut String,
    texture: Option<&egui::TextureHandle>,
    active: bool,
) {
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(title).size(10.5).color(TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if active {
                    status_chip(ui, "EM USO", "connected");
                }
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(INSET)
                .stroke(egui::Stroke::new(1.0_f32, BORDER_SOFT))
                .corner_radius(egui::CornerRadius::same(RADIUS_SM))
                .inner_margin(egui::Margin::same(5))
                .show(ui, |ui| {
                    if let Some(texture) = texture {
                        ui.add(
                            egui::Image::new(texture)
                                .fit_to_exact_size(egui::vec2(20.0, 20.0))
                                .corner_radius(egui::CornerRadius::same(5)),
                        );
                    } else {
                        let (name, emote_id) = parse_emoji(value);
                        let preview = if emote_id.is_some() {
                            "?".to_owned()
                        } else {
                            name.unwrap_or_default()
                        };
                        ui.allocate_ui_with_layout(
                            egui::vec2(20.0, 20.0),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| ui.label(egui::RichText::new(preview).size(12.0)),
                        );
                    }
                });
            ui.add(
                egui::TextEdit::singleline(value)
                    .hint_text("🎵 ou nome:id")
                    .desired_width((ui.available_width() - 40.0).max(96.0))
                    .margin(egui::Margin::symmetric(8, 5)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !value.trim().is_empty()
                    && ui
                        .add(
                            egui::Label::new(egui::RichText::new("⨯").size(13.0).color(MUTED))
                                .sense(egui::Sense::click()),
                        )
                        .clicked()
                {
                    value.clear();
                }
            });
        });
        ui.add_space(5.0);
        let accent = ui_theme_accent(ui);
        let (hint, color) = emote_hint(value, accent);
        ui.label(egui::RichText::new(hint).size(8.5).color(color));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for preset in EMOTE_PRESETS {
                let selected = value.trim() == preset;
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(26.0, 24.0), egui::Sense::click());
                if response.clicked() {
                    *value = preset.to_owned();
                }
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(7),
                    if selected {
                        blend(SURFACE, accent, 0.18)
                    } else if response.hovered() {
                        SURFACE_HOVER
                    } else {
                        INSET
                    },
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    preset,
                    egui::FontId::proportional(12.5),
                    if selected { accent } else { TEXT_DIM },
                );
            }
        });
    });
}

fn status_sample(state: &ViewState) -> String {
    match &state.track {
        Some(track) => state
            .lyric_index
            .and_then(|index| state.lyrics.get(index))
            .map(|line| line.text.trim().to_owned())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| format!("{} — {}", track.title, track.artist)),
        None => "Sua música — Artista".to_owned(),
    }
}

fn status_preview(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    emote: &str,
    sample: &str,
) {
    egui::Frame::new()
        .fill(INSET)
        .stroke(egui::Stroke::new(1.0_f32, BORDER_SOFT))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(11, 10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                if let Some(texture) = texture {
                    ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(egui::vec2(15.0, 15.0))
                            .corner_radius(egui::CornerRadius::same(4)),
                    );
                    ui.add_space(2.0);
                } else if !emote.trim().is_empty() {
                    ui.label(egui::RichText::new(emote).size(13.0));
                    ui.add_space(2.0);
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(truncate(sample, 52))
                            .size(11.0)
                            .color(TEXT_DIM),
                    )
                    .wrap(),
                );
            });
        });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    Home,
    Search,
    History,
    Lyrics,
    Settings,
}

/// Icons drawn with primitives, like Spotify's own transport artwork, so the
/// player never depends on whatever glyph a fallback font happens to have.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Glyph {
    Play,
    Pause,
    Prev,
    Next,
    Lyrics,
    Fullscreen,
    ChevronLeft,
    Settings,
}

fn paint_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    glyph: Glyph,
    color: egui::Color32,
) {
    let c = center;
    let s = size * 0.5;
    let stroke = egui::Stroke::new((size * 0.11).max(1.2), color);
    let triangle = |a: egui::Pos2, b: egui::Pos2, d: egui::Pos2| {
        painter.add(egui::Shape::convex_polygon(
            vec![a, b, d],
            color,
            egui::Stroke::NONE,
        ));
    };
    match glyph {
        Glyph::Play => triangle(
            c + egui::vec2(-s * 0.55, -s * 0.9),
            c + egui::vec2(s * 0.9, 0.0),
            c + egui::vec2(-s * 0.55, s * 0.9),
        ),
        Glyph::Pause => {
            for offset in [-s * 0.55, s * 0.19] {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        c + egui::vec2(offset, -s * 0.85),
                        egui::vec2(s * 0.36, s * 1.7),
                    ),
                    egui::CornerRadius::same(1),
                    color,
                );
            }
        }
        Glyph::Prev | Glyph::Next => {
            let dir = if glyph == Glyph::Next { 1.0 } else { -1.0 };
            triangle(
                c + egui::vec2(-s * 0.5 * dir, -s * 0.85),
                c + egui::vec2(s * 0.5 * dir, 0.0),
                c + egui::vec2(-s * 0.5 * dir, s * 0.85),
            );
            let bar_x = c.x + if dir > 0.0 { s * 0.62 } else { -s * 0.9 };
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(bar_x, c.y - s * 0.8),
                    egui::vec2(s * 0.28, s * 1.6),
                ),
                egui::CornerRadius::same(1),
                color,
            );
        }
        Glyph::Lyrics => {
            painter.rect_stroke(
                egui::Rect::from_center_size(
                    c + egui::vec2(0.0, -s * 0.25),
                    egui::vec2(s * 0.8, s * 1.25),
                ),
                egui::CornerRadius::same((s * 0.4).max(2.0) as u8),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [c + egui::vec2(0.0, s * 0.4), c + egui::vec2(0.0, s * 0.75)],
                stroke,
            );
            painter.line_segment(
                [
                    c + egui::vec2(-s * 0.42, s * 0.75),
                    c + egui::vec2(s * 0.42, s * 0.75),
                ],
                stroke,
            );
        }
        Glyph::Fullscreen => {
            let d = s * 0.85;
            let arm = s * 0.5;
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let corner = c + egui::vec2(d * sx, d * sy);
                painter.line_segment([corner, corner + egui::vec2(-arm * sx, 0.0)], stroke);
                painter.line_segment([corner, corner + egui::vec2(0.0, -arm * sy)], stroke);
            }
        }
        Glyph::ChevronLeft => {
            painter.line_segment(
                [
                    c + egui::vec2(s * 0.35, -s * 0.5),
                    c + egui::vec2(-s * 0.3, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    c + egui::vec2(s * 0.35, s * 0.5),
                    c + egui::vec2(-s * 0.3, 0.0),
                ],
                stroke,
            );
        }
        Glyph::Settings => {
            painter.circle_stroke(c, s * 0.42, stroke);
            for step in 0..8 {
                let angle = step as f32 / 8.0 * std::f32::consts::TAU;
                let direction = egui::vec2(angle.cos(), angle.sin());
                painter.line_segment(
                    [c + direction * s * 0.66, c + direction * s * 0.98],
                    stroke,
                );
            }
        }
    }
}

fn paint_icon(painter: &egui::Painter, center: egui::Pos2, icon: Icon, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.4_f32, color);
    match icon {
        Icon::Home => {
            painter.line_segment(
                [
                    center + egui::vec2(-7.0, 0.0),
                    center + egui::vec2(0.0, -6.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(0.0, -6.0),
                    center + egui::vec2(7.0, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-5.0, -1.5),
                    center + egui::vec2(-5.0, 6.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(5.0, -1.5),
                    center + egui::vec2(5.0, 6.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-5.0, 6.0),
                    center + egui::vec2(5.0, 6.0),
                ],
                stroke,
            );
        }
        Icon::Search => {
            painter.circle_stroke(egui::pos2(center.x - 1.5, center.y - 1.5), 5.5, stroke);
            painter.line_segment(
                [
                    egui::pos2(center.x + 2.5, center.y + 2.5),
                    egui::pos2(center.x + 7.0, center.y + 7.0),
                ],
                stroke,
            );
        }
        Icon::History => {
            painter.circle_stroke(center, 7.0, stroke);
            painter.line_segment([center, center + egui::vec2(0.0, -4.0)], stroke);
            painter.line_segment(
                [
                    center + egui::vec2(0.0, -0.5),
                    center + egui::vec2(3.5, 1.5),
                ],
                stroke,
            );
        }
        Icon::Lyrics => {
            for offset in [-5.0_f32, -1.5, 2.0] {
                painter.line_segment(
                    [
                        center + egui::vec2(-6.0, offset),
                        center + egui::vec2(6.0, offset),
                    ],
                    stroke,
                );
            }
            painter.circle_filled(center + egui::vec2(-5.0, 6.0), 1.6, stroke.color);
        }
        Icon::Settings => {
            painter.circle_stroke(center, 3.4, stroke);
            for step in 0..8 {
                let angle = step as f32 / 8.0 * std::f32::consts::TAU;
                let direction = egui::vec2(angle.cos(), angle.sin());
                painter.line_segment(
                    [
                        center + direction * 5.2,
                        center + direction * 7.4,
                    ],
                    stroke,
                );
            }
        }
    }
}

/// Shared column geometry so the header and every row line up exactly.
struct HistoryColumns {
    cover_left: f32,
    cover_size: f32,
    text_left: f32,
    title_width: f32,
    album_left: f32,
    album_width: f32,
    heard_right: f32,
    duration_right: f32,
}

fn history_columns(width: f32) -> HistoryColumns {
    let cover_left = 12.0;
    let cover_size = 40.0;
    let text_left = cover_left + cover_size + 12.0;
    let duration_right = 14.0;
    let heard_right = duration_right + 58.0 + 18.0;
    let title_width = (width * 0.34).clamp(150.0, 340.0);
    let album_width = (width * 0.2).clamp(110.0, 240.0);
    let album_left = text_left + title_width + 16.0;
    HistoryColumns {
        cover_left,
        cover_size,
        text_left,
        title_width,
        album_left,
        album_width,
        heard_right,
        duration_right,
    }
}

fn history_header(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::hover());
    let columns = history_columns(width);
    let font = egui::FontId::proportional(11.0);
    let painter = ui.painter();
    let baseline = rect.center().y;
    painter.text(
        egui::pos2(rect.left() + columns.text_left, baseline),
        egui::Align2::LEFT_CENTER,
        "TÍTULO",
        font.clone(),
        MUTED,
    );
    painter.text(
        egui::pos2(rect.left() + columns.album_left, baseline),
        egui::Align2::LEFT_CENTER,
        "ÁLBUM",
        font.clone(),
        MUTED,
    );
    painter.text(
        egui::pos2(rect.right() - columns.heard_right, baseline),
        egui::Align2::RIGHT_CENTER,
        "OUVIDA",
        font.clone(),
        MUTED,
    );
    painter.text(
        egui::pos2(rect.right() - columns.duration_right, baseline),
        egui::Align2::RIGHT_CENTER,
        "DURAÇÃO",
        font,
        MUTED,
    );
}

fn history_row(
    ui: &mut egui::Ui,
    entry: &HistoryEntry,
    current: Option<&Track>,
    cover_texture: Option<&egui::TextureHandle>,
) {
    let accent = ui_theme_accent(ui);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 56.0), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(
            rect.shrink2(egui::vec2(0.0, 1.0)),
            egui::CornerRadius::same(4),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 14),
        );
    }

    let columns = history_columns(width);
    let cover = egui::Rect::from_min_size(
        egui::pos2(
            rect.left() + columns.cover_left,
            rect.center().y - columns.cover_size * 0.5,
        ),
        egui::vec2(columns.cover_size, columns.cover_size),
    );
    if let Some(texture) = cover_texture {
        egui::Image::new(texture)
            .fit_to_exact_size(cover.size())
            .corner_radius(egui::CornerRadius::same(4))
            .paint_at(ui, cover);
    } else {
        let seed = stable_hash(entry.title.as_bytes()) as u32;
        let start = egui::Color32::from_rgb(
            45 + (seed as u8 % 70),
            45 + ((seed >> 8) as u8 % 70),
            45 + ((seed >> 16) as u8 % 70),
        );
        paint_gradient_rect(painter, cover, start, blend(start, accent, 0.6), 4);
        paint_note_at(painter, cover.center(), 18.0, TEXT);
    }

    let is_current = current.is_some_and(|track| {
        track.title.trim().eq_ignore_ascii_case(&entry.title)
            && track.artist.trim().eq_ignore_ascii_case(&entry.artist)
    });
    let title_chars = ((columns.title_width / 7.2) as usize).max(8);
    painter.text(
        egui::pos2(rect.left() + columns.text_left, rect.center().y - 9.0),
        egui::Align2::LEFT_CENTER,
        truncate(&entry.title, title_chars),
        egui::FontId::proportional(13.5),
        if is_current { accent } else { TEXT },
    );
    painter.text(
        egui::pos2(rect.left() + columns.text_left, rect.center().y + 10.0),
        egui::Align2::LEFT_CENTER,
        truncate(&entry.artist, title_chars),
        egui::FontId::proportional(11.5),
        MUTED,
    );
    painter.text(
        egui::pos2(rect.left() + columns.album_left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        truncate(
            if entry.album.is_empty() {
                "—"
            } else {
                &entry.album
            },
            ((columns.album_width / 6.8) as usize).max(6),
        ),
        egui::FontId::proportional(12.0),
        MUTED,
    );
    painter.text(
        egui::pos2(rect.right() - columns.heard_right, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        relative_played_at(entry.played_at, now_ms() / 1000),
        egui::FontId::proportional(11.5),
        MUTED,
    );
    painter.text(
        egui::pos2(rect.right() - columns.duration_right, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format_time(entry.duration_ms),
        egui::FontId::proportional(11.5),
        if is_current { accent } else { MUTED },
    );
}

fn paint_note(ui: &mut egui::Ui, size: f32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    paint_note_at(ui.painter(), rect.center(), size, color);
}

fn paint_note_at(painter: &egui::Painter, center: egui::Pos2, size: f32, color: egui::Color32) {
    let c = center;
    let s = size / 2.0;
    painter.circle_filled(egui::pos2(c.x - s * 0.26, c.y + s * 0.36), s * 0.26, color);
    painter.circle_filled(egui::pos2(c.x + s * 0.36, c.y + s * 0.16), s * 0.26, color);
    let stroke = egui::Stroke::new((size * 0.07).max(1.4_f32), color);
    painter.line_segment(
        [
            egui::pos2(c.x, c.y + s * 0.34),
            egui::pos2(c.x, c.y - s * 0.44),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x + s * 0.62, c.y + s * 0.14),
            egui::pos2(c.x + s * 0.62, c.y - s * 0.62),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x, c.y - s * 0.44),
            egui::pos2(c.x + s * 0.62, c.y - s * 0.62),
        ],
        stroke,
    );
}

fn icon_button(
    ui: &mut egui::Ui,
    glyph: Glyph,
    size: f32,
    active: bool,
    primary: bool,
    tooltip: &str,
    enabled: bool,
) -> bool {
    let accent = ui_theme_accent(ui);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), sense);
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        if primary {
            let fill = if enabled { TEXT } else { SURFACE_HOVER };
            ui.painter().circle_filled(rect.center(), size * 0.5, fill);
            paint_glyph(
                ui.painter(),
                rect.center(),
                size * 0.44,
                glyph,
                if enabled { BLACK } else { MUTED },
            );
        } else {
            if hovered && enabled {
                ui.painter().circle_filled(
                    rect.center(),
                    size * 0.5,
                    egui::Color32::from_rgb(30, 30, 30),
                );
            }
            let color = if !enabled {
                MUTED
            } else if active {
                accent
            } else if hovered {
                TEXT
            } else {
                MUTED
            };
            paint_glyph(ui.painter(), rect.center(), size * 0.56, glyph, color);
        }
        if hovered && enabled {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    response.on_hover_text(tooltip).clicked() && enabled
}

fn status_chip(ui: &mut egui::Ui, label: &str, status: &str) {
    let color = if status == "connected" {
        ui_theme_accent(ui)
    } else {
        status_color(status)
    };
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), egui::FontId::proportional(9.5), color);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 25.0, 21.0),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(10),
        blend(SURFACE, color, 0.13),
    );
    ui.painter()
        .circle_filled(egui::pos2(rect.left() + 10.0, rect.center().y), 3.0, color);
    ui.painter().galley(
        egui::pos2(
            rect.left() + 17.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
}

fn time_label(ui: &mut egui::Ui, width: f32, text: &str, align_right: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 16.0), egui::Sense::hover());
    let (anchor, align) = if align_right {
        (rect.right_center(), egui::Align2::RIGHT_CENTER)
    } else {
        (rect.left_center(), egui::Align2::LEFT_CENTER)
    };
    ui.painter()
        .text(anchor, align, text, egui::FontId::proportional(11.0), MUTED);
}

fn nav_item(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    active: bool,
    tooltip: &str,
) -> bool {
    let accent = ui_theme_accent(ui);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 38.0), egui::Sense::click());
    let hovered = response.hovered();
    let hover = ui.ctx().animate_bool_with_time(
        egui::Id::new(("nav", label)),
        hovered && !active,
        0.12,
    );
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if active {
            blend(BG, accent, 0.10)
        } else {
            blend(BG, SURFACE_HOVER, hover * 0.45)
        };
        painter.rect_filled(rect, egui::CornerRadius::same(6), fill);
        if active {
            painter.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 2.5, rect.center().y),
                    egui::vec2(3.0, 18.0),
                ),
                egui::CornerRadius::same(2),
                accent,
            );
        }
        let text_color = if active || hovered { TEXT } else { MUTED };
        paint_icon(
            painter,
            egui::pos2(rect.left() + 19.0, rect.center().y),
            icon,
            if active { accent } else { text_color },
        );
        painter.text(
            egui::pos2(rect.left() + 39.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.5),
            text_color,
        );
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    ui.add_space(2.0);
    response.on_hover_text(tooltip).clicked()
}

fn playback_equalizer(ui: &mut egui::Ui, is_playing: bool) {
    let accent = ui_theme_accent(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0, 18.0), egui::Sense::hover());
    let time = ui.ctx().input(|input| input.time) as f32;
    for index in 0..4 {
        let height = if is_playing {
            let wave = ((time * 4.0 + index as f32 * 0.9).sin() + 1.0) * 0.5;
            4.0 + wave * 10.0
        } else {
            4.0
        };
        let bar = egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + 1.0 + index as f32 * 5.5,
                rect.center().y - height * 0.5,
            ),
            egui::vec2(3.0, height),
        );
        ui.painter().rect_filled(
            bar,
            egui::CornerRadius::same(2),
            if is_playing { accent } else { MUTED },
        );
    }
}

fn player_bar_column_widths(width: f32) -> (f32, f32, f32) {
    let left_width = width * 0.30;
    let right_width = width * 0.30;
    let center_width = (width - left_width - right_width).max(0.0);
    (left_width, center_width, right_width)
}

fn seek_bar(ui: &mut egui::Ui, position: u64, duration: u64) -> Option<u64> {
    let width = ui.available_width().max(1.0);
    let sense = if duration > 0 {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 18.0), sense);
    let ratio = if duration > 0 {
        (position as f32 / duration as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let hovered = duration > 0 && (response.hovered() || response.dragged());
    let line = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    {
        let painter = ui.painter();
        painter.rect_filled(line, egui::CornerRadius::same(2), RAIL);
        let filled =
            egui::Rect::from_min_size(line.min, egui::vec2(line.width() * ratio, line.height()));
        let fill = if hovered { ui_theme_accent(ui) } else { TEXT };
        painter.rect_filled(filled, egui::CornerRadius::same(2), fill);
        if hovered {
            painter.circle_filled(
                egui::pos2(line.left() + line.width() * ratio, line.center().y),
                5.5,
                fill,
            );
        }
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if duration == 0 || !(response.clicked() || response.drag_stopped()) {
        return None;
    }
    let pointer = response.interact_pointer_pos()?;
    let ratio = ((pointer.x - line.left()) / line.width()).clamp(0.0, 1.0);
    Some((duration as f64 * ratio as f64).round() as u64)
}

fn draw_avatar(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    name: &str,
    size: f32,
) -> egui::Rect {
    let radius = egui::CornerRadius::same((size / 2.0) as u8);
    if let Some(texture) = texture {
        let response = ui.add(
            egui::Image::new(texture)
                .fit_to_exact_size(egui::vec2(size, size))
                .corner_radius(radius),
        );
        return response.rect;
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, radius, blend(SURFACE, ui_theme_accent(ui), 0.14));
    let initial = name
        .chars()
        .find(|character| !character.is_whitespace())
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(size * 0.42),
        ui_theme_accent(ui),
    );
    rect
}

fn soft_divider(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(0), BORDER_SOFT);
}

fn error_banner(ui: &mut egui::Ui, message: &str) {
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(40, 22, 26))
        .stroke(egui::Stroke::new(
            1.0_f32,
            egui::Color32::from_rgb(86, 40, 48),
        ))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(11, 8))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(message)
                        .size(10.5)
                        .color(egui::Color32::from_rgb(242, 168, 174)),
                )
                .wrap(),
            );
        });
}

fn status_color(status: &str) -> egui::Color32 {
    match status {
        "connected" => ACCENT,
        "waiting" | "checking" => WARNING,
        "no-token" => MUTED,
        _ => DANGER,
    }
}

fn app_icon() -> std::sync::Arc<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../../build/icon.ico"))
        .expect("Failed to load the SpotifySync application icon")
        .into_rgba8();
    std::sync::Arc::new(egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

/// Appends system fonts as fallbacks so Discord nicknames, kanji and symbols
/// that the bundled Ubuntu font lacks still render instead of showing boxes.
fn install_fonts(ctx: &egui::Context) {
    const CANDIDATES: [&str; 6] = [
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "C:\\Windows\\Fonts\\seguisym.ttf",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\yugothr.ttc",
        "C:\\Windows\\Fonts\\meiryo.ttc",
        "C:\\Windows\\Fonts\\malgun.ttf",
    ];

    let mut fonts = egui::FontDefinitions::default();
    // Spotify ships Circular; on Windows Segoe UI is the closest match, so it
    // becomes the primary face and the bundled font stays as a safe fallback.
    if let Ok(bytes) = fs::read("C:\\Windows\\Fonts\\segoeui.ttf") {
        let key = "spotify-ui".to_owned();
        fonts.font_data.insert(
            key.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, key);
    }
    // Segoe UI Semibold for the titles, following Spotify's weight contrast.
    let mut bold_family = Vec::new();
    if let Ok(bytes) = fs::read("C:\\Windows\\Fonts\\seguisb.ttf") {
        let key = "spotify-ui-bold".to_owned();
        fonts.font_data.insert(
            key.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        bold_family.push(key);
    }
    if let Some(regular) = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .and_then(|family| family.first().cloned())
    {
        bold_family.push(regular);
    }
    if !bold_family.is_empty() {
        fonts
            .families
            .insert(egui::FontFamily::Name("SpotifyBold".into()), bold_family);
    }
    let mut added = 0usize;
    for path in CANDIDATES {
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        if bytes.len() < 4 {
            continue;
        }
        let magic = &bytes[..4];
        let sfnt = matches!(
            magic,
            [0, 1, 0, 0]
                | [b't', b't', b'c', b'f']
                | [b'O', b'T', b'T', b'O']
                | [b't', b'r', b'u', b'e']
        );
        if !sfnt {
            continue;
        }
        let key = format!("system-fallback-{added}");
        fonts.font_data.insert(
            key.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(key.clone());
        }
        added += 1;
    }
    let _ = added;
    ctx.set_fonts(fonts);
}

fn setup_visuals(ctx: &egui::Context) {
    install_fonts(ctx);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BLACK;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = egui::Color32::from_rgb(36, 36, 36);
    visuals.faint_bg_color = SURFACE_HOVER;
    visuals.selection.bg_fill = ACCENT_SOFT;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT_DIM);
    visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(RADIUS_SM);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(36, 36, 36);
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(36, 36, 36);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(RADIUS_SM);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(42, 42, 42);
    visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(42, 42, 42);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(RADIUS_SM);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(51, 51, 51);
    visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(51, 51, 51);
    visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(RADIUS_SM);
    visuals.window_corner_radius = egui::CornerRadius::same(12);
    visuals.menu_corner_radius = egui::CornerRadius::same(8);
    visuals.window_stroke = egui::Stroke::new(1.0_f32, BORDER);
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 24,
        spread: 0,
        color: egui::Color32::from_black_alpha(150),
    };
    ctx.set_visuals(visuals);
    apply_theme_visuals(ctx, ACCENT);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(11.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    style.spacing.slider_width = 140.0;
    style.spacing.combo_width = 160.0;
    style.spacing.scroll.bar_width = 7.0;
    style.spacing.scroll.floating = true;
    style.text_styles = [
        (egui::TextStyle::Heading, egui::FontId::proportional(17.0)),
        (egui::TextStyle::Body, egui::FontId::proportional(12.5)),
        (egui::TextStyle::Button, egui::FontId::proportional(11.5)),
        (egui::TextStyle::Small, egui::FontId::proportional(10.5)),
        (egui::TextStyle::Monospace, egui::FontId::monospace(10.5)),
    ]
    .into();
    ctx.set_style(style);
}

fn apply_theme_visuals(ctx: &egui::Context, accent: egui::Color32) {
    let mut visuals = ctx.style().visuals.clone();
    visuals.selection.bg_fill = blend(BG, accent, 0.65);
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, accent);
    visuals.hyperlink_color = accent;
    visuals.text_cursor.stroke = egui::Stroke::new(2.0_f32, accent);
    visuals.widgets.active.bg_fill = blend(SURFACE, accent, 0.32);
    visuals.widgets.active.weak_bg_fill = visuals.widgets.active.bg_fill;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, accent);
    ctx.set_visuals(visuals);
}

fn format_time(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn stable_hash(data: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

#[cfg(windows)]
struct WinRtApartment;

#[cfg(windows)]
impl WinRtApartment {
    fn initialize() -> Result<Self, String> {
        use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)
                .map_err(|error| format!("RoInitialize: {error}"))?;
        }
        Ok(Self)
    }
}

#[cfg(windows)]
impl Drop for WinRtApartment {
    fn drop(&mut self) {
        use windows::Win32::System::WinRT::RoUninitialize;
        unsafe {
            RoUninitialize();
        }
    }
}

#[cfg(windows)]
struct SmtcReader {
    manager: windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager,
}

#[cfg(windows)]
impl SmtcReader {
    fn connect() -> Result<Self, String> {
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .map_err(|error| format!("SMTC: {error}"))?
            .get()
            .map_err(|error| format!("SMTC: {error}"))?;
        Ok(Self { manager })
    }

    fn read_track(&self) -> Result<Option<Track>, String> {
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus;
        let sessions = self
            .manager
            .GetSessions()
            .map_err(|error| format!("SMTC: {error}"))?;
        let count = sessions.Size().map_err(|error| format!("SMTC: {error}"))?;
        let mut fallback: Option<Track> = None;

        for index in 0..count {
            let session = sessions
                .GetAt(index)
                .map_err(|error| format!("SMTC: {error}"))?;
            let source = session
                .SourceAppUserModelId()
                .unwrap_or_default()
                .to_string();
            if !source.to_lowercase().contains("spotify") {
                continue;
            }

            let playback = session
                .GetPlaybackInfo()
                .map_err(|error| format!("SMTC: {error}"))?;
            let is_playing = playback
                .PlaybackStatus()
                .map_err(|error| format!("SMTC: {error}"))?
                == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
            let properties = session
                .TryGetMediaPropertiesAsync()
                .map_err(|error| format!("SMTC: {error}"))?
                .get()
                .map_err(|error| format!("SMTC: {error}"))?;
            let timeline = session
                .GetTimelineProperties()
                .map_err(|error| format!("SMTC: {error}"))?;
            let position_ms = timeline
                .Position()
                .map_err(|error| format!("SMTC: {error}"))?
                .Duration
                .max(0) as u64
                / 10_000;
            let duration_ms = timeline
                .EndTime()
                .map_err(|error| format!("SMTC: {error}"))?
                .Duration
                .max(0) as u64
                / 10_000;
            let title = properties
                .Title()
                .map_err(|error| format!("SMTC: {error}"))?
                .to_string();
            if title.trim().is_empty() {
                continue;
            }
            let artist = properties
                .Artist()
                .map_err(|error| format!("SMTC: {error}"))?
                .to_string();
            let album = properties
                .AlbumTitle()
                .map_err(|error| format!("SMTC: {error}"))?
                .to_string();
            let track = Track {
                title,
                artist,
                album,
                is_playing,
                position_ms,
                duration_ms,
                last_updated_ms: now_ms(),
                platform: "Spotify (Windows SMTC)".to_owned(),
            };
            if is_playing {
                return Ok(Some(track));
            }
            fallback = Some(track);
        }
        Ok(fallback)
    }
}

#[cfg(not(windows))]
struct SmtcReader;

#[cfg(not(windows))]
impl SmtcReader {
    fn connect() -> Result<Self, String> {
        Err("A leitura SMTC está disponível somente no Windows 10/11.".to_owned())
    }

    fn read_track(&self) -> Result<Option<Track>, String> {
        Err("A leitura SMTC está disponível somente no Windows 10/11.".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn league_status_queue_keeps_only_the_latest_pending_update() {
        let (sender, receiver) = mpsc::channel();
        sender.send("first".to_owned()).unwrap();
        sender.send("latest".to_owned()).unwrap();
        let mut desired = Some("previous".to_owned());

        assert!(drain_latest_league_status(&receiver, &mut desired));
        assert_eq!(desired.as_deref(), Some("latest"));
    }

    #[test]
    fn league_status_payload_preserves_other_presence_fields() {
        let payload = league_status_payload(
            serde_json::json!({
                "availability": "chat",
                "statusMessage": "old status"
            }),
            "new status",
        )
        .unwrap();

        assert_eq!(payload["availability"], "chat");
        assert_eq!(payload["statusMessage"], "new status");
    }

    #[test]
    fn artwork_theme_follows_cover_hue_and_falls_back_for_neutral_art() {
        let red = artwork_accent(egui::Color32::from_rgb(190, 62, 54));
        let blue = artwork_accent(egui::Color32::from_rgb(52, 94, 190));
        let neutral = artwork_accent(egui::Color32::from_rgb(120, 120, 120));

        assert!(red.r() > red.b());
        assert!(blue.b() > blue.r());
        assert_eq!(neutral, ACCENT);
    }

    #[test]
    fn artwork_palette_uses_saturated_cover_colors_over_neutral_backgrounds() {
        let mut cover = image::RgbaImage::from_pixel(16, 16, image::Rgba([120, 120, 120, 255]));
        cover.put_pixel(0, 0, image::Rgba([220, 35, 48, 255]));

        let extracted = average_color(&cover);

        assert!(extracted.r() > extracted.g() * 3);
        assert!(extracted.r() > extracted.b() * 2);
    }

    #[test]
    fn contrast_text_keeps_pills_and_chips_readable() {
        assert_eq!(contrast_text(ACCENT), BLACK);
        assert_eq!(contrast_text(egui::Color32::from_rgb(52, 94, 190)), TEXT);
    }

    #[test]
    fn cover_tint_stays_dark_enough_for_white_text() {
        for accent in [
            ACCENT,
            egui::Color32::from_rgb(240, 220, 70),
            egui::Color32::from_rgb(200, 60, 60),
            egui::Color32::from_rgb(40, 60, 160),
        ] {
            assert!(relative_luminance(theme_tint(accent)) < 0.09);
        }
    }

    #[test]
    fn dark_covers_still_get_a_readable_accent() {
        let accent = artwork_accent(egui::Color32::from_rgb(60, 20, 25));
        assert!(relative_luminance(accent) >= 0.15);
    }

    #[test]
    fn artwork_theme_updates_interactive_widget_colors() {
        let ctx = egui::Context::default();
        let accent = egui::Color32::from_rgb(218, 91, 48);

        apply_theme_visuals(&ctx, accent);

        assert_eq!(ctx.style().visuals.selection.stroke.color, accent);
        assert_eq!(ctx.style().visuals.hyperlink_color, accent);
    }

    #[test]
    fn sidebar_blocks_never_overlap_and_stay_inside_the_panel() {
        let panel = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(276.0, 700.0));
        let (recents, now_playing, footer) = sidebar_stack_rects(panel, 280.0);

        assert!(recents.top() >= 280.0);
        assert!(recents.bottom() <= now_playing.top());
        assert!(now_playing.bottom() <= footer.top());
        assert_eq!(footer.bottom(), panel.bottom());
        assert!(now_playing.top() >= panel.top());
    }

    #[test]
    fn player_bar_columns_keep_the_center_exactly_aligned() {
        let (left, center, right) = player_bar_column_widths(1252.0);
        assert!((left - 375.6).abs() < 0.01);
        assert!((center - 500.8).abs() < 0.01);
        assert!((right - 375.6).abs() < 0.01);
        assert!((left + center + right - 1252.0).abs() < 0.01);

        let (left, center, right) = player_bar_column_widths(800.0);
        assert!((left - 240.0).abs() < 0.01);
        assert!((center - 320.0).abs() < 0.01);
        assert!((right - 240.0).abs() < 0.01);
    }

    #[test]
    fn history_search_matches_title_artist_and_album() {
        let entry = HistoryEntry {
            title: "Du hast".to_owned(),
            artist: "Rammstein".to_owned(),
            album: "Sehnsucht".to_owned(),
            ..HistoryEntry::default()
        };
        assert!(history_matches(&entry, "du hast"));
        assert!(history_matches(&entry, "ramm"));
        assert!(history_matches(&entry, "sehnsucht"));
        assert!(history_matches(&entry, ""));
        assert!(!history_matches(&entry, "nirvana"));
    }

    #[test]
    fn listening_span_reads_as_minutes_and_hours() {
        assert_eq!(format_span(0), "—");
        assert_eq!(format_span(90_000), "1 min");
        assert_eq!(format_span(3_600_000), "1h 00");
        assert_eq!(format_span(5_460_000), "1h 31");
    }

    #[test]
    fn emote_editor_stays_inside_its_column() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(420.0, 400.0),
        ));
        let mut value = "nome:123456789012345678".to_owned();
        let mut rect = egui::Rect::NOTHING;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let inner = ui.scope(|ui| {
                    ui.set_max_width(330.0);
                    emote_editor(ui, "test-emote", "Letra da música", &mut value, None, true);
                });
                rect = inner.response.rect;
            });
        });
        assert!(
            rect.width() <= 344.0,
            "largura inesperada: {}",
            rect.width()
        );
        assert!(
            rect.height() <= 150.0,
            "altura inesperada: {}",
            rect.height()
        );
    }

    #[test]
    fn track_id_ignores_duration_and_case() {
        let first = Track {
            title: "Song".to_owned(),
            artist: "Artist".to_owned(),
            duration_ms: 200_000,
            ..Track::default()
        };
        let second = Track {
            title: " song ".to_owned(),
            artist: "ARTIST".to_owned(),
            duration_ms: 0,
            ..Track::default()
        };
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn playback_history_is_newest_first_bounded_and_round_trips() {
        let mut history = Vec::new();
        for played_at in 1..=MAX_HISTORY_ENTRIES as u64 + 1 {
            let track = Track {
                title: format!("Song {played_at}"),
                artist: "Artist".to_owned(),
                album: "Album".to_owned(),
                duration_ms: 185_000,
                ..Track::default()
            };
            record_history(&mut history, &track, played_at);
        }

        assert_eq!(history.len(), MAX_HISTORY_ENTRIES);
        assert_eq!(history[0].played_at, MAX_HISTORY_ENTRIES as u64 + 1);
        assert_eq!(history.last().unwrap().played_at, 2);

        let config = Config {
            recent_tracks: history,
            ..Config::default()
        };
        let serialized = serde_json::to_string(&config).expect("serializar histórico");
        let restored: Config = serde_json::from_str(&serialized).expect("ler histórico");
        assert_eq!(restored.recent_tracks, config.recent_tracks);
    }

    #[test]
    fn replaying_a_track_moves_it_to_the_top_without_duplicating() {
        let mut history = Vec::new();
        let song = Track {
            title: "Song".to_owned(),
            artist: "Artist".to_owned(),
            album: "Album".to_owned(),
            duration_ms: 200_000,
            ..Track::default()
        };
        let other = Track {
            title: "Other".to_owned(),
            artist: "Band".to_owned(),
            ..Track::default()
        };
        record_history(&mut history, &song, 10);
        record_history(&mut history, &other, 20);
        record_history(&mut history, &song, 30);

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].title, "Song");
        assert_eq!(history[0].played_at, 30);

        let mut restored = vec![
            HistoryEntry {
                title: "Song".to_owned(),
                artist: "Artist".to_owned(),
                played_at: 30,
                ..HistoryEntry::default()
            },
            HistoryEntry {
                title: "Song".to_owned(),
                artist: "Artist".to_owned(),
                played_at: 12,
                cover: Some("cover.jpg".to_owned()),
                ..HistoryEntry::default()
            },
        ];
        dedupe_history(&mut restored);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].played_at, 30);
        assert_eq!(restored[0].cover.as_deref(), Some("cover.jpg"));
    }

    #[test]
    fn favorite_artist_ties_stay_stable_when_history_changes() {
        let history = ["Teto", "Able Heart", "Other Artist"]
            .into_iter()
            .map(|artist| HistoryEntry {
                artist: artist.to_owned(),
                ..HistoryEntry::default()
            })
            .collect::<Vec<_>>();
        let reordered = history.iter().rev().cloned().collect::<Vec<_>>();

        assert_eq!(favorite_artist(&history).as_deref(), Some("Able Heart"));
        assert_eq!(favorite_artist(&reordered).as_deref(), Some("Able Heart"));
    }

    #[test]
    fn relative_history_time_handles_future_and_elapsed_entries() {
        assert_eq!(relative_played_at(1_100, 1_000), "Agora");
        assert_eq!(relative_played_at(940, 1_000), "há 1 min");
        assert_eq!(relative_played_at(1_000, 87_400), "há 1 dia");
    }

    #[test]
    fn throttled_status_error_waits_for_the_server_interval() {
        let error = StatusError::throttled(Duration::from_secs(5));
        assert_eq!(error.retry_after, Some(Duration::from_secs(5)));
        assert_eq!(error.pause(), Duration::from_millis(5_250));
        assert_eq!(StatusError::new("boom").pause(), Duration::from_millis(1_500));
    }

    #[test]
    fn greeting_follows_the_period_of_the_day() {
        assert_eq!(greeting_for_hour(5), "Bom dia");
        assert_eq!(greeting_for_hour(11), "Bom dia");
        assert_eq!(greeting_for_hour(12), "Boa tarde");
        assert_eq!(greeting_for_hour(17), "Boa tarde");
        assert_eq!(greeting_for_hour(18), "Boa noite");
        assert_eq!(greeting_for_hour(23), "Boa noite");
        assert_eq!(greeting_for_hour(2), "Boa noite");
    }

    #[test]
    fn playback_clock_ignores_stale_reads_and_follows_seeks() {
        // Spotify still reports 10.0 s but 1.2 s have passed: keep the
        // interpolated clock so the lyric line does not lag behind.
        assert_eq!(
            advance_position(10_000, 1_200, 10_000, 10_000),
            (11_200, false, false)
        );
        // A real forward step re-anchors.
        assert_eq!(
            advance_position(10_000, 1_200, 12_400, 10_000),
            (12_400, true, false)
        );
        // A backwards seek is applied and flagged so the lyric may rewind.
        assert_eq!(
            advance_position(30_000, 200, 5_000, 30_000),
            (5_000, true, true)
        );
        // Buffering lag: Spotify's timeline barely moved while the clock ran
        // ahead. Trust Spotify, but never report a seek.
        assert_eq!(
            advance_position(10_000, 6_000, 13_000, 12_800),
            (13_000, true, false)
        );
        // A frozen timeline read must not rewind the position at all.
        assert_eq!(
            advance_position(10_000, 6_000, 10_000, 10_000),
            (16_000, false, false)
        );
    }

    #[test]
    fn lyric_line_never_moves_backwards_while_playing() {
        assert_eq!(hold_lyric_floor(Some(4), Some(3)), Some(4));
        assert_eq!(hold_lyric_floor(Some(4), None), Some(4));
        assert_eq!(hold_lyric_floor(Some(4), Some(5)), Some(5));
        assert_eq!(hold_lyric_floor(None, Some(2)), Some(2));
    }

    #[test]
    fn strips_noise_from_track_titles() {
        assert_eq!(strip_track_suffixes("Song (feat. X)"), "Song");
        assert_eq!(strip_track_suffixes("Song [Remastered]"), "Song");
        assert_eq!(strip_track_suffixes("Song - 2011 Remaster"), "Song");
        assert_eq!(strip_track_suffixes("Song - Live at Home"), "Song");
        assert_eq!(strip_track_suffixes("Song - Part 2"), "Song - Part 2");
        assert_eq!(strip_track_suffixes("  Song  "), "Song");
    }

    #[test]
    fn extracts_primary_artist() {
        assert_eq!(primary_artist("A, B").as_deref(), Some("A"));
        assert_eq!(primary_artist("A feat. B").as_deref(), Some("A"));
        assert_eq!(primary_artist("A & B").as_deref(), Some("A"));
        assert_eq!(primary_artist("A - Topic").as_deref(), Some("A"));
        assert_eq!(primary_artist("A"), None);
    }

    #[test]
    fn prefers_candidate_closest_to_playing_duration() {
        let items = vec![
            serde_json::json!({"duration": 300.0, "syncedLyrics": "[00:01.00]a"}),
            serde_json::json!({"duration": 201.0, "syncedLyrics": "[00:01.00]b"}),
            serde_json::json!({"duration": 199.0, "syncedLyrics": null, "plainLyrics": "c\nd\ne"}),
        ];
        let best = pick_best(&items, 200_000, true).expect("candidate");
        assert_eq!(best["duration"], serde_json::json!(201.0));
    }

    #[test]
    fn system_fonts_cover_wide_glyph_sets() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(480.0, 320.0),
        ));
        let mut widths = Vec::new();
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for sample in ["日本語", "ｱｲｳ", "★☆", "Buffy"] {
                    let galley = ui.painter().layout_no_wrap(
                        sample.to_owned(),
                        egui::FontId::proportional(14.0),
                        egui::Color32::WHITE,
                    );
                    widths.push(galley.rect.width());
                }
            });
        });
        assert!(widths.iter().all(|width| *width > 1.0));
    }

    #[test]
    fn parses_multiple_lrc_timestamps_and_fractional_precision() {
        let parsed = parse_lrc("[00:01.20][00:02.34]<00:01.20>Lin<00:01.70>ha\n[01:03.456]Outra");
        assert_eq!(
            parsed,
            vec![
                LyricLine {
                    time_ms: 1200,
                    text: "Linha".to_owned(),
                },
                LyricLine {
                    time_ms: 2340,
                    text: "Linha".to_owned(),
                },
                LyricLine {
                    time_ms: 63_456,
                    text: "Outra".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn ignores_invalid_lrc_timestamps() {
        assert!(parse_lrc("[not-a-time]Sem timestamp").is_empty());
        assert_eq!(parse_timestamp("00:61.00"), None);
        assert_eq!(parse_timestamp("00:01.0000"), None);
    }

    #[test]
    fn selects_latest_lyric_at_or_before_position() {
        let lines = parse_lrc("[00:01.00]A\n[00:03.00]B");
        assert_eq!(active_lyric(&lines, 2999), Some(0));
        assert_eq!(active_lyric(&lines, 3000), Some(1));
        assert_eq!(active_lyric(&lines, 0), None);
    }

    #[test]
    fn distributes_plain_lyrics_and_rejects_too_short_results() {
        let lines = distribute_plain_lyrics("a\n\nb\nc", 9000);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2].time_ms, 6000);
        assert!(distribute_plain_lyrics("a\nb", 9000).is_empty());
    }

    #[test]
    fn tempo_adjusts_the_manual_lyric_lead_by_a_quarter_beat() {
        assert_eq!(effective_lead_ms(500, None), 500);
        assert_eq!(effective_lead_ms(500, Some(60)), 750);
        assert_eq!(effective_lead_ms(500, Some(120)), 625);
        assert_eq!(effective_lead_ms(500, Some(200)), 575);
        assert_eq!(effective_lead_ms(0, Some(200)), 0);
        assert_eq!(effective_lead_ms(1_500, Some(220)), 1_500);
    }

    #[test]
    fn estimates_tempo_from_synced_lyric_cadence() {
        let lines = parse_lrc("[00:10.00]a\n[00:12.00]b\n[00:13.80]c\n[00:16.00]d\n[00:18.00]e");
        assert_eq!(lyric_cadence_bpm(&lines, true), Some(120));
        let steady = parse_lrc("[00:10.00]a\n[00:12.00]b\n[00:14.00]c\n[00:16.00]d\n[00:18.00]e");
        assert_eq!(lyric_cadence_bpm(&steady, true), Some(120));
        let plain = distribute_plain_lyrics("a\nb\nc\nd\ne\nf", 12_000);
        assert_eq!(lyric_cadence_bpm(&plain, false), None);
        assert_eq!(lyric_cadence_bpm(&steady, false), None);
        assert_eq!(lyric_cadence_bpm(&parse_lrc("[00:01.00]a"), true), None);
    }

    #[test]
    fn limits_lookahead_to_avoid_skipping_close_lyrics() {
        let lines = parse_lrc("[00:01.00]a\n[00:02.00]b\n[00:04.00]c\n[00:06.00]d\n[00:08.00]e");
        assert_eq!(cap_lyric_lookahead_ms(&lines, 1_000), 500);
        assert_eq!(cap_lyric_lookahead_ms(&lines, 300), 300);
        assert_eq!(cap_lyric_lookahead_ms(&[], 1_000), 1_000);
    }

    #[test]
    fn update_checks_only_accept_newer_releases() {
        assert!(is_newer_version("v1.7.2", "1.7.1"));
        assert!(is_newer_version("1.7.10", "1.7.9"));
        assert!(is_newer_version("v2.0.0", "1.9.9"));
        assert!(is_newer_version("v1.8", "1.7.1"));
        assert!(!is_newer_version("v1.7.1", "1.7.1"));
        assert!(!is_newer_version("v1.6.9", "1.7.1"));
        assert!(!is_newer_version("", "1.7.1"));
        assert!(!is_newer_version("nightly", "1.7.1"));
    }

    #[test]
    fn update_feed_uses_the_github_releases_api() {
        let endpoint = latest_release_endpoint().expect("endpoint");
        assert!(endpoint.starts_with("https://api.github.com/repos/"));
        assert!(endpoint.ends_with("/releases/latest"));
    }

    #[test]
    fn update_release_accepts_github_and_manifest_shapes() {
        let github = serde_json::json!({
            "tag_name": "v1.8.0",
            "assets": [
                { "name": "notes.txt", "browser_download_url": "https://example.com/notes.txt" },
                { "name": "SpotifySync.exe", "browser_download_url": "https://example.com/app.exe" }
            ]
        });
        assert_eq!(
            parse_release(&github),
            Some((
                "1.8.0".to_owned(),
                Some("https://example.com/app.exe".to_owned())
            ))
        );
        let manifest = serde_json::json!({
            "version": "v1.9.2",
            "url": "https://example.com/SpotifySync.exe"
        });
        assert_eq!(
            parse_release(&manifest),
            Some((
                "1.9.2".to_owned(),
                Some("https://example.com/SpotifySync.exe".to_owned())
            ))
        );
        assert_eq!(parse_release(&serde_json::json!({ "assets": [] })), None);
        assert_eq!(parse_release(&serde_json::json!({ "version": "" })), None);
    }

    #[test]
    fn only_loopback_update_feeds_skip_tls() {
        assert!(is_loopback_url("http://127.0.0.1:8123/feed.json"));
        assert!(is_loopback_url("http://localhost:8123/feed.json"));
        assert!(!is_loopback_url("http://example.com/feed.json"));
        assert!(!is_loopback_url("https://api.github.com/repos/a/b/releases/latest"));
    }

    /// Serves a manifest and an executable from loopback and checks that a
    /// release newer than the running version is downloaded and stored.
    #[test]
    fn updater_downloads_a_newer_release_from_a_local_feed() {
        use std::io::{Read, Write};
        let executable =
            std::fs::read(std::env::current_exe().expect("test executable")).expect("executable");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();
        let feed = serde_json::json!({
            "version": "99.0.0",
            "url": format!("http://127.0.0.1:{port}/SpotifySync.exe")
        })
        .to_string();
        let payload = executable.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buffer = [0u8; 2048];
                let _ = stream.read(&mut buffer);
                let request = String::from_utf8_lossy(&buffer);
                let body = if request.starts_with("GET /feed.json") {
                    feed.clone().into_bytes()
                } else {
                    payload.clone()
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        std::env::set_var(
            "SPOTIFYSYNC_UPDATE_FEED",
            format!("http://127.0.0.1:{port}/feed.json"),
        );
        let pending = pending_update_path().expect("pending path");
        let attempt = update_attempt_path("99.0.0").expect("attempt path");
        let _ = std::fs::remove_file(&pending);
        let _ = std::fs::remove_file(&attempt);
        let state = std::sync::Arc::new(std::sync::Mutex::new(ViewState::default()));
        run_update_check(&state, false);
        std::env::remove_var("SPOTIFYSYNC_UPDATE_FEED");
        let downloaded = std::fs::read(&pending).expect("downloaded update");
        assert_eq!(downloaded.len(), executable.len());
        assert!(downloaded.starts_with(b"MZ"));
        let view = state.lock().expect("state");
        assert!(view.update.ready, "mensagem: {}", view.update.message);
        assert!(view.update.message.contains("99.0.0"));
        drop(view);
        let _ = std::fs::remove_file(&pending);
        let _ = std::fs::remove_file(&attempt);
        server.join().ok();
    }

    #[test]
    fn reads_discord_token_from_dotenv_without_persisting_it() {
        let contents = "# local settings\nOTHER=value\nDISCORD_USER_TOKEN='sample-token'\n";
        assert_eq!(
            parse_dotenv_token(contents),
            Some("sample-token".to_owned())
        );
    }

    #[test]
    fn config_never_serializes_the_token() {
        let config = Config {
            user_token: Some("super-secret-token".to_owned()),
            ..Config::default()
        };
        let json = serde_json::to_string(&config).expect("json");
        assert!(!json.contains("super-secret-token"));
        assert!(!json.contains("userToken"));
    }

    #[test]
    fn wiping_a_token_clears_the_buffer() {
        let mut token = "secret-token".to_owned();
        let length = token.len();
        let pointer = token.as_ptr();
        wipe_secret(&mut token);
        assert!(token.is_empty());
        let bytes = unsafe { std::slice::from_raw_parts(pointer, length) };
        assert!(bytes.iter().all(|byte| *byte == 0));
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_keeps_the_token_readable_only_by_windows() {
        let secret = b"local-only-token-value";
        let protected = protect_secret(secret).expect("proteger");
        assert_ne!(protected.as_slice(), secret);
        assert!(!String::from_utf8_lossy(&protected).contains("local-only-token-value"));
        assert_eq!(
            unprotect_secret(&protected).expect("abrir"),
            secret.to_vec()
        );
    }
}
