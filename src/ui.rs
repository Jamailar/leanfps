//! The small window.
//!
//! Look and feel follow the app icon: a graphite tile with a mint "leaning L". Both themes share
//! the mint gradient; dark mode keeps the icon's graphite, light mode puts the same mint on paper.

use crate::catalog::{Catalog, Genre, Risk, Scope, Tweak};
use crate::config::{Config, Tier, PRICE};
use crate::engine::{Engine, TweakState};
use crate::games::GameDb;
use crate::license::{ApiError, Device, Licenses, State as LicenseState};
use crate::store::Store;
use crate::watcher::{Command, Shared};
use eframe::egui::{
    self, epaint::RectShape, pos2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Shadow,
    Stroke, StrokeKind, TextureHandle, TextureId, Ui, Vec2,
};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const WINDOW_SIZE: [f32; 2] = [380.0, 660.0];

// ------------------------------------------------------------------------ theme

/// One theme's colors. Read with [`pal`], which follows the active egui theme.
pub struct Pal {
    pub bg: Color32,
    /// Mint haze behind the header.
    pub halo: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    /// Inset surfaces: segmented control track, stat tiles, tips.
    pub well: Color32,
    pub border: Color32,
    pub hairline: Color32,
    /// Top-edge highlight on cards, like the icon's inner rim.
    pub rim: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    /// Mint for fills, dots and icons.
    pub accent: Color32,
    /// Mint for text; darker in light mode so it stays readable on white.
    pub accent_text: Color32,
    pub accent_soft: Color32,
    pub accent_line: Color32,
    /// Text on the mint gradient.
    pub on_accent: Color32,
    pub warn: Color32,
    pub warn_soft: Color32,
    pub bad: Color32,
    pub pro: Color32,
    pub pro_soft: Color32,
    pub track_off: Color32,
    pub shadow: Color32,
}

/// Premultiplied color from straight RGB and an opacity in 0..=255.
const fn a(r: u8, g: u8, b: u8, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, alpha)
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub const DARK: Pal = Pal {
    bg: rgb(0x0b, 0x0e, 0x13),
    halo: a(0x3d, 0xdc, 0x97, 34),
    card: rgb(0x13, 0x18, 0x20),
    card_hover: rgb(0x17, 0x1d, 0x26),
    well: rgb(0x0e, 0x12, 0x18),
    border: rgb(0x20, 0x28, 0x33),
    hairline: rgb(0x1b, 0x22, 0x2c),
    rim: a(0xff, 0xff, 0xff, 14),
    text: rgb(0xec, 0xf1, 0xf5),
    muted: rgb(0x8d, 0x98, 0xa5),
    faint: rgb(0x5d, 0x68, 0x76),
    accent: rgb(0x3d, 0xdc, 0x97),
    accent_text: rgb(0x52, 0xe6, 0xa6),
    accent_soft: a(0x3d, 0xdc, 0x97, 26),
    accent_line: a(0x3d, 0xdc, 0x97, 80),
    on_accent: rgb(0x05, 0x1a, 0x10),
    warn: rgb(0xf5, 0xc4, 0x51),
    warn_soft: a(0xf5, 0xc4, 0x51, 24),
    bad: rgb(0xff, 0x7d, 0x7d),
    pro: rgb(0xb7, 0xa2, 0xff),
    pro_soft: a(0xb7, 0xa2, 0xff, 26),
    track_off: rgb(0x2a, 0x33, 0x40),
    shadow: a(0x00, 0x00, 0x00, 120),
};

pub const LIGHT: Pal = Pal {
    bg: rgb(0xf2, 0xf5, 0xf4),
    halo: a(0x3d, 0xdc, 0x97, 46),
    card: rgb(0xff, 0xff, 0xff),
    card_hover: rgb(0xfa, 0xfc, 0xfb),
    well: rgb(0xeb, 0xf0, 0xee),
    border: rgb(0xdf, 0xe6, 0xe4),
    hairline: rgb(0xe7, 0xec, 0xea),
    rim: a(0xff, 0xff, 0xff, 0),
    text: rgb(0x10, 0x17, 0x20),
    muted: rgb(0x58, 0x64, 0x70),
    faint: rgb(0x93, 0x9e, 0xa8),
    accent: rgb(0x1f, 0xbf, 0x7a),
    accent_text: rgb(0x0a, 0x84, 0x52),
    accent_soft: a(0x1f, 0xbf, 0x7a, 28),
    accent_line: a(0x1f, 0xbf, 0x7a, 90),
    on_accent: rgb(0x05, 0x1a, 0x10),
    warn: rgb(0x9a, 0x62, 0x00),
    warn_soft: a(0xf5, 0xb0, 0x20, 36),
    bad: rgb(0xc8, 0x33, 0x33),
    pro: rgb(0x63, 0x45, 0xd6),
    pro_soft: a(0x7c, 0x5c, 0xf0, 24),
    track_off: rgb(0xd3, 0xdb, 0xd9),
    shadow: a(0x1c, 0x2a, 0x26, 22),
};

pub fn pal(ui: &Ui) -> &'static Pal {
    pal_ctx(ui.ctx())
}

fn pal_ctx(ctx: &egui::Context) -> &'static Pal {
    if ctx.theme() == egui::Theme::Dark {
        &DARK
    } else {
        &LIGHT
    }
}

/// The icon's mint gradient (#7cf7c2 to #1fbf7a), drawn as a 2x2 texture that the GPU blends
/// diagonally. Set once in [`App::new`].
static GRADIENT: OnceLock<TextureId> = OnceLock::new();

fn semi() -> FontFamily {
    FontFamily::Name("semibold".into())
}

fn heavy() -> FontFamily {
    FontFamily::Name("heavy".into())
}

fn theme_pref(name: &str) -> egui::ThemePreference {
    match name {
        "light" => egui::ThemePreference::Light,
        "dark" => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    }
}

// ------------------------------------------------------------------------ app

pub struct App {
    engine: Engine,
    db: GameDb,
    config: Arc<Mutex<Config>>,
    shared: Arc<Mutex<Shared>>,
    store: Store,
    per_game: Arc<Mutex<crate::pergame::PerGame>>,
    tx: Sender<Command>,
    icons: HashMap<String, TextureHandle>,
    expanded: HashSet<String>,
    toast: Option<(String, bool, Instant)>,
    seen_version: u64,
    /// Tweak states are read from the registry and power settings; cache them so redraws
    /// don't touch the system. Refreshed after changes and every few seconds.
    states: HashMap<String, TweakState>,
    states_at: Option<Instant>,
    tray: crate::tray::Tray,
    tray_rx: std::sync::mpsc::Receiver<crate::tray::TrayEvent>,
    tray_active: Option<bool>,
    quitting: bool,
    autostart_choice: bool,
    logo: TextureHandle,
    _gradient: TextureHandle,
    licenses: Arc<Licenses>,
    license: LicenseState,
    license_tx: Sender<LicenseEvent>,
    license_rx: std::sync::mpsc::Receiver<LicenseEvent>,
    /// `None` until the first check. Not `now - LICENSE_CHECK`: on Windows an `Instant` counts
    /// from boot, so that subtraction panics when the PC has been up less than six hours.
    license_checked: Option<Instant>,
    activation: Option<Activation>,
}

/// Results of license calls, which run on a worker thread so the window never waits.
enum LicenseEvent {
    Activated(Result<LicenseState, ApiError>),
    Refreshed(Result<LicenseState, ApiError>),
    DeviceRemoved(Result<(), ApiError>),
    Deactivated(Result<(), ApiError>),
}

/// The "Activate LeanFPS Pro" dialog.
#[derive(Default)]
struct Activation {
    key: String,
    email: String,
    busy: bool,
    error: Option<String>,
    /// Shown when every seat is taken, so one can be freed right here.
    devices: Vec<Device>,
}

/// How often a running LeanFPS asks whether its license file is due for a refresh.
const LICENSE_CHECK: Duration = Duration::from_secs(6 * 3600);

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        engine: Engine,
        db: GameDb,
        config: Arc<Mutex<Config>>,
        shared: Arc<Mutex<Shared>>,
        store: Store,
        per_game: Arc<Mutex<crate::pergame::PerGame>>,
        tx: Sender<Command>,
        licenses: Arc<Licenses>,
    ) -> App {
        style(&cc.egui_ctx);
        cc.egui_ctx.set_theme(theme_pref(&config.lock().unwrap().theme));
        let logo = cc.egui_ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied([64, 64], include_bytes!("../res/window-64.rgba")),
            egui::TextureOptions::LINEAR,
        );
        let (top, bottom) = ([0x7c, 0xf7, 0xc2, 0xff], [0x1f, 0xbf, 0x7a, 0xff]);
        let mid = [0x4e, 0xdb, 0x9e, 0xff];
        let px: Vec<u8> = [top, mid, mid, bottom].concat();
        let gradient = cc.egui_ctx.load_texture("mint", egui::ColorImage::from_rgba_unmultiplied([2, 2], &px), egui::TextureOptions::LINEAR);
        let _ = GRADIENT.set(gradient.id());
        let (ttx, tray_rx) = std::sync::mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        let tray = crate::tray::Tray::start(ttx, Box::new(move || ctx.request_repaint()));
        let (license_tx, license_rx) = std::sync::mpsc::channel();
        let license = licenses.state();
        App {
            engine,
            db,
            config,
            shared,
            store,
            per_game,
            tx,
            icons: HashMap::new(),
            expanded: HashSet::new(),
            toast: None,
            seen_version: 0,
            states: HashMap::new(),
            states_at: None,
            tray,
            tray_rx,
            tray_active: None,
            quitting: false,
            autostart_choice: true,
            logo,
            _gradient: gradient,
            licenses,
            license,
            license_tx,
            license_rx,
            // Due right away, so a file that asks for a refresh gets one at start.
            license_checked: None,
            activation: None,
        }
    }

    /// Run a license call off the UI thread; the answer arrives in `license_events`.
    fn license_job(&self, ctx: &egui::Context, job: impl FnOnce(&Licenses) -> LicenseEvent + Send + 'static) {
        let (licenses, tx, ctx) = (self.licenses.clone(), self.license_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job(&licenses));
            ctx.request_repaint();
        });
    }

    /// A new license state: Pro tweaks follow it (applied with their prior values recorded,
    /// or undone) and session tweaks pick it up when the watcher restarts.
    fn set_license(&mut self, state: LicenseState) {
        let pro = state.is_pro() || crate::license::dev_pro();
        self.license = state;
        let changed = {
            let mut cfg = self.config.lock().unwrap();
            let changed = cfg.pro != pro;
            cfg.pro = pro;
            changed
        };
        if changed {
            self.sync();
        }
    }

    fn license_events(&mut self, ctx: &egui::Context) {
        if self.license_checked.is_none_or(|t| t.elapsed() >= LICENSE_CHECK) && self.license.payload().is_some() {
            self.license_checked = Some(Instant::now());
            self.license_job(ctx, |l| LicenseEvent::Refreshed(l.refresh_if_due(false)));
        }
        while let Ok(ev) = self.license_rx.try_recv() {
            match ev {
                LicenseEvent::Activated(Ok(state)) => {
                    self.activation = None;
                    self.set_license(state);
                    self.notify("LeanFPS Pro is active on this PC. Thank you!", true);
                }
                LicenseEvent::Activated(Err(e)) => {
                    if let Some(a) = &mut self.activation {
                        a.busy = false;
                        a.devices = if e.code == "LICENSE_SEATS_FULL" { e.devices() } else { Vec::new() };
                        a.error = Some(e.message());
                    }
                }
                LicenseEvent::DeviceRemoved(r) => {
                    if let Some(a) = &mut self.activation {
                        a.busy = false;
                        match r {
                            Ok(()) => {
                                a.devices.clear();
                                a.error = Some("PC removed. Activate again to use this PC.".into());
                            }
                            Err(e) => a.error = Some(e.message()),
                        }
                    }
                }
                LicenseEvent::Refreshed(r) => {
                    let lost = r.as_ref().err().map(|e| e.message());
                    self.set_license(self.licenses.state());
                    if let Some(msg) = lost {
                        self.notify(format!("Pro was turned off: {msg}"), false);
                    }
                }
                LicenseEvent::Deactivated(r) => {
                    self.set_license(self.licenses.state());
                    match r {
                        Ok(()) => self.notify("Pro was removed from this PC. Its seat is free for another PC.", true),
                        Err(_) => self.notify("Pro was removed from this PC. The server could not be reached, so free the seat on leanfps.com/license.", false),
                    }
                }
            }
        }
    }

    fn activation_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut a) = self.activation.take() else { return };
        let mut close = false;
        let mut submit = false;
        let mut remove: Option<String> = None;
        let modal = egui::Modal::new(egui::Id::new("activate")).show(ctx, |ui| {
            let p = pal(ui);
            ui.set_width(300.0);
            ui.label(RichText::new("Activate LeanFPS Pro").font(FontId::new(16.0, heavy())).color(p.text));
            ui.add_space(4.0);
            ui.label(RichText::new("Enter the key from your purchase email and the email you bought with. Activation needs the internet once; after that Pro works offline.").small().color(p.muted));
            ui.add_space(10.0);
            ui.label(RichText::new("License key").small().color(p.muted));
            let key = ui.add_enabled(!a.busy, egui::TextEdit::singleline(&mut a.key).hint_text("LFPS-XXXXX-XXXXX-XXXXX-XXXXX").desired_width(f32::INFINITY));
            ui.add_space(6.0);
            ui.label(RichText::new("Purchase email").small().color(p.muted));
            let email = ui.add_enabled(!a.busy, egui::TextEdit::singleline(&mut a.email).hint_text("you@example.com").desired_width(f32::INFINITY));
            if a.key.is_empty() && !key.has_focus() && !email.has_focus() {
                key.request_focus();
            }
            let enter = (key.lost_focus() || email.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(err) = &a.error {
                ui.add_space(8.0);
                ui.label(RichText::new(err).small().color(Tone::Warn.color(p)));
            }
            for d in &a.devices {
                ui.horizontal(|ui| {
                    let seen = d.last_seen_at.as_deref().map(|t| t.get(..10).unwrap_or(t)).unwrap_or("");
                    ui.label(RichText::new(format!("{}  {}", d.device_name.as_deref().unwrap_or("PC"), seen)).small().color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !d.current && ui.add_enabled(!a.busy, egui::Button::new(RichText::new("Remove").small())).clicked() {
                            remove = Some(d.id.clone());
                        }
                    });
                });
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ghost_button(ui, "Cancel").clicked() {
                    close = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = if a.busy { "Activating…" } else { "Activate" };
                    if (primary_button(ui, label).clicked() || enter) && !a.busy {
                        submit = true;
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.link(RichText::new(format!("Buy a key ({PRICE})")).small()).clicked() {
                    open_url(crate::license::BUY_URL);
                }
                ui.label(RichText::new("·").small().color(p.faint));
                if ui.link(RichText::new("Lost your key?").small()).clicked() {
                    open_url(crate::license::MANAGE_URL);
                }
            });
        });
        if modal.should_close() && !a.busy {
            close = true;
        }
        if submit {
            if a.key.trim().len() < 20 || !a.email.contains('@') {
                a.error = Some("Enter the full key and the email you used to buy it.".into());
            } else {
                a.busy = true;
                a.error = None;
                let (key, email) = (a.key.clone(), a.email.clone());
                self.license_job(ctx, move |l| LicenseEvent::Activated(l.activate(&key, &email)));
            }
        }
        if let Some(id) = remove {
            a.busy = true;
            let (key, email) = (a.key.clone(), a.email.clone());
            self.license_job(ctx, move |l| LicenseEvent::DeviceRemoved(l.remove_device(&key, &email, &id)));
        }
        if !close {
            self.activation = Some(a);
        }
    }

    fn state(&mut self, id: &str) -> TweakState {
        if self.states_at.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            let ids: Vec<String> = self.engine.catalog.tweak.iter().map(|t| t.id.clone()).collect();
            self.states = ids.into_iter().map(|id| {
                let st = self.engine.state(&id);
                (id, st)
            }).collect();
            self.states_at = Some(Instant::now());
        }
        self.states.get(id).cloned().unwrap_or(TweakState::NotApplied)
    }

    fn invalidate_states(&mut self) {
        self.states_at = None;
    }

    fn catalog(&self) -> &Catalog {
        &self.engine.catalog
    }

    fn save_config(&mut self) {
        let cfg = self.config.lock().unwrap().clone();
        if let Err(e) = self.store.save("config.json", &cfg) {
            self.notify(format!("Could not save settings: {e}"), false);
        }
    }

    fn notify(&mut self, msg: impl Into<String>, ok: bool) {
        self.toast = Some((msg.into(), ok, Instant::now()));
    }

    /// Bring persistent tweaks in line with the settings. Returns how many changed.
    fn sync(&mut self) -> usize {
        let cfg = self.config.lock().unwrap().clone();
        let mut changed = 0;
        let mut errors = Vec::new();
        let tweaks: Vec<Tweak> = self.catalog().tweak.iter().filter(|t| t.scope == Scope::Persistent).cloned().collect();
        for t in &tweaks {
            let want = cfg.active(t);
            let state = self.engine.state(&t.id);
            let r = match (want, &state) {
                (true, TweakState::NotApplied | TweakState::ChangedOutside) => Some(self.engine.apply(&t.id)),
                (false, TweakState::Applied | TweakState::ChangedOutside) => Some(self.engine.revert(&t.id)),
                _ => None,
            };
            match r {
                Some(Ok(())) => changed += 1,
                Some(Err(e)) => errors.push(format!("{}: {e}", t.title)),
                None => {}
            }
        }
        self.invalidate_states();
        // Session tweaks take effect the next time the game is detected.
        let _ = self.tx.send(Command::Restart);
        if let Some(e) = errors.first() {
            self.notify(e.clone(), false);
        }
        changed
    }

    fn set_preset(&mut self, preset: &str) {
        self.config.lock().unwrap().set_preset(preset);
        self.save_config();
        let n = self.sync();
        self.notify(format!("Preset applied. {n} system settings changed."), true);
    }

    fn toggle(&mut self, t: &Tweak, on: bool) {
        self.config.lock().unwrap().set_wanted(t, on);
        self.save_config();
        if t.scope == Scope::Persistent {
            self.sync();
        } else {
            if !on {
                let errs = self.per_game.lock().unwrap().revert_tweak(&*self.engine.pf, &t.id);
                if let Some(e) = errs.first() {
                    self.notify(format!("Could not undo: {e}"), false);
                }
            }
            let _ = self.tx.send(Command::Restart);
        }
    }

    fn restore_all(&mut self) {
        self.config.lock().unwrap().set_preset("none");
        self.save_config();
        self.invalidate_states();
        let mut errs = self.engine.revert_all();
        errs.extend(self.per_game.lock().unwrap().revert_all(&*self.engine.pf).into_iter().map(|e| ("per-game".to_string(), e)));
        let _ = self.tx.send(Command::Restart);
        match errs.first() {
            None => self.notify("Every change was undone. Your PC is back to how it was.", true),
            Some((id, e)) => self.notify(format!("Could not restore {id}: {e}"), false),
        }
    }

    // ------------------------------------------------------------------ sections

    fn header(&mut self, ui: &mut Ui, tier: Tier) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(Vec2::splat(30.0)));
            ui.add_space(4.0);
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(RichText::new("Lean").font(FontId::new(19.0, heavy())).color(p.text));
                ui.label(RichText::new("FPS").font(FontId::new(19.0, heavy())).color(p.accent_text));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.settings_menu(ui);
                self.theme_button(ui);
                ui.add_space(2.0);
                match tier {
                    Tier::Pro => pill(ui, "PRO", p.pro, p.pro_soft),
                    Tier::Trial { left } => pill(ui, &format!("Pro trial · {left} game{} left", if left == 1 { "" } else { "s" }), p.pro, p.pro_soft),
                    Tier::Free => pill(ui, "Free", p.muted, p.well),
                }
            });
        });
    }

    /// Sun in dark mode, moon in light mode; switches to the other one.
    fn theme_button(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let dark = ui.visuals().dark_mode;
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
        let painter = ui.painter();
        let behind = if resp.hovered() { p.card } else { p.bg };
        painter.rect_filled(rect, 8, behind);
        let c = if resp.hovered() { p.text } else { p.muted };
        let o = rect.center();
        if dark {
            painter.circle_filled(o, 3.6, c);
            for i in 0..8 {
                let d = Vec2::angled(i as f32 * std::f32::consts::FRAC_PI_4);
                painter.line_segment([o + d * 6.0, o + d * 8.0], Stroke::new(1.6, c));
            }
        } else {
            painter.circle_filled(o, 6.5, c);
            painter.circle_filled(o + Vec2::new(3.4, -3.2), 5.6, behind);
        }
        let resp = resp.on_hover_text(if dark { "Light mode" } else { "Dark mode" }).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            self.set_theme(if dark { "light" } else { "dark" }, ui.ctx());
        }
    }

    fn set_theme(&mut self, theme: &str, ctx: &egui::Context) {
        self.config.lock().unwrap().theme = theme.to_string();
        self.save_config();
        ctx.set_theme(theme_pref(theme));
    }

    fn set_autostart(&mut self, on: bool) {
        #[cfg(windows)]
        if let Err(e) = crate::platform::win::set_autostart(on) {
            self.notify(format!("Could not change start with Windows: {e}"), false);
            return;
        }
        self.config.lock().unwrap().start_with_windows = on;
        self.save_config();
    }

    fn settings_menu(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 8, p.card);
        }
        let c = if resp.hovered() { p.text } else { p.muted };
        for dx in [-5.5, 0.0, 5.5] {
            ui.painter().circle_filled(rect.center() + Vec2::new(dx, 0.0), 1.8, c);
        }
        let resp = resp.on_hover_text("Settings").on_hover_cursor(egui::CursorIcon::PointingHand);
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(240.0);
            let cfg = self.config.lock().unwrap().clone();
            ui.label(overline("Window", p.faint));
            let mut tray = cfg.close_to_tray;
            if check(ui, &mut tray, "Keep running in the tray when closed").clicked() {
                self.config.lock().unwrap().close_to_tray = tray;
                self.save_config();
            }
            let mut auto = cfg.start_with_windows;
            if check(ui, &mut auto, "Start with Windows").clicked() {
                self.set_autostart(auto);
            }
            let mut top = cfg.always_on_top;
            if check(ui, &mut top, "Keep window on top").clicked() {
                self.config.lock().unwrap().always_on_top = top;
                self.save_config();
                let level = if top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            }
            ui.add_space(6.0);
            ui.label(overline("License", p.faint));
            match self.license.payload().filter(|_| self.license.is_pro()) {
                Some(lic) => {
                    ui.label(RichText::new(format!("{} · {}", lic.plan_name, lic.email_hint)).small().color(p.text));
                    if ui.link(RichText::new("Manage devices ↗").small()).clicked() {
                        open_url(crate::license::MANAGE_URL);
                    }
                    if ui.link(RichText::new("Remove Pro from this PC").small()).on_hover_text("Frees this PC's seat so you can use the key on another PC").clicked() {
                        self.license_job(ui.ctx(), |l| LicenseEvent::Deactivated(l.deactivate()));
                        ui.close();
                    }
                }
                None => {
                    if let LicenseState::Invalid { reason, .. } = &self.license {
                        let why = match reason {
                            crate::license::Invalid::Expired => "Pro needs to go online once to renew.",
                            crate::license::Invalid::VersionNotCovered => "Your key doesn't cover this version.",
                            _ => "The license on this PC isn't valid here.",
                        };
                        ui.label(RichText::new(why).small().color(p.muted));
                    }
                    if ui.link(RichText::new("Enter license key…").small()).clicked() {
                        self.activation = Some(Activation::default());
                        ui.close();
                    }
                }
            }
            ui.add_space(6.0);
            ui.label(overline("Appearance", p.faint));
            ui.add_space(2.0);
            let current = if matches!(cfg.theme.as_str(), "light" | "dark") { cfg.theme.as_str() } else { "system" };
            let themes: Vec<(String, String, String)> = [("system", "System", "Follow Windows"), ("light", "Light", "Light mode"), ("dark", "Dark", "Dark mode")]
                .iter()
                .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
                .collect();
            let picked = ui.allocate_ui(Vec2::new(240.0, 36.0), |ui| segmented(ui, &themes, current)).inner;
            if let Some(id) = picked {
                self.set_theme(&id, ui.ctx());
            }
        });
    }

    fn game_card(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let session = self.shared.lock().unwrap().session.clone();
        let installed = self.shared.lock().unwrap().installed.len();
        hero(ui, session.is_some(), |ui| {
            let Some(s) = session else {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(52.0), Sense::hover());
                    idle_tile(ui, r);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(overline("Waiting for a game", p.faint));
                        ui.label(RichText::new("No game running").font(FontId::new(16.0, semi())).color(p.text));
                        ui.label(RichText::new("Start a game. LeanFPS detects it and optimizes while you play.").small().color(p.muted));
                    });
                });
                let last = self.shared.lock().unwrap().last_game.clone();
                if let Some(last) = last {
                    let r = &last.record;
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(overline("Last game", p.faint));
                        ui.label(RichText::new(&r.name).small().color(p.text));
                    });
                    ui.add_space(2.0);
                    stats(ui, &[(format!("{:.0}", r.avg_fps), "FPS average"), (format!("{:.0}", r.low1_fps), "FPS 1% low"), (format!("{:.0}", r.minutes), "Minutes")]);
                    if let Some((avg, low, n)) = last.usual {
                        compare(ui, r.avg_fps, r.low1_fps, avg, low, n);
                    }
                }
                if installed > 0 {
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("Found {installed} installed games.")).small().color(p.muted));
                }
                return;
            };
            ui.horizontal(|ui| {
                let tex = s.icon.as_ref().map(|icon| {
                    self.icons
                        .entry(s.game.exe_path.clone())
                        .or_insert_with(|| {
                            ui.ctx().load_texture(
                                s.game.exe_path.clone(),
                                egui::ColorImage::from_rgba_unmultiplied([icon.size, icon.size], &icon.rgba),
                                egui::TextureOptions::LINEAR,
                            )
                        })
                        .clone()
                });
                let (r, _) = ui.allocate_exact_size(Vec2::splat(52.0), Sense::hover());
                ui.painter().add(RectShape::filled(r.expand(2.0), 14, p.accent.gamma_multiply(0.35)).with_blur_width(18.0));
                match tex {
                    Some(t) => {
                        ui.painter().rect_filled(r, 12, p.well);
                        egui::Image::new(&t).corner_radius(12).paint_at(ui, r);
                    }
                    None => gradient_fill(ui.painter(), r, 12),
                }
                ui.painter().rect_stroke(r, 12, Stroke::new(1.0, p.rim), StrokeKind::Inside);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        live_dot(ui);
                        ui.label(overline("Optimizing now", p.accent_text));
                    });
                    ui.label(RichText::new(&s.game.name).font(FontId::new(17.0, semi())).color(p.text));
                    ui.horizontal(|ui| {
                        let label = s.game.genre.map(|g| g.label()).unwrap_or("Pick a game type");
                        let chip = chip_button(ui, label);
                        egui::Popup::menu(&chip).show(|ui| {
                            ui.label(overline("Game type", p.faint));
                            for g in Genre::ALL {
                                if ui.selectable_label(s.game.genre == Some(g), g.label()).clicked() {
                                    let exe = crate::platform::file_name_lower(&s.game.exe_path);
                                    self.config.lock().unwrap().user_genres.insert(exe, g);
                                    self.save_config();
                                    let _ = self.tx.send(Command::Restart);
                                }
                            }
                        });
                        let mins = s.started.elapsed().as_secs() / 60;
                        ui.label(RichText::new(format!("{mins} min")).small().color(p.muted));
                    });
                });
            });
            if s.game.genre.is_none() {
                ui.add_space(4.0);
                ui.label(RichText::new("Pick its type for extra optimizations").small().color(p.warn));
            }
            if let Some(f) = s.fps.filter(|f| f.frames > 0) {
                ui.add_space(10.0);
                stats(ui, &[(format!("{:.0}", f.live), "FPS now"), (format!("{:.0}", f.avg), "Average"), (format!("{:.0}", f.low1), "1% low")]);
                if let Some((avg, low, n)) = s.usual {
                    if f.frames > 3000 {
                        compare(ui, f.avg, f.low1, avg, low, n);
                    }
                }
            }
            ui.add_space(10.0);
            hairline(ui);
            ui.add_space(8.0);
            let ok = s.applied.iter().filter(|a| a.result.is_ok()).count();
            let genre_items: Vec<_> = s.applied.iter().filter(|a| a.genre_specific).collect();
            let general: Vec<_> = s.applied.iter().filter(|a| !a.genre_specific).collect();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{ok}")).font(FontId::new(15.0, heavy())).color(p.accent_text));
                ui.label(RichText::new("optimizations active").font(FontId::new(13.0, semi())).color(p.text));
                if let (Some(g), false) = (s.game.genre, genre_items.is_empty()) {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let n = genre_items.iter().filter(|a| a.result.is_ok()).count();
                        ui.label(RichText::new(format!("{n} for {}", g.label().to_lowercase())).small().color(p.muted));
                    });
                }
            });
            ui.add_space(2.0);
            for a in &genre_items {
                applied_line(ui, a, true);
            }
            if !general.is_empty() {
                let id = ui.make_persistent_id("general-applied");
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                    .show_header(ui, |ui| {
                        ui.label(RichText::new(format!("{} general optimizations", general.len())).small().color(p.muted));
                    })
                    .body(|ui| {
                        for a in &general {
                            applied_line(ui, a, false);
                        }
                    });
            }
            if s.purges > 0 {
                ui.label(RichText::new(format!("Standby memory freed {} time{}", s.purges, if s.purges == 1 { "" } else { "s" })).small().color(p.muted));
            }
            if let Some(tip) = s.game.genre.and_then(|g| self.db.tips(g).first()) {
                ui.add_space(6.0);
                egui::Frame::NONE.fill(p.well).corner_radius(10).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(overline("Tip", p.accent_text));
                        ui.label(RichText::new(tip).small().color(p.muted));
                    });
                });
            }
        });
    }

    /// A windowed 3D app that might be a game: ask once, remember the answer.
    fn candidate(&mut self, ui: &mut Ui) {
        let Some(c) = self.shared.lock().unwrap().candidate.clone() else { return };
        let p = pal(ui);
        let exe = crate::platform::file_name_lower(&c.exe_path);
        let mut answer = None;
        card(ui, |ui| {
            ui.label(overline("New app detected", p.faint));
            ui.label(RichText::new(format!("Is {} a game?", c.name)).font(FontId::new(14.5, semi())).color(p.text));
            ui.label(RichText::new(format!("{exe} uses 3D graphics in a large window. Say yes and LeanFPS optimizes it while it runs.")).small().color(p.muted));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if primary_button(ui, "Yes, optimize it").clicked() {
                    answer = Some(true);
                }
                if ghost_button(ui, "Not a game").clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            {
                let mut cfg = self.config.lock().unwrap();
                if yes {
                    cfg.confirmed_games.insert(exe);
                } else {
                    cfg.not_games.insert(exe);
                }
            }
            self.save_config();
            self.shared.lock().unwrap().candidate = None;
            let _ = self.tx.send(Command::Restart);
        }
        ui.add_space(10.0);
    }

    fn welcome(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let n = {
            let cfg = self.config.lock().unwrap();
            self.catalog().tweak.iter().filter(|t| t.scope == Scope::Persistent && cfg.active(t)).count()
        };
        let mut go = false;
        egui::Frame::NONE
            .fill(p.accent_soft)
            .stroke(Stroke::new(1.0, p.accent_line))
            .corner_radius(14)
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_max_width(ui.available_width() - 104.0);
                        ui.label(overline("Get started", p.accent_text));
                        ui.label(RichText::new(format!("Apply {n} recommended system settings")).font(FontId::new(14.5, semi())).color(p.text));
                        ui.label(RichText::new("Every change can be undone. See them under All settings.").small().color(p.muted));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        go = primary_button(ui, "Optimize").clicked();
                    });
                });
                ui.add_space(6.0);
                check(ui, &mut self.autostart_choice, "Start with Windows and optimize games automatically");
            });
        if go {
            self.config.lock().unwrap().first_run_done = true;
            self.save_config();
            if self.autostart_choice {
                self.set_autostart(true);
            }
            let changed = self.sync();
            self.notify(format!("Done. {changed} settings changed. Start a game to see the rest."), true);
        }
    }

    fn preset_row(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let (preset, custom) = {
            let c = self.config.lock().unwrap();
            (c.preset.clone(), c.is_custom())
        };
        let presets: Vec<(String, String, String)> = self.catalog().preset.iter().map(|p| (p.id.clone(), p.title.clone(), p.summary.clone())).collect();
        ui.horizontal(|ui| {
            ui.label(overline("Preset", p.faint));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if preset == "none" {
                    pill(ui, "Off", p.muted, p.well);
                } else if custom {
                    pill(ui, "Customized", p.warn, p.warn_soft);
                }
            });
        });
        ui.add_space(2.0);
        let choose = segmented(ui, &presets, &preset);
        if let Some(summary) = presets.iter().find(|x| x.0 == preset).map(|x| x.2.clone()) {
            ui.add_space(2.0);
            ui.label(RichText::new(summary).small().color(p.muted));
        }
        if let Some(id) = choose {
            self.set_preset(&id);
        }
    }

    /// Simple view by default: the preset decides, the full list stays folded away. Returns
    /// whether the list is shown.
    fn all_settings_toggle(&mut self, ui: &mut Ui, cat: &Catalog, cfg: &Config, progress: Option<(usize, usize)>) -> bool {
        let p = pal(ui);
        let open = cfg.show_all_settings;
        let on = cat.tweak.iter().filter(|t| cfg.active(t)).count();
        ui.add_space(10.0);
        let resp = ui
            .horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(10.0, 14.0), Sense::hover());
                chevron(ui.painter(), r.center(), open, p.muted);
                ui.label(RichText::new("All settings").font(FontId::new(13.0, semi())).color(p.text));
                ui.label(RichText::new(format!("{on} of {} on", cat.tweak.len())).small().color(p.muted));
                if let (false, Some((done, total))) = (open, progress) {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let c = if done == total { p.accent_text } else { p.warn };
                        ui.label(RichText::new(format!("{done}/{total} system applied")).small().color(c));
                    });
                }
            })
            .response
            .interact(Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            self.config.lock().unwrap().show_all_settings = !open;
            self.save_config();
        }
        if !open {
            ui.label(RichText::new("The preset picks these for you. Open to turn single changes on or off.").small().color(p.muted));
        }
        open
    }

    fn group(&mut self, ui: &mut Ui, title: &str, hint: &str, tweaks: Vec<Tweak>, progress: Option<(usize, usize)>) {
        if tweaks.is_empty() {
            return;
        }
        let p = pal(ui);
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.label(overline(title, p.text));
            if let Some((done, total)) = progress {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(44.0, 4.0), Sense::hover());
                    ui.painter().rect_filled(r, 2, p.track_off);
                    let f = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                    if f > 0.0 {
                        gradient_fill(ui.painter(), Rect::from_min_size(r.min, Vec2::new(r.width() * f, r.height())), 2);
                    }
                    let c = if done == total { p.accent_text } else { p.warn };
                    ui.label(RichText::new(format!("{done}/{total} applied")).small().color(c));
                });
            }
        });
        ui.label(RichText::new(hint).small().color(p.muted));
        ui.add_space(6.0);
        for t in tweaks {
            self.tweak_row(ui, &t);
        }
    }

    fn tweak_row(&mut self, ui: &mut Ui, t: &Tweak) {
        let p = pal(ui);
        let cfg = self.config.lock().unwrap().clone();
        let tier = cfg.tier();
        let locked = t.pro && tier == Tier::Free;
        let state = self.state(&t.id);
        let unavailable = matches!(state, TweakState::Unavailable(_));
        let mut on = cfg.wants(t) && !locked;
        let open = self.expanded.contains(&t.id);
        let id = ui.make_persistent_id(("tweak", &t.id));
        let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());

        let frame = egui::Frame::NONE
            .fill(if hovered { p.card_hover } else { p.card })
            .stroke(Stroke::new(1.0, if hovered || open { p.accent_line } else { p.border }))
            .corner_radius(12)
            .inner_margin(Margin { left: 12, right: 10, top: 10, bottom: 10 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let full = ui.available_rect_before_wrap();
                let text_rect = ui
                    .vertical(|ui| {
                        ui.set_width(full.width() - 68.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.add(egui::Label::new(RichText::new(&t.title).font(FontId::new(13.5, semi())).color(if unavailable { p.muted } else { p.text })).wrap());
                            if t.pro && tier != Tier::Pro {
                                tag(ui, "PRO", p.pro, p.pro_soft);
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            let (status, tone) = state_text(&state, t, on);
                            status_dot(ui, tone.color(p));
                            ui.label(RichText::new(status).small().color(tone.color(p)));
                            for g in &t.genres {
                                ui.label(RichText::new("·").small().color(p.faint));
                                ui.label(RichText::new(g.label()).small().color(p.muted));
                            }
                        });
                    })
                    .response
                    .rect;
                // The switch sits at a fixed spot on the right, centered on the text beside it.
                let sw_rect = Rect::from_center_size(pos2(full.right() - 19.0, text_rect.center().y), Vec2::new(38.0, 22.0));
                chevron(ui.painter(), pos2(sw_rect.left() - 14.0, sw_rect.center().y), open, if hovered || open { p.muted } else { p.faint });
                let sw = ui.put(sw_rect, |ui: &mut Ui| ui.add_enabled(!locked && !unavailable, Toggle(&mut on)));
                if sw.changed() {
                    self.toggle(t, on);
                }
                if open {
                    ui.add_space(8.0);
                    hairline(ui);
                    ui.add_space(8.0);
                    ui.label(RichText::new(&t.summary).small().color(p.text));
                    ui.add_space(2.0);
                    ui.label(RichText::new(&t.detail).small().color(p.muted));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        let (c, soft) = match t.risk {
                            Risk::Low => (p.accent_text, p.accent_soft),
                            Risk::Medium => (p.warn, p.warn_soft),
                            Risk::High => (p.bad, p.warn_soft),
                        };
                        pill(ui, t.risk.label(), c, soft);
                        if t.reboot {
                            pill(ui, "Needs a restart", p.muted, p.well);
                        }
                        for s in &t.sources {
                            ui.hyperlink_to(RichText::new("Source ↗").small(), s);
                        }
                    });
                }
                text_rect
            });
        // The text side of the row opens the details; the switch keeps its own clicks.
        let resp = ui.interact(frame.inner, id, Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            if open {
                self.expanded.remove(&t.id);
            } else {
                self.expanded.insert(t.id.clone());
            }
        }
        ui.add_space(6.0);
    }

    fn footer(&mut self, ui: &mut Ui, tier: Tier) {
        hairline(ui);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ghost_button(ui, "Restore all").on_hover_text("Undo every change LeanFPS made").clicked() {
                self.restore_all();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if tier != Tier::Pro {
                    if primary_button(ui, &format!("Unlock Pro · {PRICE}")).on_hover_text("One-time payment on leanfps.com. Opens your browser.").clicked() {
                        open_url(crate::license::BUY_URL);
                    }
                    if ghost_button(ui, "Enter key").on_hover_text("Already bought Pro? Activate it on this PC.").clicked() {
                        self.activation = Some(Activation::default());
                    }
                }
            });
        });
    }

    fn toast(&self, ctx: &egui::Context) {
        let Some((msg, ok, at)) = &self.toast else { return };
        let age = at.elapsed().as_secs_f32();
        if age >= 6.0 {
            return;
        }
        let p = pal_ctx(ctx);
        // Fade in and out instead of popping.
        let alpha = (age / 0.2).min(1.0).min((6.0 - age) / 0.4).clamp(0.0, 1.0);
        egui::Area::new(egui::Id::new("toast"))
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -70.0 + (1.0 - alpha) * 8.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                egui::Frame::NONE
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, if *ok { p.accent_line } else { p.bad.gamma_multiply(0.5) }))
                    .corner_radius(14)
                    .shadow(Shadow { offset: [0, 8], blur: 24, spread: 0, color: p.shadow })
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_max_width(320.0);
                        ui.horizontal(|ui| {
                            status_dot(ui, if *ok { p.accent } else { p.bad });
                            ui.add(egui::Label::new(RichText::new(msg).small().color(p.text)).wrap());
                        });
                    });
            });
        ctx.request_repaint();
    }
}

impl eframe::App for App {
    /// Runs even while the window is hidden in the tray.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        use crate::tray::TrayEvent;
        self.license_events(ctx);
        while let Ok(ev) = self.tray_rx.try_recv() {
            match ev {
                TrayEvent::Open => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayEvent::RestoreAll => self.restore_all(),
                TrayEvent::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting && self.config.lock().unwrap().close_to_tray {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        let session = self.shared.lock().unwrap().session.as_ref().map(|s| s.game.name.clone());
        if self.tray_active != Some(session.is_some()) {
            self.tray_active = Some(session.is_some());
            let tip = match &session {
                Some(name) => format!("LeanFPS: optimizing {name}"),
                None => "LeanFPS: waiting for a game".to_string(),
            };
            self.tray.set_active(session.is_some(), &tip);
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let (version, has_session) = {
            let s = self.shared.lock().unwrap();
            (s.version, s.session.is_some())
        };
        if version != self.seen_version {
            self.seen_version = version;
            let notices: Vec<String> = std::mem::take(&mut self.shared.lock().unwrap().notices);
            if let Some(n) = notices.first() {
                self.notify(n.clone(), false);
            }
        }
        let tier = self.config.lock().unwrap().tier();
        let first_run = !self.config.lock().unwrap().first_run_done;
        let p = pal(ui);

        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0, p.bg);
        // Mint haze behind the logo, the window's one decorative touch.
        ui.painter().add(RectShape::filled(Rect::from_center_size(full.left_top() + Vec2::new(60.0, -10.0), Vec2::new(300.0, 120.0)), 60, p.halo).with_blur_width(150.0));

        egui::Frame::NONE.inner_margin(Margin { left: 16, right: 16, top: 14, bottom: 12 }).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            let width = ui.available_width();
            self.header(ui, tier);
            ui.add_space(12.0);
            let footer_h = 56.0;
            let cat = self.catalog().clone();
            let cfg = self.config.lock().unwrap().clone();
            let wanted: Vec<String> = cat.tweak.iter().filter(|t| t.scope == Scope::Persistent && cfg.active(t)).map(|t| t.id.clone()).collect();
            let done = wanted
                .iter()
                .filter(|id| matches!(self.state(id), TweakState::Applied | TweakState::AlreadySet | TweakState::Unavailable(_)))
                .count();
            let progress = (!wanted.is_empty()).then_some((done, wanted.len()));
            egui::ScrollArea::vertical().max_height(ui.available_height() - footer_h).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(width);
                // Room for the card shadows at the top.
                ui.add_space(2.0);
                self.game_card(ui);
                ui.add_space(12.0);
                self.candidate(ui);
                if first_run {
                    self.welcome(ui);
                    ui.add_space(14.0);
                }
                self.preset_row(ui);
                if self.all_settings_toggle(ui, &cat, &cfg, progress) {
                    let by = |f: &dyn Fn(&Tweak) -> bool| cat.tweak.iter().filter(|t| f(t)).cloned().collect::<Vec<_>>();
                    self.group(ui, "System", "Changed once, kept until you undo them.", by(&|t| t.scope == Scope::Persistent), progress);
                    self.group(ui, "While gaming", "On when a game starts, undone when it closes.", by(&|t| t.scope == Scope::Session && !t.is_genre_specific()), None);
                    self.group(ui, "Game profiles", "Extra changes for the type of game you are playing.", by(&|t| t.is_genre_specific()), None);
                }
                ui.add_space(8.0);
            });
            ui.scope(|ui| {
                ui.set_max_width(width);
                self.footer(ui, tier);
            });
        });
        self.activation_dialog(ui.ctx());
        self.toast(ui.ctx());

        if has_session {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.tray.remove();
        let _ = self.tx.send(Command::Stop);
    }
}

// ---------------------------------------------------------------------- widgets

fn style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        ("inter", &include_bytes!("../res/fonts/Inter-Regular.ttf")[..]),
        ("inter-semibold", &include_bytes!("../res/fonts/Inter-SemiBold.ttf")[..]),
        ("inter-heavy", &include_bytes!("../res/fonts/Inter-ExtraBold.ttf")[..]),
    ] {
        fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_static(bytes)));
    }
    // Inter covers Latin; the built-in fonts stay behind it for other scripts and symbols.
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    for (family, first) in [(FontFamily::Proportional, "inter"), (semi(), "inter-semibold"), (heavy(), "inter-heavy")] {
        let mut list = vec![first.to_string()];
        list.extend(fallback.iter().filter(|f| !f.starts_with("inter")).cloned());
        fonts.families.insert(family, list);
    }
    ctx.set_fonts(fonts);
    ctx.set_visuals_of(egui::Theme::Dark, visuals(&DARK, true));
    ctx.set_visuals_of(egui::Theme::Light, visuals(&LIGHT, false));
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(6.0, 4.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.menu_margin = Margin::same(10);
        s.spacing.interact_size.y = 22.0;
        use egui::TextStyle;
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
        s.text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
    });
}

fn visuals(p: &Pal, dark: bool) -> egui::Visuals {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let shadow = Shadow { offset: [0, 10], blur: 28, spread: 0, color: p.shadow };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(12);
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.extreme_bg_color = p.well;
    v.faint_bg_color = p.well;
    v.hyperlink_color = p.accent_text;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent_text);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.hairline);
    v.widgets.noninteractive.bg_fill = p.card;
    for (w, fill, stroke, fg) in [
        (&mut v.widgets.inactive, p.well, p.border, p.text),
        (&mut v.widgets.hovered, p.card_hover, p.accent_line, p.text),
        (&mut v.widgets.active, p.accent_soft, p.accent_text, p.accent_text),
        (&mut v.widgets.open, p.card_hover, p.accent_line, p.text),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, stroke);
        w.fg_stroke = Stroke::new(1.5, fg);
        w.corner_radius = CornerRadius::same(8);
        w.expansion = 0.0;
    }
    v.widgets.inactive.fg_stroke = Stroke::new(1.5, p.muted);
    v
}

/// Overline label: small, heavy, tracked-out capitals.
fn overline(text: &str, c: Color32) -> RichText {
    RichText::new(text.to_uppercase()).font(FontId::new(9.5, heavy())).extra_letter_spacing(1.0).color(c)
}

fn gradient_fill(painter: &egui::Painter, rect: Rect, radius: u8) {
    let uv = Rect::from_min_max(pos2(0.25, 0.25), pos2(0.75, 0.75));
    match GRADIENT.get() {
        Some(id) => painter.add(RectShape::filled(rect, radius, Color32::WHITE).with_texture(*id, uv)),
        None => painter.add(RectShape::filled(rect, radius, DARK.accent)),
    };
}

fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) {
    let p = pal(ui);
    egui::Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(14)
        .inner_margin(Margin::same(14))
        .shadow(Shadow { offset: [0, 2], blur: 8, spread: 0, color: p.shadow.gamma_multiply(0.5) })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
}

/// The game card: a raised card with a mint glow behind the game icon while a game runs.
fn hero<R>(ui: &mut Ui, live: bool, add: impl FnOnce(&mut Ui) -> R) {
    let p = pal(ui);
    let resp = egui::Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0, if live { p.accent_line } else { p.border }))
        .corner_radius(16)
        .inner_margin(Margin::same(14))
        .shadow(Shadow { offset: [0, 8], blur: 24, spread: 0, color: p.shadow })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .response;
    let r = resp.rect;
    // Inner rim highlight along the top edge, like the icon's.
    ui.painter().line_segment([pos2(r.left() + 16.0, r.top() + 1.0), pos2(r.right() - 16.0, r.top() + 1.0)], Stroke::new(1.0, p.rim));
    if live {
        // The icon's speed lines in the corner: this is the "running" state.
        let mut mesh = egui::Mesh::default();
        for (dy, inset, len, o) in [(0.0, 10.0, 54.0, 0.55), (9.0, 0.0, 78.0, 0.8), (18.0, 18.0, 36.0, 0.4)] {
            let right = r.right() - 16.0 - inset;
            let line = Rect::from_min_max(pos2(right - len, r.top() + 16.0 + dy), pos2(right, r.top() + 19.0 + dy));
            let c = p.accent.gamma_multiply(o * 0.7);
            let i = mesh.vertices.len() as u32;
            mesh.colored_vertex(line.left_top(), Color32::TRANSPARENT);
            mesh.colored_vertex(line.right_top(), c);
            mesh.colored_vertex(line.right_bottom(), c);
            mesh.colored_vertex(line.left_bottom(), Color32::TRANSPARENT);
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i, i + 2, i + 3);
            ui.painter().circle_filled(pos2(right, line.center().y), 1.5, c);
        }
        ui.painter().add(mesh);
    }
}

fn idle_tile(ui: &Ui, r: Rect) {
    let p = pal(ui);
    ui.painter().rect_filled(r, 12, p.well);
    ui.painter().rect_stroke(r, 12, Stroke::new(1.0, p.border), StrokeKind::Inside);
    // The tray's idle mark: the leaning L and its speed lines, at rest in grey.
    let (u, o) = (r.width() / 32.0, r.min.to_vec2());
    let at = |x: f32, y: f32| pos2(x, y) * u + o;
    let c = p.faint;
    for (y, x0, x1, alpha) in [(10.75, 6.25, 11.5, 0.55), (14.25, 4.75, 10.75, 0.8), (17.75, 7.25, 10.0, 0.4)] {
        ui.painter().line_segment([at(x0, y), at(x1, y)], Stroke::new(1.6 * u, c.gamma_multiply(alpha)));
    }
    // The L is not convex, so it is drawn as its stem and its foot.
    ui.painter().add(egui::Shape::convex_polygon(vec![at(14.6, 5.0), at(20.4, 5.0), at(17.2, 18.6), at(11.34, 18.6)], c, Stroke::NONE));
    ui.painter().add(egui::Shape::convex_polygon(vec![at(11.34, 18.6), at(26.6, 18.6), at(25.2, 25.0), at(9.8, 25.0)], c, Stroke::NONE));
}

fn live_dot(ui: &mut Ui) {
    let p = pal(ui);
    let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 5.0, p.accent_soft);
    ui.painter().circle_filled(r.center(), 2.8, p.accent);
}

fn status_dot(ui: &mut Ui, c: Color32) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 3.0, c);
}

fn chevron(painter: &egui::Painter, o: Pos2, open: bool, c: Color32) {
    let pts = if open {
        vec![o + Vec2::new(-3.5, -1.5), o + Vec2::new(0.0, 2.0), o + Vec2::new(3.5, -1.5)]
    } else {
        vec![o + Vec2::new(-1.5, -3.5), o + Vec2::new(2.0, 0.0), o + Vec2::new(-1.5, 3.5)]
    };
    painter.line(pts, Stroke::new(1.5, c));
}

fn hairline(ui: &mut Ui) {
    let p = pal(ui);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0, p.hairline);
}

/// Equal-width number tiles: big value, small caption.
fn stats(ui: &mut Ui, items: &[(String, &str)]) {
    let p = pal(ui);
    let gap = 6.0;
    let w = (ui.available_width() - gap * (items.len() as f32 - 1.0)) / items.len() as f32;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (i, (value, label)) in items.iter().enumerate() {
            let (r, _) = ui.allocate_exact_size(Vec2::new(w, 54.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(r, 10, p.well);
            let c = if i == 0 { p.accent_text } else { p.text };
            painter.text(r.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, value, FontId::new(22.0, heavy()), c);
            painter.text(r.left_bottom() + Vec2::new(10.0, -8.0), Align2::LEFT_BOTTOM, label.to_uppercase(), FontId::new(9.0, semi()), p.muted);
        }
    });
}

/// "+4% average  +7% 1% low  vs your last 3 sessions of this game"
fn compare(ui: &mut Ui, avg: f64, low: f64, usual_avg: f64, usual_low: f64, n: usize) {
    let p = pal(ui);
    let pct = |a: f64, b: f64| if b > 0.0 { (a / b - 1.0) * 100.0 } else { 0.0 };
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for (v, what) in [(pct(avg, usual_avg), "average"), (pct(low, usual_low), "1% low")] {
            let (c, soft) = if v >= 0.0 { (p.accent_text, p.accent_soft) } else { (p.warn, p.warn_soft) };
            pill(ui, &format!("{v:+.0}% {what}"), c, soft);
        }
        ui.label(RichText::new(format!("vs your last {n} session{} of this game", if n == 1 { "" } else { "s" })).small().color(p.muted));
    });
}

fn pill(ui: &mut Ui, text: &str, c: Color32, fill: Color32) {
    egui::Frame::NONE.fill(fill).corner_radius(99).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
        ui.label(RichText::new(text).font(FontId::new(11.0, semi())).color(c));
    });
}

/// Fixed-size label like "PRO"; wraps as a whole next to a long title.
fn tag(ui: &mut Ui, text: &str, c: Color32, fill: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(8.5, heavy()), c);
    let (r, _) = ui.allocate_exact_size(Vec2::new(galley.size().x + 9.0, 16.0), Sense::hover());
    ui.painter().rect_filled(r, 4, fill);
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, c);
}

/// Genre chip with a caret; opens a menu.
fn chip_button(ui: &mut Ui, text: &str) -> egui::Response {
    let p = pal(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(11.5, semi()), p.accent_text);
    let size = Vec2::new(galley.size().x + 30.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 99, p.accent_soft);
    if resp.hovered() {
        painter.rect_stroke(rect, 99, Stroke::new(1.0, p.accent_line), StrokeKind::Inside);
    }
    painter.galley(pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, p.accent_text);
    let o = pos2(rect.right() - 12.0, rect.center().y);
    painter.line(vec![o + Vec2::new(-3.0, -1.5), o + Vec2::new(0.0, 1.5), o + Vec2::new(3.0, -1.5)], Stroke::new(1.4, p.accent_text));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Mint gradient button with a soft glow, the one call to action per view.
fn primary_button(ui: &mut Ui, text: &str) -> egui::Response {
    let p = pal(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, semi()), p.on_accent);
    let size = Vec2::new(galley.size().x + 30.0, 34.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
        let glow = Rect::from_min_max(rect.left_top() + Vec2::new(6.0, 8.0), rect.right_bottom() + Vec2::new(-6.0, 4.0));
        painter.add(RectShape::filled(glow, 12, p.accent.gamma_multiply(0.30 + 0.25 * hover)).with_blur_width(16.0));
        gradient_fill(painter, rect, 10);
        painter.rect_filled(rect, 10, Color32::from_white_alpha((28.0 * hover) as u8));
        if resp.is_pointer_button_down_on() {
            painter.rect_filled(rect, 10, Color32::from_black_alpha(26));
        }
        painter.line_segment([pos2(rect.left() + 8.0, rect.top() + 0.5), pos2(rect.right() - 8.0, rect.top() + 0.5)], Stroke::new(1.0, Color32::from_white_alpha(60)));
        painter.galley(rect.center() - galley.size() / 2.0, galley, p.on_accent);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn ghost_button(ui: &mut Ui, text: &str) -> egui::Response {
    let p = pal(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, semi()), p.text);
    let size = Vec2::new(galley.size().x + 28.0, 34.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 10, if resp.hovered() { p.card_hover } else { p.card });
        painter.rect_stroke(rect, 10, Stroke::new(1.0, if resp.hovered() { p.accent_line } else { p.border }), StrokeKind::Inside);
        painter.galley(rect.center() - galley.size() / 2.0, galley, p.text);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rounded checkbox with the mint gradient when checked.
fn check(ui: &mut Ui, on: &mut bool, text: &str) -> egui::Response {
    let p = pal(ui);
    let resp = ui
        .horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            if *on {
                gradient_fill(ui.painter(), r, 5);
                let o = r.center();
                ui.painter().line(vec![o + Vec2::new(-3.5, 0.2), o + Vec2::new(-1.0, 2.7), o + Vec2::new(3.8, -2.5)], Stroke::new(1.8, p.on_accent));
            } else {
                ui.painter().rect_filled(r, 5, p.card);
                ui.painter().rect_stroke(r, 5, Stroke::new(1.2, p.faint), StrokeKind::Inside);
            }
            ui.label(RichText::new(text).small().color(p.text));
        })
        .response;
    let resp = ui.interact(resp.rect, resp.id.with("check"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        *on = !*on;
    }
    resp
}

/// Segmented control for presets. Returns the id the user picked.
fn segmented(ui: &mut Ui, items: &[(String, String, String)], selected: &str) -> Option<String> {
    let p = pal(ui);
    let (track, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
    ui.painter().rect_filled(track, 11, p.well);
    ui.painter().rect_stroke(track, 11, Stroke::new(1.0, p.hairline), StrokeKind::Inside);
    let inner = track.shrink(3.0);
    let w = inner.width() / items.len().max(1) as f32;
    let mut picked = None;
    for (i, (id, title, summary)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(inner.left() + w * i as f32, inner.top()), Vec2::new(w, inner.height()));
        let resp = ui.interact(r, ui.id().with(("preset", id)), Sense::click()).on_hover_text(summary).on_hover_cursor(egui::CursorIcon::PointingHand);
        let on = id == selected;
        let painter = ui.painter();
        if on {
            painter.add(RectShape::filled(r.translate(Vec2::new(0.0, 1.0)), 8, p.shadow).with_blur_width(6.0));
            painter.rect_filled(r, 8, p.card);
            painter.rect_stroke(r, 8, Stroke::new(1.0, p.accent_line), StrokeKind::Inside);
        } else if resp.hovered() {
            painter.rect_filled(r, 8, p.card.gamma_multiply(0.5));
        }
        let c = if on || resp.hovered() { p.text } else { p.muted };
        let galley = painter.layout_no_wrap(title.clone(), FontId::new(12.0, semi()), c);
        let dot = if on { 9.0 } else { 0.0 };
        let left = r.center().x - (galley.size().x + dot) / 2.0;
        if on {
            painter.circle_filled(pos2(left + 3.0, r.center().y), 3.0, p.accent);
        }
        painter.galley(pos2(left + dot, r.center().y - galley.size().y / 2.0), galley, c);
        if resp.clicked() && !on {
            picked = Some(id.clone());
        }
    }
    picked
}

#[derive(Clone, Copy)]
enum Tone {
    Good,
    Warn,
    Quiet,
}

impl Tone {
    fn color(self, p: &Pal) -> Color32 {
        match self {
            Tone::Good => p.accent_text,
            Tone::Warn => p.warn,
            Tone::Quiet => p.muted,
        }
    }
}

fn applied_line(ui: &mut Ui, a: &crate::session::Applied, strong: bool) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        check_mark(ui, if a.result.is_ok() { p.accent_text } else { p.warn });
        let font = if strong { FontId::new(12.0, semi()) } else { FontId::proportional(11.5) };
        let r = ui.label(RichText::new(&a.title).font(font).color(p.text));
        if let Err(e) = &a.result {
            r.on_hover_text(format!("Skipped: {e}"));
        } else if let Some(n) = &a.note {
            r.on_hover_text(n);
        }
    });
    if let Some(n) = &a.note {
        ui.horizontal(|ui| {
            ui.add_space(22.0);
            ui.label(RichText::new(n).small().color(p.muted));
        });
    }
}

fn check_mark(ui: &mut Ui, c: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
    let p = ui.painter();
    let o = rect.center();
    p.circle_filled(o, 7.5, c.gamma_multiply(0.14));
    p.line(vec![o + Vec2::new(-3.2, 0.2), o + Vec2::new(-0.8, 2.5), o + Vec2::new(3.4, -2.2)], Stroke::new(1.6, c));
}

fn state_text(state: &TweakState, t: &Tweak, on: bool) -> (String, Tone) {
    match state {
        TweakState::Applied => ("Applied".into(), Tone::Good),
        TweakState::AlreadySet => ("Already set on this PC".into(), Tone::Good),
        TweakState::NotApplied if on => ("Not applied yet".into(), Tone::Warn),
        TweakState::NotApplied => ("Off".into(), Tone::Quiet),
        TweakState::ChangedOutside => ("Changed by another app".into(), Tone::Warn),
        TweakState::Session if on && t.is_genre_specific() => ("On for matching games".into(), Tone::Good),
        TweakState::Session if on => ("On while gaming".into(), Tone::Good),
        TweakState::Session => ("Off".into(), Tone::Quiet),
        TweakState::Unavailable(r) => (r.clone(), Tone::Quiet),
    }
}

/// Switch with the mint gradient when on.
struct Toggle<'a>(&'a mut bool);

impl egui::Widget for Toggle<'_> {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        let p = pal(ui);
        let size = Vec2::new(38.0, 22.0);
        let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
        if resp.clicked() {
            *self.0 = !*self.0;
            resp.mark_changed();
        }
        let t = ui.ctx().animate_bool_with_time(resp.id, *self.0, 0.18);
        let enabled = ui.is_enabled();
        let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(11), fade(p.track_off));
        painter.rect_stroke(rect, CornerRadius::same(11), Stroke::new(1.0, fade(p.border)), StrokeKind::Inside);
        if t > 0.0 {
            if enabled {
                painter.add(RectShape::filled(rect.translate(Vec2::new(0.0, 2.0)), 11, p.accent.gamma_multiply(0.35 * t)).with_blur_width(10.0));
            }
            let lit = Rect::from_min_max(rect.min, pos2(egui::lerp(rect.left() + 22.0..=rect.right(), t), rect.bottom()));
            let tint = ui.painter().with_clip_rect(lit.intersect(ui.clip_rect()));
            gradient_fill(&tint, rect, 11);
            if !enabled {
                painter.rect_filled(rect, CornerRadius::same(11), p.card.gamma_multiply(0.55));
            }
        }
        let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
        let knob = Pos2::new(x, rect.center().y);
        painter.add(RectShape::filled(Rect::from_center_size(knob + Vec2::new(0.0, 1.0), Vec2::splat(16.0)), 8, Color32::from_black_alpha(60)).with_blur_width(4.0));
        painter.circle_filled(knob, 8.0, if enabled { Color32::WHITE } else { fade(p.muted) });
        if resp.hovered() && enabled {
            painter.rect_stroke(rect, CornerRadius::same(11), Stroke::new(1.0, p.accent_line), StrokeKind::Outside);
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

fn open_url(url: &str) {
    #[cfg(windows)]
    crate::platform::win::open_url(url);
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}
