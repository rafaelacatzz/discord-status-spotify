#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const APP_NAME: &str = "SpotifySync";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const LEGACY_CONFIG_DIR: &str = "discord-lyrics-status";
const DISCORD_SETTINGS_URL: &str = "https://discord.com/api/v9/users/@me/settings";
const DISCORD_ME_URL: &str = "https://discord.com/api/v9/users/@me";
const DEFAULT_SYNC_LEAD_MS: u16 = 500;
const BG: egui::Color32 = egui::Color32::from_rgb(9, 10, 13);
const SIDEBAR_BG: egui::Color32 = egui::Color32::from_rgb(12, 14, 18);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(17, 20, 25);
const SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(24, 28, 35);
const INSET: egui::Color32 = egui::Color32::from_rgb(11, 13, 17);
const BORDER: egui::Color32 = egui::Color32::from_rgb(33, 38, 47);
const BORDER_SOFT: egui::Color32 = egui::Color32::from_rgb(25, 29, 36);
const TEXT: egui::Color32 = egui::Color32::from_rgb(238, 242, 248);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(186, 194, 206);
const MUTED: egui::Color32 = egui::Color32::from_rgb(124, 133, 148);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(30, 215, 96);
const ACCENT_SOFT: egui::Color32 = egui::Color32::from_rgb(18, 46, 29);
const ACCENT_DIM: egui::Color32 = egui::Color32::from_rgb(22, 38, 28);
const ON_ACCENT: egui::Color32 = egui::Color32::from_rgb(6, 20, 11);
const WARNING: egui::Color32 = egui::Color32::from_rgb(240, 180, 90);
const DANGER: egui::Color32 = egui::Color32::from_rgb(240, 110, 110);
const RADIUS_LG: u8 = 18;
const RADIUS_MD: u8 = 12;
const RADIUS_SM: u8 = 8;
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
    lyrics: Vec<LyricLine>,
    lyric_index: Option<usize>,
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
    #[serde(rename = "accountId")]
    account_id: Option<String>,
    #[serde(rename = "accountName")]
    account_name: Option<String>,
    #[serde(rename = "accountAvatar")]
    account_avatar: Option<String>,
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
    }
}

enum Command {
    SaveToken(String),
    SignOut,
    Seek(u64),
    UpdateSettings(Settings),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Player,
    Lyrics,
    Settings,
}

struct NativeApp {
    state: Arc<Mutex<ViewState>>,
    command_tx: mpsc::Sender<Command>,
    page: Page,
    token_input: String,
    show_token: bool,
    settings: Settings,
    logo_texture: egui::TextureHandle,
    texture: Option<egui::TextureHandle>,
    texture_hash: u64,
    art_tint: egui::Color32,
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
}

impl NativeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_visuals(&cc.egui_ctx);
        let mut config = load_config();
        migrate_legacy_token(&mut config);
        let token = configured_token().or_else(load_local_token);
        let settings = Settings::from_config(&config);
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
        spawn_worker(state.clone(), command_rx, config, token, settings.clone());

        Self {
            state,
            command_tx,
            page: Page::Player,
            token_input: String::new(),
            show_token: false,
            last_sent_settings: settings.clone(),
            settings,
            logo_texture,
            texture: None,
            texture_hash: 0,
            art_tint: SURFACE,
            avatar_texture: None,
            avatar_hash: 0,
            emote_lyric_texture: None,
            emote_lyric_hash: 0,
            emote_track_texture: None,
            emote_track_hash: 0,
            saved_flash: None,
            last_scrolled_lyric: None,
            ui_error: None,
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

        if let Some(bytes) = &state.artwork {
            if let Ok(image) = image::load_from_memory(bytes) {
                let rgba = image.to_rgba8();
                self.art_tint = average_color(&rgba);
                let size = [rgba.width() as usize, rgba.height() as usize];
                let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                self.texture =
                    Some(ctx.load_texture("album-art", color_image, egui::TextureOptions::LINEAR));
            }
        } else {
            self.texture = None;
            self.art_tint = SURFACE;
        }
        self.texture_hash = artwork_hash;
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

    fn draw_sidebar(&mut self, ctx: &egui::Context, state: &ViewState) {
        egui::SidePanel::left("navigation")
            .exact_width(244.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(SIDEBAR_BG)
                    .inner_margin(egui::Margin {
                        left: 16,
                        right: 16,
                        top: 20,
                        bottom: 16,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.image((self.logo_texture.id(), egui::vec2(32.0, 32.0)));
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(APP_NAME).size(14.5).strong().color(TEXT));
                        ui.add_space(1.0);
                        ui.label(
                            egui::RichText::new("MUSIC COMPANION")
                                .size(8.0)
                                .strong()
                                .color(MUTED),
                        );
                    });
                });
                ui.add_space(28.0);
                ui.label(kicker("MENU"));
                ui.add_space(10.0);
                if nav_item(ui, Icon::Play, "Tocando agora", self.page == Page::Player) {
                    self.page = Page::Player;
                }
                if nav_item(ui, Icon::Lyrics, "Letras", self.page == Page::Lyrics) {
                    self.page = Page::Lyrics;
                }
                if nav_item(ui, Icon::Sliders, "Configurações", self.page == Page::Settings) {
                    self.page = Page::Settings;
                }

                ui.add_space(26.0);
                if let Some(track) = &state.track {
                    ui.label(kicker("REPRODUZINDO"));
                    ui.add_space(8.0);
                    mini_track_card(ui, track, self.texture.as_ref());
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(format!("v{APP_VERSION}  ·  Rust + egui"))
                            .size(8.5)
                            .color(MUTED),
                    );
                    ui.add_space(11.0);
                    soft_divider(ui);
                    ui.add_space(11.0);
                    connection_row(ui, "Letras", lyrics_connection(state));
                    connection_row(ui, "Discord", &state.discord_status);
                    connection_row(
                        ui,
                        "Spotify",
                        if state.track.is_some() {
                            "connected"
                        } else {
                            "waiting"
                        },
                    );
                    ui.add_space(11.0);
                    ui.label(kicker("CONEXÕES"));
                    ui.add_space(11.0);
                    if let Some(account) = &state.account {
                        account_card(ui, self.avatar_texture.as_ref(), account);
                        ui.add_space(12.0);
                    }
                });
            });
    }

    fn draw_header(&self, ui: &mut egui::Ui, state: &ViewState) {
        let (title, subtitle) = match self.page {
            Page::Settings => ("Configurações", "Conta, status e sincronização"),
            Page::Lyrics => ("Letras", "Acompanhe cada linha em tempo real"),
            Page::Player => ("Sua música", "Tudo o que está tocando, em um só lugar"),
        };
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(title).size(20.0).strong().color(TEXT));
                ui.add_space(3.0);
                ui.label(egui::RichText::new(subtitle).size(11.0).color(MUTED));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(account) = &state.account {
                    egui::Frame::new()
                        .fill(SURFACE)
                        .stroke(egui::Stroke::new(1.0_f32, BORDER))
                        .corner_radius(egui::CornerRadius::same(14))
                        .inner_margin(egui::Margin {
                            left: 5,
                            right: 10,
                            top: 4,
                            bottom: 4,
                        })
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                draw_avatar(ui, self.avatar_texture.as_ref(), &account.name, 20.0);
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(truncate(&account.name, 16))
                                        .size(10.5)
                                        .color(TEXT_DIM),
                                );
                            });
                        });
                    ui.add_space(8.0);
                }
                status_chip(ui, "Discord", &state.discord_status);
                if state.track.as_ref().is_some_and(|track| track.is_playing) {
                    ui.add_space(6.0);
                    status_chip(ui, "AO VIVO", "connected");
                }
            });
        });
        ui.add_space(18.0);
    }

    fn draw_login(&mut self, ctx: &egui::Context, state: &ViewState) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("login-scroll")
                    .show(ui, |ui| {
                        let card_width = ui.available_width().min(500.0).max(320.0);
                        ui.add_space(48.0);
                        ui.vertical_centered(|ui| {
                            egui::Frame::new()
                                .fill(blend(SURFACE, ACCENT, 0.10))
                                .stroke(egui::Stroke::new(1.0_f32, blend(BORDER, ACCENT, 0.24)))
                                .corner_radius(egui::CornerRadius::same(20))
                                .inner_margin(egui::Margin::same(9))
                                .show(ui, |ui| {
                                    ui.image((self.logo_texture.id(), egui::vec2(42.0, 42.0)))
                                });
                            ui.add_space(16.0);
                            ui.label(egui::RichText::new(APP_NAME).size(27.0).strong().color(TEXT));
                            ui.add_space(5.0);
                            ui.label(
                                egui::RichText::new(
                                    "Sua música no status do Discord, linha por linha.",
                                )
                                .size(12.5)
                                .color(MUTED),
                            );
                        });
                        ui.add_space(24.0);
                        ui.vertical_centered(|ui| {
                            ui.set_max_width(card_width);
                            card(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(kicker("CONEXÃO"));
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
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new("Conectar sua conta")
                                        .size(17.0)
                                        .strong()
                                        .color(TEXT),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Cole o token do Discord. Ele é validado antes de ser salvo, criptografado com o Windows (DPAPI) e fica apenas neste computador.",
                                    )
                                    .size(10.5)
                                    .color(MUTED),
                                );
                                ui.add_space(14.0);
                                let response = ui.add(
                                    egui::TextEdit::singleline(&mut self.token_input)
                                        .password(!self.show_token)
                                        .hint_text("Cole seu token do Discord")
                                        .desired_width(f32::INFINITY)
                                        .margin(egui::Margin::symmetric(9, 7)),
                                );
                                let submitted = response.lost_focus()
                                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
                                ui.add_space(9.0);
                                let ready = !self.token_input.trim().is_empty()
                                    && state.auth != AuthState::Validating;
                                let mut connect = false;
                                ui.horizontal(|ui| {
                                    if ghost_button(
                                        ui,
                                        if self.show_token { "Ocultar" } else { "Mostrar" },
                                        84.0,
                                    ) {
                                        self.show_token = !self.show_token;
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            connect = filled_button(ui, "Conectar", 128.0, ready);
                                        },
                                    );
                                });
                                if connect || submitted {
                                    self.submit_token();
                                }
                                ui.add_space(14.0);
                                soft_divider(ui);
                                ui.add_space(11.0);
                                ui.label(
                                    egui::RichText::new(&state.token_status)
                                        .size(10.5)
                                        .color(MUTED),
                                );
                                if let Some(error) = &self.ui_error {
                                    ui.add_space(9.0);
                                    error_banner(ui, error);
                                }
                                ui.add_space(10.0);
                                egui::CollapsingHeader::new(
                                    egui::RichText::new("Como obtenho meu token?")
                                        .size(10.5)
                                        .color(TEXT_DIM),
                                )
                                .id_salt("token-help")
                                .show(ui, |ui| {
                                    ui.add_space(4.0);
                                    for step in [
                                        "1. Abra o Discord no navegador e entre na sua conta.",
                                        "2. Pressione F12 e abra a aba Rede (Network).",
                                        "3. Procure qualquer requisição para discord.com/api.",
                                        "4. Copie o valor do cabeçalho authorization.",
                                    ] {
                                        ui.label(
                                            egui::RichText::new(step).size(10.5).color(MUTED),
                                        );
                                    }
                                    ui.add_space(6.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "Aviso: usar token de usuário pode violar os Termos de Serviço do Discord.",
                                        )
                                        .size(10.0)
                                        .color(WARNING),
                                    );
                                });
                            });
                            ui.add_space(14.0);
                            ui.label(
                                egui::RichText::new(
                                    "O token fica somente neste computador, na pasta de configuração do app.",
                                )
                                .size(10.0)
                                .color(MUTED),
                            );
                            ui.add_space(24.0);
                        });
                    });
            });
    }

    fn draw_home(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        let total = ui.available_width().max(320.0);
        if total < 760.0 {
            self.draw_home_hero(ui, state);
            ui.add_space(14.0);
            profile_card(
                ui,
                state,
                &self.settings,
                self.avatar_texture.as_ref(),
                self.emote_lyric_texture.as_ref(),
                self.emote_track_texture.as_ref(),
            );
            ui.add_space(14.0);
            connections_card(ui, state);
            ui.add_space(14.0);
            self.draw_lyrics_preview(ui, state);
            return;
        }

        let hero_width = (total * 0.62).clamp(300.0, (total - 250.0).max(300.0));
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(hero_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.draw_home_hero(ui, state),
            );
            ui.add_space(16.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    profile_card(
                        ui,
                        state,
                        &self.settings,
                        self.avatar_texture.as_ref(),
                        self.emote_lyric_texture.as_ref(),
                        self.emote_track_texture.as_ref(),
                    );
                    ui.add_space(16.0);
                    connections_card(ui, state);
                },
            );
        });
        ui.add_space(16.0);
        self.draw_lyrics_preview(ui, state);
    }

    fn draw_home_hero(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        egui::Frame::new()
            .fill(blend(SURFACE, self.art_tint, 0.12))
            .stroke(egui::Stroke::new(
                1.0_f32,
                blend(BORDER, self.art_tint, 0.32),
            ))
            .corner_radius(egui::CornerRadius::same(RADIUS_LG))
            .inner_margin(egui::Margin::same(22))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if let Some(track) = &state.track {
                    ui.horizontal_top(|ui| {
                        egui::Frame::new()
                            .fill(INSET)
                            .stroke(egui::Stroke::new(1.0_f32, BORDER))
                            .corner_radius(egui::CornerRadius::same(16))
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                if let Some(texture) = &self.texture {
                                    ui.add(
                                        egui::Image::new(texture)
                                            .fit_to_exact_size(egui::vec2(188.0, 188.0))
                                            .corner_radius(egui::CornerRadius::same(11)),
                                    );
                                } else {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(188.0, 188.0),
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
                                "REPRODUÇÃO"
                            }));
                            ui.add_space(9.0);
                            ui.label(
                                egui::RichText::new(truncate(&track.title, 32))
                                    .size(23.0)
                                    .strong()
                                    .color(TEXT),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(truncate(&track.artist, 38))
                                    .size(13.5)
                                    .color(TEXT_DIM),
                            );
                            if !track.album.is_empty() {
                                ui.add_space(3.0);
                                ui.label(
                                    egui::RichText::new(truncate(&track.album, 40))
                                        .size(10.5)
                                        .color(MUTED),
                                );
                            }
                        });
                    });
                    ui.add_space(18.0);
                    let position = track.estimated_position().min(track.duration_ms);
                    if let Some(target) = seek_bar(ui, position, track.duration_ms) {
                        if let Err(error) = self.command_tx.send(Command::Seek(target)) {
                            self.ui_error = Some(format!("Falha ao buscar faixa: {error}"));
                        }
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format_time(position))
                                .size(10.5)
                                .color(MUTED),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "-{}",
                                        format_time(track.duration_ms.saturating_sub(position))
                                    ))
                                    .size(10.5)
                                    .color(MUTED),
                                );
                            },
                        );
                    });
                    ui.add_space(12.0);
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
                                            ACCENT
                                        }),
                                );
                            },
                        );
                    });
                } else {
                    ui.add_space(18.0);
                    ui.vertical_centered(|ui| {
                        egui::Frame::new()
                            .fill(blend(SURFACE, ACCENT, 0.08))
                            .stroke(egui::Stroke::new(1.0_f32, blend(BORDER, ACCENT, 0.18)))
                            .corner_radius(egui::CornerRadius::same(20))
                            .inner_margin(egui::Margin::same(18))
                            .show(ui, |ui| paint_note(ui, 44.0, ACCENT));
                        ui.add_space(16.0);
                        ui.label(
                            egui::RichText::new("Nada tocando agora")
                                .size(21.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new("Abra o Spotify e dê play em qualquer faixa.")
                                .size(12.0)
                                .color(MUTED),
                        );
                        ui.add_space(16.0);
                        status_chip(ui, "AGUARDANDO SPOTIFY", "waiting");
                    });
                    ui.add_space(18.0);
                }
                if let Some(error) = state.last_error.as_ref().or(self.ui_error.as_ref()) {
                    ui.add_space(6.0);
                    error_banner(ui, error);
                }
            });
    }

    fn draw_lyrics_preview(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(texture) = &self.texture {
                    ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(egui::vec2(40.0, 40.0))
                            .corner_radius(egui::CornerRadius::same(9)),
                    );
                    ui.add_space(10.0);
                }
                ui.vertical(|ui| {
                    if let Some(track) = &state.track {
                        ui.label(kicker("AGORA NA FAIXA"));
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(truncate(&track.title, 44))
                                .size(14.5)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            egui::RichText::new(truncate(&track.artist, 50))
                                .size(10.5)
                                .color(MUTED),
                        );
                    } else {
                        ui.label(kicker("LETRAS SINCRONIZADAS"));
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Acompanhe a música, linha por linha.")
                                .size(14.5)
                                .strong()
                                .color(TEXT),
                        );
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ghost_button(ui, "Abrir letras", 96.0) {
                        self.page = Page::Lyrics;
                    }
                    ui.add_space(2.0);
                    if !state.lyrics.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} linhas · {}",
                                state.lyrics.len(),
                                state.lyrics_status
                            ))
                            .size(9.5)
                            .color(MUTED),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(&state.lyrics_status)
                                .size(9.5)
                                .color(MUTED),
                        );
                    }
                });
            });
            ui.add_space(12.0);
            soft_divider(ui);
            ui.add_space(12.0);
            if state.lyrics.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(6.0);
                    paint_note(ui, 32.0, blend(MUTED, ACCENT, 0.35));
                    ui.add_space(9.0);
                    ui.label(
                        egui::RichText::new(&state.lyrics_status)
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.add_space(6.0);
                });
                self.last_scrolled_lyric = None;
                return;
            }
            let start = state.lyric_index.unwrap_or(0).min(state.lyrics.len() - 1);
            let first = start.saturating_sub(1);
            let last = (start + 2).min(state.lyrics.len() - 1);
            for index in first..=last {
                let line = &state.lyrics[index];
                let active = Some(index) == state.lyric_index;
                ui.add_space(2.0);
                ui.label(
                    (if active {
                        egui::RichText::new(&line.text).strong()
                    } else {
                        egui::RichText::new(&line.text)
                    })
                    .size(if active { 17.0 } else { 13.5 })
                    .color(if active { ACCENT } else { MUTED }),
                );
            }
            ui.add_space(4.0);
        });
    }

    fn draw_lyrics_page(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        let card_height = ui.available_height().max(240.0);
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, BORDER))
            .corner_radius(egui::CornerRadius::same(RADIUS_LG))
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
                            ui.label(
                                egui::RichText::new("Nada tocando")
                                    .size(15.0)
                                    .color(MUTED),
                            );
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
                                paint_note(ui, 38.0, blend(MUTED, ACCENT, 0.4));
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
                            });
                            self.last_scrolled_lyric = None;
                            return;
                        }
                        for (index, line) in state.lyrics.iter().enumerate() {
                            let active = state.lyric_index == Some(index);
                            let row = egui::Frame::new()
                                .fill(if active { ACCENT_DIM } else { egui::Color32::TRANSPARENT })
                                .stroke(if active {
                                    egui::Stroke::new(1.0_f32, blend(BORDER, ACCENT, 0.35))
                                } else {
                                    egui::Stroke::NONE
                                })
                                .corner_radius(egui::CornerRadius::same(RADIUS_MD))
                                .inner_margin(egui::Margin::symmetric(12, 8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        if self.settings.show_timestamps {
                                            ui.label(
                                                egui::RichText::new(format_time(line.time_ms))
                                                    .size(9.5)
                                                    .color(if active { ACCENT } else { MUTED }),
                                            );
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                (if active {
                                                    egui::RichText::new(&line.text).strong()
                                                } else {
                                                    egui::RichText::new(&line.text)
                                                })
                                                .size(if active { 18.0 } else { 14.0 })
                                                .color(if active {
                                                    TEXT
                                                } else {
                                                    TEXT_DIM
                                                }),
                                            )
                                            .wrap(),
                                        );
                                    });
                                });
                            if active && self.last_scrolled_lyric != state.lyric_index {
                                row.response.scroll_to_me(Some(egui::Align::Center));
                            }
                            ui.add_space(2.0);
                        }
                        self.last_scrolled_lyric = state.lyric_index;
                    });
            });
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui, state: &ViewState) {
        ui.columns(2, |columns| {
            card(&mut columns[0], |ui| {
                section_header(
                    ui,
                    "CONTA",
                    "Sua conta do Discord",
                    "A sessão fica salva como preferência neste computador.",
                );
                ui.horizontal(|ui| {
                    let name = state
                        .account
                        .as_ref()
                        .map(|account| account.name.clone())
                        .unwrap_or_else(|| "?".to_owned());
                    draw_avatar(ui, self.avatar_texture.as_ref(), &name, 42.0);
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new(if state.account.is_some() {
                                name.clone()
                            } else {
                                "Nenhuma conta".to_owned()
                            })
                            .size(14.5)
                            .strong()
                            .color(TEXT),
                        );
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(&state.token_status)
                                .size(9.5)
                                .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if state.account.is_some() && ghost_button(ui, "Sair", 62.0) {
                            self.sign_out();
                        }
                    });
                });
                ui.add_space(12.0);
                if let Some(account) = &state.account {
                    ui.label(
                        egui::RichText::new(format!("ID {}", account.id))
                            .size(9.0)
                            .color(MUTED),
                    );
                    ui.add_space(6.0);
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Token criptografado com Windows DPAPI em token.bin: só a sua conta do Windows consegue ler, e ele nunca vai para outros serviços.",
                        )
                        .size(8.5)
                        .color(MUTED),
                    )
                    .wrap(),
                );
            });

            columns[0].add_space(14.0);
            card(&mut columns[0], |ui| {
                section_header(
                    ui,
                    "STATUS",
                    "O que aparece no seu perfil",
                    "Escolha a presença; o texto é escolhido sozinho pela música.",
                );
                control_row(ui, "Presença", |ui| {
                    egui::ComboBox::from_id_salt("presence")
                        .width(148.0)
                        .selected_text(presence_label(&self.settings.presence))
                        .show_ui(ui, |ui| {
                            for option in ["online", "idle", "dnd", "invisible"] {
                                ui.selectable_value(
                                    &mut self.settings.presence,
                                    option.to_owned(),
                                    presence_label(option),
                                );
                            }
                        });
                });
                ui.add_space(10.0);
                ui.label(kicker("PRÉVIA"));
                ui.add_space(6.0);
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
                let emote_text = emote_name.unwrap_or_default();
                status_preview(ui, emote_texture.as_ref(), &emote_text, &sample);
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(if has_lyric {
                        "Letra sincronizada com a música."
                    } else {
                        "Sem letra: título e artista."
                    })
                    .size(9.0)
                    .color(MUTED),
                );
            });

            card(&mut columns[1], |ui| {
                section_header(
                    ui,
                    "EMOTES",
                    "Emojis personalizados",
                    "Um emote para a letra e outro para título e artista, trocados sozinhos.",
                );
                let has_lyric = has_active_lyric(state);
                emote_editor(
                    ui,
                    "emote-lyric",
                    "Letra da música",
                    &mut self.settings.lyric_emoji,
                    self.emote_lyric_texture.as_ref(),
                    has_lyric,
                );
                ui.add_space(10.0);
                soft_divider(ui);
                ui.add_space(10.0);
                emote_editor(
                    ui,
                    "emote-track",
                    "Título e artista",
                    &mut self.settings.track_emoji,
                    self.emote_track_texture.as_ref(),
                    !has_lyric,
                );
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(
                        "Aceita emoji (🎵) ou emote de servidor como nome:123456789012345678. Emotes de servidor só aparecem se sua conta puder usá-los.",
                    )
                    .size(9.0)
                    .color(MUTED),
                );
            });

            columns[1].add_space(14.0);
            card(&mut columns[1], |ui| {
                section_header(
                    ui,
                    "SINCRONIZAÇÃO",
                    "Ajuste fino",
                    "Compense atrasos e controle o consumo do app.",
                );
                control_row(ui, "Adiantar letra", |ui| {
                    ui.add(
                        egui::Slider::new(&mut self.settings.sync_lead_ms, 0..=1_500)
                            .suffix(" ms"),
                    );
                });
                ui.label(
                    egui::RichText::new(
                        "Adianta a linha da letra para compensar o atraso de exibição do Discord.",
                    )
                    .size(9.0)
                    .color(MUTED),
                );
                ui.add_space(12.0);
                toggle_row(
                    ui,
                    "Mostrar horário das linhas",
                    "Exibe o tempo ao lado de cada linha nas letras.",
                    &mut self.settings.show_timestamps,
                );
                ui.add_space(10.0);
                toggle_row(
                    ui,
                    "Modo econômico",
                    "Lê a reprodução com menos frequência em segundo plano.",
                    &mut self.settings.eco_mode,
                );
            });

            columns[1].add_space(14.0);
            card(&mut columns[1], |ui| {
                section_header(
                    ui,
                    "DADOS",
                    "Arquivo de configuração",
                    "Preferências e sessão ficam em um JSON local.",
                );
                let path_text = config_path().map(|path| path.display().to_string());
                if let Some(path) = &path_text {
                    path_box(ui, path);
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ghost_button(ui, "Abrir pasta", 92.0) {
                        open_config_dir();
                    }
                    if ghost_button(ui, "Copiar", 68.0) {
                        if let Some(path) = &path_text {
                            ui.ctx().copy_text(path.clone());
                        }
                    }
                    if ghost_button(ui, "Restaurar", 84.0) {
                        self.settings = Settings::default();
                    }
                });
                ui.add_space(10.0);
                if self.settings != self.last_sent_settings {
                    ui.label(egui::RichText::new("Salvando…").size(9.5).color(MUTED));
                } else if self
                    .saved_flash
                    .map(|at| at.elapsed() < Duration::from_secs(3))
                    .unwrap_or(false)
                {
                    ui.label(
                        egui::RichText::new("Preferências salvas")
                            .size(9.5)
                            .color(ACCENT),
                    );
                }
                ui.add_space(10.0);
                soft_divider(ui);
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{APP_NAME} v{APP_VERSION} · Windows SMTC · egui"
                    ))
                    .size(9.0)
                    .color(MUTED),
                );
            });

            if let Some(error) = &self.ui_error {
                columns[0].add_space(14.0);
                error_banner(&mut columns[0], error);
            }
        });

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

        if state.account.is_none() && state.auth != AuthState::SignedIn {
            self.draw_login(ctx, &state);
            return;
        }

        self.draw_sidebar(ctx, &state);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin {
                left: 26,
                right: 26,
                top: 22,
                bottom: 16,
            }))
            .show(ctx, |ui| {
                self.draw_header(ui, &state);
                if self.page == Page::Lyrics {
                    self.draw_lyrics_page(ui, &state);
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("main-content")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            match self.page {
                                Page::Settings => self.draw_settings(ui, &state),
                                _ => self.draw_home(ui, &state),
                            }
                            ui.add_space(10.0);
                        });
                }
            });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 800.0])
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
            .user_agent(concat!("SpotifySync/", env!("CARGO_PKG_VERSION"), " (native Rust)"))
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
        let mut current_track_id = String::new();
        let mut smtc_reader: Option<SmtcReader> = None;
        let mut lyrics = Vec::new();
        let mut lyrics_cache: HashMap<String, (Vec<LyricLine>, &'static str)> = HashMap::new();
        let mut last_lyric_text = String::new();
        let mut lyric_index = None;
        let mut lyrics_attempts = 0u32;
        let mut next_lyrics_retry: Option<Instant> = None;
        let mut smtc_error_reported = false;
        let mut last_status_text = String::new();
        let mut last_status_sent_at = Instant::now() - Duration::from_secs(10);
        let mut next_status_attempt = Instant::now();
        // Adaptive gap between updates: tight while Discord accepts them, backed
        // off as soon as it throttles.
        let mut status_interval = Duration::from_millis(900);
        let mut last_settings_write = Instant::now() - Duration::from_secs(5);
        let mut cached_lyric_emote_id: Option<String> = None;
        let mut cached_track_emote_id: Option<String> = None;
        let mut clock = PlaybackClock::new();

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
                            let _ = clear_discord_status(
                                &client,
                                &current,
                                settings.presence.as_str(),
                            );
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
                    Command::UpdateSettings(next) => {
                        let presence_changed = settings.presence != next.presence;
                        settings = next;
                        settings.apply(&mut config);
                        if presence_changed {
                            last_status_text.clear();
                        }
                        if last_settings_write.elapsed() >= Duration::from_millis(400) {
                            if let Err(error) = save_config(&config) {
                                set_error(
                                    &state,
                                    format!("Falha ao salvar configurações: {error}"),
                                );
                            } else {
                                last_settings_write = Instant::now();
                            }
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
                            next_lyrics_retry = if lyrics.is_empty() {
                                Some(Instant::now() + Duration::from_secs(5))
                            } else {
                                None
                            };
                            let artwork = fetch_artwork(&client, &track);
                            if let Ok(mut view) = state.lock() {
                                view.lyrics = lyrics.clone();
                                view.lyrics_status = if lyrics.is_empty() {
                                    "Não encontradas".to_owned()
                                } else {
                                    format!("{} linhas ({source})", lyrics.len())
                                };
                                view.artwork = artwork;
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
                                    next_lyrics_retry = None;
                                    if let Ok(mut view) = state.lock() {
                                        view.lyrics = lyrics.clone();
                                        view.lyrics_status =
                                            format!("{} linhas ({source})", lyrics.len());
                                    }
                                }
                            }
                        }

                        let (position, seek) =
                            clock.position(track.position_ms, track.is_playing);
                        let candidate_index = active_lyric(
                            &lyrics,
                            position.saturating_add(settings.sync_lead_ms as u64),
                        );
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

                        if let Some(token) = token.as_deref() {
                            if track.is_playing {
                                let current_line = active_index
                                    .and_then(|index| lyrics.get(index))
                                    .map(|line| line.text.trim())
                                    .filter(|text| !text.is_empty());
                                if let Some(text) = current_line {
                                    last_lyric_text = text.to_owned();
                                }
                                // Never flash the title in the middle of a song:
                                // if the current index is momentarily missing, keep
                                // the last line that was already showing.
                                let lyric_text = current_line.map(ToOwned::to_owned).or_else(|| {
                                    (!lyrics.is_empty() && !last_lyric_text.is_empty())
                                        .then(|| last_lyric_text.clone())
                                });
                                let (status_text, has_lyric) = match lyric_text {
                                    Some(text) => (text, true),
                                    None => (
                                        format!("{} — {}", track.title, track.artist),
                                        false,
                                    ),
                                };
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
                                        // Discord throttles this endpoint; pause
                                        // instead of hammering it every tick.
                                        let pause = if error.contains("429") {
                                            status_interval = (status_interval * 2)
                                                .min(Duration::from_millis(4_000));
                                            Duration::from_secs(8)
                                        } else {
                                            Duration::from_secs(2)
                                        };
                                        next_status_attempt = Instant::now() + pause;
                                        set_discord_status(&state, format!("Erro: {error}"));
                                    } else {
                                        last_status_text = status_text;
                                        last_status_sent_at = Instant::now();
                                        status_interval = status_interval
                                            .saturating_sub(Duration::from_millis(100))
                                            .max(Duration::from_millis(400));
                                        set_discord_status(&state, "connected".to_owned());
                                    }
                                }
                            } else if !last_status_text.is_empty() {
                                if let Err(error) = clear_discord_status(
                                    &client,
                                    token,
                                    settings.presence.as_str(),
                                ) {
                                    set_discord_status(&state, format!("Erro: {error}"));
                                } else {
                                    last_status_text.clear();
                                }
                            }
                        }
                    } else {
                        if let Some(token) = token.as_deref() {
                            if !last_status_text.is_empty() {
                                if let Err(error) = clear_discord_status(
                                    &client,
                                    token,
                                    settings.presence.as_str(),
                                ) {
                                    set_discord_status(&state, format!("Erro: {error}"));
                                } else {
                                    last_status_text.clear();
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
                        last_status_text.clear();
                        if let Ok(mut view) = state.lock() {
                            view.track = None;
                            view.lyrics.clear();
                            view.lyric_index = None;
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

            let delay = if settings.eco_mode { 2000 } else { 250 };
            thread::sleep(Duration::from_millis(delay));
        }
    });
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
    Err("O armazenamento seguro do token usa o Windows DPAPI e só existe no Windows 10/11."
        .to_owned())
}

#[cfg(not(windows))]
fn unprotect_secret(_blob: &[u8]) -> Result<Vec<u8>, String> {
    Err("O armazenamento seguro do token usa o Windows DPAPI e só existe no Windows 10/11."
        .to_owned())
}

fn save_config(config: &Config) -> Result<(), String> {
    let path = config_path().ok_or_else(|| "Pasta de configuração indisponível".to_owned())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Caminho de configuração inválido".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let json = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
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

fn update_discord_status(
    client: &Client,
    token: &str,
    text: &str,
    presence: &str,
    emoji: &str,
) -> Result<(), String> {
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
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .unwrap_or(8);
            return Err(format!(
                "HTTP 429 · limite de atualizações do Discord, tentando de novo em {wait}s"
            ));
        }
        Err(format!("Discord retornou HTTP {status}"))
    }
}

fn clear_discord_status(client: &Client, token: &str, presence: &str) -> Result<(), String> {
    let response = client
        .patch(DISCORD_SETTINGS_URL)
        .header("Authorization", token)
        .json(&serde_json::json!({
            "status": presence,
            "custom_status": null
        }))
        .send()
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("Discord retornou HTTP {}", response.status()))
    }
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
        " feat. ", " feat ", " Feat. ", " Feat ", " Featuring ", " featuring ", " & ", ", ",
        " x ", " X ", " vs. ", " / ",
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
fn pick_best(items: &[serde_json::Value], duration_ms: u64, synced: bool) -> Option<&serde_json::Value> {
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
    let best = pick_best(&items, duration_ms, true).or_else(|| pick_best(&items, duration_ms, false))?;
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
    let best = pick_best(&items, duration_ms, true).or_else(|| pick_best(&items, duration_ms, false))?;
    lyrics_from_payload(best, duration_ms)
}

fn fetch_lyrics(client: &Client, track: &Track) -> (Vec<LyricLine>, &'static str) {
    let titles = distinct(vec![track.title.clone(), strip_track_suffixes(&track.title)]);
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

fn set_error(state: &Arc<Mutex<ViewState>>, error: String) {
    if let Ok(mut view) = state.lock() {
        view.last_error = Some(error);
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

fn average_color(image: &image::RgbaImage) -> egui::Color32 {
    let mut total = [0u64; 3];
    let mut count = 0u64;
    let step = ((image.width() as usize * image.height() as usize) / 512).max(1);
    for (index, pixel) in image.pixels().enumerate() {
        if index % step != 0 {
            continue;
        }
        total[0] += pixel[0] as u64;
        total[1] += pixel[1] as u64;
        total[2] += pixel[2] as u64;
        count += 1;
    }
    if count == 0 {
        return SURFACE;
    }
    egui::Color32::from_rgb(
        (total[0] / count) as u8,
        (total[1] / count) as u8,
        (total[2] / count) as u8,
    )
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

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, BORDER))
        .corner_radius(egui::CornerRadius::same(RADIUS_LG))
        .inner_margin(egui::Margin::same(20))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn section_header(ui: &mut egui::Ui, kicker_text: &str, title: &str, description: &str) {
    ui.label(kicker(kicker_text));
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(title)
            .size(14.5)
            .strong()
            .color(TEXT),
    );
    ui.add_space(4.0);
    ui.label(egui::RichText::new(description).size(10.0).color(MUTED));
    ui.add_space(13.0);
    soft_divider(ui);
    ui.add_space(13.0);
}

fn control_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(11.0).color(TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            add(ui);
        });
    });
    ui.add_space(6.0);
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
                egui::Label::new(
                    egui::RichText::new(path)
                        .size(9.5)
                        .monospace()
                        .color(MUTED),
                )
                .wrap(),
            );
        });
}

fn filled_button(ui: &mut egui::Ui, label: &str, width: f32, enabled: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
    let hovered = enabled && response.hovered();
    let fill = if !enabled {
        egui::Color32::from_rgb(36, 50, 41)
    } else if hovered {
        blend(ACCENT, egui::Color32::WHITE, 0.12)
    } else {
        ACCENT
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(10), fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.5),
        if enabled { ON_ACCENT } else { MUTED },
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && response.clicked()
}

fn ghost_button(ui: &mut egui::Ui, label: &str, width: f32) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(9), SURFACE_HOVER);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(10.5),
        if hovered { TEXT } else { TEXT_DIM },
    );
    response.clicked()
}

fn switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(38.0, 20.0), egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let amount = ui
        .ctx()
        .animate_bool_with_time(egui::Id::new(("switch", response.id)), *on, 0.12);
    let track = blend(egui::Color32::from_rgb(38, 44, 54), ACCENT, amount);
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(10), track);
    let knob_x = rect.left() + 10.0 + (rect.width() - 20.0) * amount;
    ui.painter().circle_filled(
        egui::pos2(knob_x, rect.center().y),
        7.0,
        if amount > 0.5 { ON_ACCENT } else { TEXT_DIM },
    );
    response
}

fn toggle_row(ui: &mut egui::Ui, title: &str, description: &str, value: &mut bool) {
    ui.horizontal(|ui| {
        let text_width = (ui.available_width() - 50.0).max(90.0);
        ui.allocate_ui_with_layout(
            egui::vec2(text_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.label(egui::RichText::new(title).size(11.0).color(TEXT_DIM));
                ui.add(
                    egui::Label::new(egui::RichText::new(description).size(9.0).color(MUTED))
                        .wrap(),
                );
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            switch(ui, value);
        });
    });
}

fn emote_hint(raw: &str) -> (&'static str, egui::Color32) {
    let (name, id) = parse_emoji(raw);
    match (name, id) {
        (None, _) => ("Sem emote neste modo.", MUTED),
        (Some(_), Some(_)) => ("Emote de servidor reconhecido.", ACCENT),
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
                    .desired_width(ui.available_width() - 40.0)
                    .margin(egui::Margin::symmetric(8, 5)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !value.trim().is_empty()
                    && ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new("⨯").size(13.0).color(MUTED),
                            )
                            .sense(egui::Sense::click()),
                        )
                        .clicked()
                {
                    value.clear();
                }
            });
        });
        ui.add_space(5.0);
        let (hint, color) = emote_hint(value);
        ui.label(egui::RichText::new(hint).size(8.5).color(color));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for preset in EMOTE_PRESETS {
                let selected = value.trim() == preset;
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(26.0, 24.0),
                    egui::Sense::click(),
                );
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
                        blend(SURFACE, ACCENT, 0.18)
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
                    if selected { ACCENT } else { TEXT_DIM },
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

fn next_lyric(state: &ViewState) -> Option<String> {
    let start = state.lyric_index.map(|index| index + 1).unwrap_or(0);
    state
        .lyrics
        .iter()
        .skip(start)
        .map(|line| line.text.trim())
        .find(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn presence_status(presence: &str) -> &'static str {
    match presence {
        "idle" => "waiting",
        "dnd" => "error",
        "invisible" => "no-token",
        _ => "connected",
    }
}

fn lyrics_connection(state: &ViewState) -> &'static str {
    if state.track.is_none() {
        "no-token"
    } else if state.lyrics.is_empty() {
        "waiting"
    } else {
        "connected"
    }
}

fn profile_card(
    ui: &mut egui::Ui,
    state: &ViewState,
    settings: &Settings,
    avatar_texture: Option<&egui::TextureHandle>,
    emote_lyric: Option<&egui::TextureHandle>,
    emote_track: Option<&egui::TextureHandle>,
) {
    let name = state
        .account
        .as_ref()
        .map(|account| account.name.clone())
        .unwrap_or_else(|| "Sua conta".to_owned());
    let has_lyric = has_active_lyric(state);
    let sample = status_sample(state);
    let (emote_name, emote_id) = parse_emoji(settings.emoji_for_lyric(has_lyric));
    let emote_texture = if emote_id.is_some() {
        if has_lyric {
            emote_lyric
        } else {
            emote_track
        }
    } else {
        None
    };
    let emote_text = emote_name.unwrap_or_default();

    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(kicker("NO SEU PERFIL"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                status_chip(
                    ui,
                    presence_label(&settings.presence),
                    presence_status(&settings.presence),
                );
            });
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Status do Discord")
                .size(14.5)
                .strong()
                .color(TEXT),
        );
        ui.add_space(12.0);
        egui::Frame::new()
            .fill(INSET)
            .stroke(egui::Stroke::new(1.0_f32, BORDER_SOFT))
            .corner_radius(egui::CornerRadius::same(RADIUS_MD))
            .inner_margin(egui::Margin::symmetric(11, 10))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    draw_avatar(ui, avatar_texture, &name, 34.0);
                    ui.add_space(3.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(truncate(&name, 15))
                                .size(11.5)
                                .strong()
                                .color(TEXT),
                        );
                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            if let Some(texture) = emote_texture {
                                ui.add(
                                    egui::Image::new(texture)
                                        .fit_to_exact_size(egui::vec2(13.0, 13.0))
                                        .corner_radius(egui::CornerRadius::same(3)),
                                );
                            } else if !emote_text.trim().is_empty() {
                                ui.label(egui::RichText::new(&emote_text).size(11.0));
                            }
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(truncate(&sample, 22))
                                        .size(9.5)
                                        .color(TEXT_DIM),
                                )
                                .wrap(),
                            );
                        });
                    });
                });
            });
        ui.add_space(9.0);
        let hint = if has_lyric {
            "A linha da letra entra sozinha, sincronizada."
        } else if state.track.is_some() {
            "Sem letra sincronizada: título e artista."
        } else {
            "Aparece aqui assim que algo tocar."
        };
        ui.label(egui::RichText::new(hint).size(9.0).color(MUTED));
        ui.add_space(13.0);
        soft_divider(ui);
        ui.add_space(11.0);
        ui.label(kicker("A SEGUIR"));
        ui.add_space(5.0);
        let upcoming = next_lyric(state);
        ui.add(
            egui::Label::new(
                egui::RichText::new(upcoming.as_deref().unwrap_or("—"))
                    .size(11.0)
                    .color(if upcoming.is_some() { TEXT_DIM } else { MUTED }),
            )
            .wrap(),
        );
    });
}

fn connections_card(ui: &mut egui::Ui, state: &ViewState) {
    card(ui, |ui| {
        ui.label(kicker("CONEXÕES"));
        ui.add_space(9.0);
        connection_row(
            ui,
            "Spotify",
            if state.track.is_some() {
                "connected"
            } else {
                "waiting"
            },
        );
        connection_row(ui, "Discord", &state.discord_status);
        connection_row(ui, "Letras", lyrics_connection(state));
        ui.add_space(9.0);
        soft_divider(ui);
        ui.add_space(9.0);
        ui.label(
            egui::RichText::new(&state.lyrics_status)
                .size(9.0)
                .color(MUTED),
        );
    });
}

fn mini_track_card(ui: &mut egui::Ui, track: &Track, artwork: Option<&egui::TextureHandle>) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, BORDER_SOFT))
        .corner_radius(egui::CornerRadius::same(RADIUS_MD))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                if let Some(texture) = artwork {
                    ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(egui::vec2(34.0, 34.0))
                            .corner_radius(egui::CornerRadius::same(RADIUS_SM)),
                    );
                } else {
                    egui::Frame::new()
                        .fill(blend(SURFACE, ACCENT, 0.10))
                        .corner_radius(egui::CornerRadius::same(RADIUS_SM))
                        .inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| paint_note(ui, 22.0, ACCENT));
                }
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.add_space(1.0);
                    ui.label(
                        egui::RichText::new(truncate(&track.title, 18))
                            .size(10.5)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        egui::RichText::new(truncate(&track.artist, 20))
                            .size(9.0)
                            .color(MUTED),
                    );
                });
            });
        });
}

fn account_card(ui: &mut egui::Ui, texture: Option<&egui::TextureHandle>, account: &Account) {
    egui::Frame::new()
        .fill(blend(SURFACE, ACCENT, 0.05))
        .stroke(egui::Stroke::new(1.0_f32, BORDER_SOFT))
        .corner_radius(egui::CornerRadius::same(RADIUS_MD))
        .inner_margin(egui::Margin::symmetric(9, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let rect = draw_avatar(ui, texture, &account.name, 30.0);
                let dot = rect.right_bottom() + egui::vec2(-2.0, -2.0);
                {
                    let painter = ui.painter();
                    painter.circle_filled(dot, 5.0, SIDEBAR_BG);
                    painter.circle_filled(dot, 3.0, ACCENT);
                }
                ui.add_space(7.0);
                ui.vertical(|ui| {
                    ui.add_space(1.0);
                    ui.label(
                        egui::RichText::new(truncate(&account.name, 15))
                            .size(10.5)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(egui::RichText::new("Sessão ativa").size(8.5).color(ACCENT));
                });
            });
        });
}

#[derive(Clone, Copy)]
enum Icon {
    Play,
    Lyrics,
    Sliders,
}

fn paint_icon(painter: &egui::Painter, center: egui::Pos2, icon: Icon, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.4_f32, color);
    match icon {
        Icon::Play => {
            painter.circle_stroke(center, 6.5, stroke);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    center + egui::vec2(-1.8, -3.1),
                    center + egui::vec2(-1.8, 3.1),
                    center + egui::vec2(3.2, 0.0),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::Lyrics => {
            for (index, width) in [11.0_f32, 8.0, 9.5].iter().enumerate() {
                let y = center.y - 5.0 + index as f32 * 5.0;
                painter.line_segment(
                    [
                        egui::pos2(center.x - 6.0, y),
                        egui::pos2(center.x - 6.0 + width, y),
                    ],
                    stroke,
                );
            }
        }
        Icon::Sliders => {
            for index in 0..2 {
                let y = center.y - 4.0 + index as f32 * 8.0;
                painter.line_segment(
                    [
                        egui::pos2(center.x - 6.0, y),
                        egui::pos2(center.x + 6.0, y),
                    ],
                    stroke,
                );
                let knob = center.x + if index == 0 { 2.5 } else { -2.8 };
                painter.circle_filled(egui::pos2(knob, y), 2.2, color);
            }
        }
    }
}

fn paint_note(ui: &mut egui::Ui, size: f32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let c = rect.center();
    let s = size / 2.0;
    let painter = ui.painter();
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

fn nav_item(ui: &mut egui::Ui, icon: Icon, label: &str, active: bool) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 36.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let hover = ui
            .ctx()
            .animate_bool_with_time(egui::Id::new(("nav", label)), hovered && !active, 0.14);
        let painter = ui.painter();
        if active {
            painter.rect_filled(rect, egui::CornerRadius::same(10), SURFACE);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 1.0, rect.center().y - 8.0),
                    egui::vec2(3.0, 16.0),
                ),
                egui::CornerRadius::same(2),
                ACCENT,
            );
        } else if hover > 0.0 {
            painter.rect_filled(
                rect,
                egui::CornerRadius::same(10),
                blend(SIDEBAR_BG, SURFACE, hover * 0.8),
            );
        }
        let text_color = if active {
            TEXT
        } else if hovered {
            TEXT_DIM
        } else {
            MUTED
        };
        paint_icon(
            painter,
            egui::pos2(rect.left() + 19.0, rect.center().y),
            icon,
            if active { ACCENT } else { text_color },
        );
        painter.text(
            egui::pos2(rect.left() + 37.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(11.5),
            text_color,
        );
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    ui.add_space(3.0);
    response.clicked()
}

fn seek_bar(ui: &mut egui::Ui, position: u64, duration: u64) -> Option<u64> {
    let width = ui.available_width().max(120.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, 18.0), egui::Sense::click_and_drag());
    let line = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    let ratio = if duration > 0 {
        (position as f32 / duration as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let hovered = response.hovered() || response.dragged();
    {
        let painter = ui.painter();
        painter.rect_filled(line, egui::CornerRadius::same(2), BORDER);
        let filled =
            egui::Rect::from_min_size(line.min, egui::vec2(line.width() * ratio, line.height()));
        painter.rect_filled(filled, egui::CornerRadius::same(2), ACCENT);
        painter.circle_filled(
            egui::pos2(line.left() + line.width() * ratio, line.center().y),
            if hovered { 5.5 } else { 3.5 },
            ACCENT,
        );
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
        .rect_filled(rect, radius, blend(SURFACE, ACCENT, 0.14));
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
        ACCENT,
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
        .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(86, 40, 48)))
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

fn status_label(status: &str) -> &'static str {
    match status {
        "connected" => "Ativo",
        "waiting" => "Aguardando",
        "no-token" => "Sem sessão",
        "checking" => "Verificando",
        _ if status.starts_with("Erro") => "Falhou",
        _ => "Verificando",
    }
}

fn status_chip(ui: &mut egui::Ui, label: &str, status: &str) {
    let color = status_color(status);
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

fn connection_row(ui: &mut egui::Ui, label: &str, status: &str) {
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 3.0, status_color(status));
        ui.add_space(3.0);
        ui.label(egui::RichText::new(label).size(10.5).color(TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(status_label(status))
                    .size(9.0)
                    .color(MUTED),
            );
        });
    });
    ui.add_space(4.0);
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
            [0, 1, 0, 0] | [b't', b't', b'c', b'f'] | [b'O', b'T', b'T', b'O'] | [b't', b'r', b'u', b'e']
        );
        if !sfnt {
            continue;
        }
        let key = format!("system-fallback-{added}");
        fonts
            .font_data
            .insert(key.clone(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(key.clone());
        }
        added += 1;
    }
    if added > 0 {
        ctx.set_fonts(fonts);
    }
}

fn setup_visuals(ctx: &egui::Context) {
    install_fonts(ctx);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = INSET;
    visuals.faint_bg_color = SURFACE_HOVER;
    visuals.selection.bg_fill = ACCENT_SOFT;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT_DIM);
    visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(RADIUS_SM);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(29, 34, 42);
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(29, 34, 42);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(RADIUS_SM);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(37, 43, 53);
    visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(37, 43, 53);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(64, 73, 86));
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(RADIUS_SM);
    visuals.widgets.active.bg_fill = ACCENT_SOFT;
    visuals.widgets.active.weak_bg_fill = ACCENT_SOFT;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(RADIUS_SM);
    visuals.window_corner_radius = egui::CornerRadius::same(16);
    visuals.menu_corner_radius = egui::CornerRadius::same(12);
    visuals.window_stroke = egui::Stroke::new(1.0_f32, BORDER);
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 24,
        spread: 0,
        color: egui::Color32::from_black_alpha(150),
    };
    ctx.set_visuals(visuals);

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
                    emote_editor(
                        ui,
                        "test-emote",
                        "Letra da música",
                        &mut value,
                        None,
                        true,
                    );
                });
                rect = inner.response.rect;
            });
        });
        assert!(rect.width() <= 344.0, "largura inesperada: {}", rect.width());
        assert!(rect.height() <= 150.0, "altura inesperada: {}", rect.height());
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
