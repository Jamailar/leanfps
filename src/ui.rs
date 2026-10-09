//! The small window.

use crate::catalog::{Catalog, Genre, Risk, Scope, Tweak};
use crate::config::{Config, Tier, PRICE};
use crate::engine::{Engine, TweakState};
use crate::games::GameDb;
use crate::store::Store;
use crate::watcher::{Command, Shared};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Margin, RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2,
};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const WINDOW_SIZE: [f32; 2] = [380.0, 640.0];

mod color {
    use eframe::egui::Color32;
    pub const BG: Color32 = Color32::from_rgb(0x0e, 0x11, 0x16);
    pub const CARD: Color32 = Color32::from_rgb(0x16, 0x1b, 0x22);
    pub const CARD_HOVER: Color32 = Color32::from_rgb(0x1c, 0x22, 0x2b);
    pub const BORDER: Color32 = Color32::from_rgb(0x26, 0x2d, 0x37);
    pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xed, 0xf3);
    pub const MUTED: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);
    pub const ACCENT: Color32 = Color32::from_rgb(0x3d, 0xdc, 0x97);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x1f, 0x4d, 0x3a);
    pub const WARN: Color32 = Color32::from_rgb(0xf2, 0xc1, 0x4e);
    pub const BAD: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);
    pub const PRO: Color32 = Color32::from_rgb(0xa7, 0x8b, 0xfa);
}

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
}

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
    ) -> App {
        style(&cc.egui_ctx);
        let (ttx, tray_rx) = std::sync::mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        let tray = crate::tray::Tray::start(ttx, Box::new(move || ctx.request_repaint()));
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
        ui.horizontal(|ui| {
            ui.label(RichText::new("LeanFPS").font(FontId::proportional(18.0)).strong().color(color::TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (text, c) = match tier {
                    Tier::Pro => ("PRO".to_string(), color::PRO),
                    Tier::Trial { left } => (format!("Pro trial · {left} game{} left", if left == 1 { "" } else { "s" }), color::PRO),
                    Tier::Free => ("Free".to_string(), color::MUTED),
                };
                self.settings_menu(ui);
                pill(ui, &text, c);
                let cfg = self.config.lock().unwrap().clone();
                let wanted: Vec<String> =
                    self.catalog().tweak.iter().filter(|t| t.scope == Scope::Persistent && cfg.active(t)).map(|t| t.id.clone()).collect();
                let done = wanted
                    .iter()
                    .filter(|id| matches!(self.state(id), TweakState::Applied | TweakState::AlreadySet | TweakState::Unavailable(_)))
                    .count();
                if !wanted.is_empty() {
                    let c = if done == wanted.len() { color::ACCENT } else { color::WARN };
                    pill(ui, &format!("System {done}/{}", wanted.len()), c);
                }
            });
        });
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
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(26.0, 22.0), Sense::click());
        let c = if resp.hovered() { color::TEXT } else { color::MUTED };
        for dx in [-6.0, 0.0, 6.0] {
            ui.painter().circle_filled(rect.center() + Vec2::new(dx, 0.0), 2.0, c);
        }
        let resp = resp.on_hover_text("Settings");
        egui::Popup::menu(&resp).show(|ui| {
            let cfg = self.config.lock().unwrap().clone();
            let mut tray = cfg.close_to_tray;
            if ui.checkbox(&mut tray, "Keep running in the tray when closed").changed() {
                self.config.lock().unwrap().close_to_tray = tray;
                self.save_config();
            }
            let mut auto = cfg.start_with_windows;
            if ui.checkbox(&mut auto, "Start with Windows").changed() {
                self.set_autostart(auto);
            }
            let mut top = cfg.always_on_top;
            if ui.checkbox(&mut top, "Keep window on top").changed() {
                self.config.lock().unwrap().always_on_top = top;
                self.save_config();
                let level = if top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            }
        });
    }

    fn game_card(&mut self, ui: &mut Ui) {
        let session = self.shared.lock().unwrap().session.clone();
        let installed = self.shared.lock().unwrap().installed.len();
        card(ui, |ui| {
            let Some(s) = session else {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                    ui.painter().rect_filled(r, 10, color::BORDER);
                    ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(2.0, color::MUTED));
                    ui.vertical(|ui| {
                        ui.label(RichText::new("No game running").strong().color(color::TEXT));
                        ui.label(RichText::new("Start a game. LeanFPS detects it and optimizes while you play.").small().color(color::MUTED));
                    });
                });
                if installed > 0 {
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("Found {installed} installed games.")).small().color(color::MUTED));
                }
                if let Some(last) = self.shared.lock().unwrap().last_game.clone() {
                    let r = &last.record;
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("Last game: {} · {:.0} FPS average · {:.0} FPS 1% low · {:.0} min", r.name, r.avg_fps, r.low1_fps, r.minutes))
                            .small()
                            .color(color::TEXT),
                    );
                    if let Some((avg, low, n)) = last.usual {
                        ui.label(RichText::new(compare_line(r.avg_fps, r.low1_fps, avg, low, n)).small().color(color::ACCENT));
                    }
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
                match tex {
                    Some(t) => {
                        ui.add(egui::Image::new(&t).fit_to_exact_size(Vec2::splat(44.0)).corner_radius(8));
                    }
                    None => {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                        ui.painter().rect_filled(r, 10, color::ACCENT_DIM);
                    }
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new(&s.game.name).font(FontId::proportional(16.0)).strong().color(color::TEXT));
                    ui.horizontal(|ui| {
                        let label = s.game.genre.map(|g| g.label()).unwrap_or("Pick a game type");
                        let chip = ui.add(egui::Button::new(RichText::new(label).small().color(color::ACCENT)).fill(color::ACCENT_DIM).corner_radius(10));
                        egui::Popup::menu(&chip).show(|ui| {
                            ui.label(RichText::new("Game type").small().color(color::MUTED));
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
                        ui.label(RichText::new(format!("{mins} min")).small().color(color::MUTED));
                        if s.game.genre.is_none() {
                            ui.label(RichText::new("Pick its type for extra optimizations").small().color(color::WARN));
                        }
                    });
                });
            });
            ui.add_space(6.0);
            if let Some(f) = s.fps.filter(|f| f.frames > 0) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{:.0}", f.live)).font(FontId::proportional(22.0)).strong().color(color::TEXT));
                    ui.label(RichText::new("FPS now").small().color(color::MUTED));
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("{:.0} avg · {:.0} 1% low", f.avg, f.low1)).small().color(color::TEXT));
                });
                if let Some((avg, low, n)) = s.usual {
                    if f.frames > 3000 {
                        ui.label(RichText::new(compare_line(f.avg, f.low1, avg, low, n)).small().color(color::ACCENT));
                    }
                }
            }
            let ok = s.applied.iter().filter(|a| a.result.is_ok()).count();
            let genre_items: Vec<_> = s.applied.iter().filter(|a| a.genre_specific).collect();
            let general: Vec<_> = s.applied.iter().filter(|a| !a.genre_specific).collect();
            let mut line = format!("{ok} optimizations active");
            if let (Some(g), false) = (s.game.genre, genre_items.is_empty()) {
                line.push_str(&format!(" · {} for {}", genre_items.iter().filter(|a| a.result.is_ok()).count(), g.label().to_lowercase()));
            }
            ui.label(RichText::new(line).small().strong().color(color::ACCENT));
            for a in &genre_items {
                applied_line(ui, a, true);
            }
            if !general.is_empty() {
                let id = ui.make_persistent_id("general-applied");
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                    .show_header(ui, |ui| {
                        ui.label(RichText::new(format!("{} general optimizations", general.len())).small().color(color::MUTED));
                    })
                    .body(|ui| {
                        for a in &general {
                            applied_line(ui, a, false);
                        }
                    });
            }
            if s.purges > 0 {
                ui.label(RichText::new(format!("Standby memory freed {} time{}", s.purges, if s.purges == 1 { "" } else { "s" })).small().color(color::MUTED));
            }
            if let Some(tip) = s.game.genre.and_then(|g| self.db.tips(g).first()) {
                ui.label(RichText::new(format!("Tip: {tip}")).small().color(color::MUTED));
            }
        });
    }

    /// A windowed 3D app that might be a game: ask once, remember the answer.
    fn candidate(&mut self, ui: &mut Ui) {
        let Some(c) = self.shared.lock().unwrap().candidate.clone() else { return };
        let exe = crate::platform::file_name_lower(&c.exe_path);
        let mut answer = None;
        card(ui, |ui| {
            ui.label(RichText::new(format!("Is {} a game?", c.name)).strong().color(color::TEXT));
            ui.label(RichText::new(format!("{exe} uses 3D graphics in a large window. Say yes and LeanFPS optimizes it while it runs.")).small().color(color::MUTED));
            ui.horizontal(|ui| {
                if primary_button(ui, "Yes, optimize it").clicked() {
                    answer = Some(true);
                }
                if ui.button("Not a game").clicked() {
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
        ui.add_space(8.0);
    }

    fn welcome(&mut self, ui: &mut Ui) {
        let n = {
            let cfg = self.config.lock().unwrap();
            self.catalog().tweak.iter().filter(|t| t.scope == Scope::Persistent && cfg.active(t)).count()
        };
        let mut go = false;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width(ui.available_width() - 96.0);
                    ui.label(RichText::new(format!("Apply {n} recommended system settings")).strong().color(color::TEXT));
                    ui.label(RichText::new("Every change is listed below and can be undone.").small().color(color::MUTED));
                    ui.checkbox(&mut self.autostart_choice, RichText::new("Start with Windows and optimize games automatically").small());
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    go = primary_button(ui, "Optimize").clicked();
                });
            });
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
        let (preset, custom) = {
            let c = self.config.lock().unwrap();
            (c.preset.clone(), c.is_custom())
        };
        let presets: Vec<(String, String, String)> = self.catalog().preset.iter().map(|p| (p.id.clone(), p.title.clone(), p.summary.clone())).collect();
        let current = if preset == "none" {
            "Off".to_string()
        } else {
            presets.iter().find(|p| p.0 == preset).map(|p| p.1.clone()).unwrap_or_default()
        };
        let shown = if custom { format!("{current} (customized)") } else { current };
        let mut choose = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Preset").color(color::MUTED));
            egui::ComboBox::from_id_salt("preset").selected_text(shown).width(220.0).show_ui(ui, |ui| {
                for (id, title, summary) in &presets {
                    if ui.selectable_label(*id == preset && !custom, title).on_hover_text(summary).clicked() {
                        choose = Some(id.clone());
                    }
                }
            });
        });
        if let Some(p) = choose {
            self.set_preset(&p);
        }
    }

    fn group(&mut self, ui: &mut Ui, title: &str, hint: &str, tweaks: Vec<Tweak>) {
        if tweaks.is_empty() {
            return;
        }
        ui.add_space(8.0);
        ui.label(RichText::new(title).strong().color(color::TEXT));
        ui.label(RichText::new(hint).small().color(color::MUTED));
        ui.add_space(2.0);
        for t in tweaks {
            self.tweak_row(ui, &t);
        }
    }

    fn tweak_row(&mut self, ui: &mut Ui, t: &Tweak) {
        let cfg = self.config.lock().unwrap().clone();
        let tier = cfg.tier();
        let locked = t.pro && tier == Tier::Free;
        let state = self.state(&t.id);
        let unavailable = matches!(state, TweakState::Unavailable(_));
        let mut on = cfg.wants(t) && !locked;
        let open = self.expanded.contains(&t.id);

        let resp = egui::Frame::NONE
            .fill(color::CARD)
            .stroke(Stroke::new(1.0, color::BORDER))
            .corner_radius(10)
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let sw = ui.add_enabled(!locked && !unavailable, Toggle(&mut on));
                    if sw.changed() {
                        self.toggle(t, on);
                    }
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let title = RichText::new(&t.title).color(if unavailable { color::MUTED } else { color::TEXT });
                            if ui.add(egui::Label::new(title).sense(Sense::click())).clicked() {
                                if open {
                                    self.expanded.remove(&t.id);
                                } else {
                                    self.expanded.insert(t.id.clone());
                                }
                            }
                            if t.pro && tier != Tier::Pro {
                                tag(ui, "PRO", color::PRO);
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            risk_dot(ui, t.risk);
                            let (status, c) = state_text(&state, t, on);
                            ui.label(RichText::new(status).small().color(c));
                            for g in &t.genres {
                                ui.label(RichText::new(g.label()).small().color(color::MUTED));
                            }
                        });
                    });
                });
                if open {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&t.summary).small().color(color::TEXT));
                    ui.label(RichText::new(&t.detail).small().color(color::MUTED));
                    ui.label(RichText::new(format!("{}{}", t.risk.label(), if t.reboot { " · needs a restart" } else { "" })).small().color(color::MUTED));
                    for s in &t.sources {
                        ui.hyperlink_to(RichText::new("Source").small(), s);
                    }
                }
            })
            .response;
        if resp.hovered() {
            ui.painter().rect_stroke(resp.rect, 10, Stroke::new(1.0, color::ACCENT_DIM), StrokeKind::Inside);
        }
        ui.add_space(4.0);
    }

    fn footer(&mut self, ui: &mut Ui, tier: Tier) {
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new("Restore all").color(color::TEXT)).fill(color::CARD)).on_hover_text("Undo every change LeanFPS made").clicked() {
                self.restore_all();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if tier != Tier::Pro && primary_button(ui, &format!("Unlock Pro · {PRICE}")).on_hover_text("One-time payment. Opens your browser.").clicked() {
                    open_url(crate::license::BUY_URL);
                }
            });
        });
        if let Some((msg, ok, at)) = &self.toast {
            if at.elapsed() < Duration::from_secs(6) {
                ui.label(RichText::new(msg).small().color(if *ok { color::ACCENT } else { color::BAD }));
            }
        }
    }
}

impl eframe::App for App {
    /// Runs even while the window is hidden in the tray.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        use crate::tray::TrayEvent;
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

        egui::Frame::NONE.fill(color::BG).inner_margin(Margin::same(14)).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            self.header(ui, tier);
            ui.add_space(10.0);
            self.game_card(ui);
            ui.add_space(8.0);
            self.candidate(ui);
            if first_run {
                self.welcome(ui);
                ui.add_space(8.0);
            }
            self.preset_row(ui);
            let footer_h = 56.0;
            egui::ScrollArea::vertical().max_height(ui.available_height() - footer_h).auto_shrink([false, false]).show(ui, |ui| {
                let cat = self.catalog().clone();
                let by = |f: &dyn Fn(&Tweak) -> bool| cat.tweak.iter().filter(|t| f(t)).cloned().collect::<Vec<_>>();
                self.group(ui, "System", "Changed once, kept until you undo them.", by(&|t| t.scope == Scope::Persistent));
                self.group(ui, "While gaming", "On when a game starts, undone when it closes.", by(&|t| t.scope == Scope::Session && !t.is_genre_specific()));
                self.group(ui, "Game profiles", "Extra changes for the type of game you are playing.", by(&|t| t.is_genre_specific()));
            });
            ui.add_space(6.0);
            self.footer(ui, tier);
        });

        if has_session || self.toast.as_ref().is_some_and(|t| t.2.elapsed() < Duration::from_secs(6)) {
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
    let mut v = egui::Visuals::dark();
    v.panel_fill = color::BG;
    v.window_fill = color::CARD;
    v.extreme_bg_color = color::BG;
    v.widgets.inactive.weak_bg_fill = color::CARD;
    v.widgets.inactive.bg_fill = color::CARD;
    v.widgets.hovered.weak_bg_fill = color::CARD_HOVER;
    v.widgets.hovered.bg_fill = color::CARD_HOVER;
    v.widgets.active.weak_bg_fill = color::ACCENT_DIM;
    v.hyperlink_color = color::ACCENT;
    v.selection.bg_fill = color::ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0, color::ACCENT);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(6.0, 4.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        use egui::TextStyle;
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
        s.text_styles.insert(TextStyle::Body, FontId::proportional(13.5));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(13.5));
    });
}

fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) {
    egui::Frame::NONE
        .fill(color::CARD)
        .stroke(Stroke::new(1.0, color::BORDER))
        .corner_radius(12)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
}

fn pill(ui: &mut Ui, text: &str, c: Color32) {
    egui::Frame::NONE.stroke(Stroke::new(1.0, c)).corner_radius(10).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
        ui.label(RichText::new(text).small().color(c));
    });
}

fn tag(ui: &mut Ui, text: &str, c: Color32) {
    ui.label(RichText::new(text).size(9.0).strong().color(c));
}

fn primary_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).strong().color(color::BG)).fill(color::ACCENT).corner_radius(8))
}

fn risk_dot(ui: &mut Ui, r: Risk) {
    let c = match r {
        Risk::Low => color::ACCENT,
        Risk::Medium => color::WARN,
        Risk::High => color::BAD,
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, c);
    resp.on_hover_text(r.label());
}

fn compare_line(avg: f64, low: f64, usual_avg: f64, usual_low: f64, n: usize) -> String {
    let pct = |a: f64, b: f64| if b > 0.0 { (a / b - 1.0) * 100.0 } else { 0.0 };
    format!(
        "{:+.0}% average, {:+.0}% 1% low vs your last {} session{} of this game",
        pct(avg, usual_avg),
        pct(low, usual_low),
        n,
        if n == 1 { "" } else { "s" }
    )
}

fn applied_line(ui: &mut Ui, a: &crate::session::Applied, strong: bool) {
    ui.horizontal(|ui| {
        check_mark(ui, if a.result.is_ok() { color::ACCENT } else { color::WARN });
        let mut t = RichText::new(&a.title).small().color(color::TEXT);
        if strong {
            t = t.strong();
        }
        let r = ui.label(t);
        if let Err(e) = &a.result {
            r.on_hover_text(format!("Skipped: {e}"));
        } else if let Some(n) = &a.note {
            r.on_hover_text(n);
        }
    });
    if let Some(n) = &a.note {
        ui.label(RichText::new(format!("    {n}")).small().color(color::MUTED));
    }
}

fn check_mark(ui: &mut Ui, c: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    let p = ui.painter();
    let a = rect.center() + Vec2::new(-4.0, 0.0);
    let b = rect.center() + Vec2::new(-1.0, 3.0);
    let d = rect.center() + Vec2::new(4.5, -3.5);
    p.line_segment([a, b], Stroke::new(2.0, c));
    p.line_segment([b, d], Stroke::new(2.0, c));
}

fn state_text(state: &TweakState, t: &Tweak, on: bool) -> (String, Color32) {
    match state {
        TweakState::Applied => ("Applied".into(), color::ACCENT),
        TweakState::AlreadySet => ("Already set on this PC".into(), color::ACCENT),
        TweakState::NotApplied if on => ("Not applied yet".into(), color::WARN),
        TweakState::NotApplied => ("Off".into(), color::MUTED),
        TweakState::ChangedOutside => ("Changed by another app".into(), color::WARN),
        TweakState::Session if on && t.is_genre_specific() => ("On for matching games".into(), color::ACCENT),
        TweakState::Session if on => ("On while gaming".into(), color::ACCENT),
        TweakState::Session => ("Off".into(), color::MUTED),
        TweakState::Unavailable(r) => (r.clone(), color::MUTED),
    }
}

/// iOS-style switch.
struct Toggle<'a>(&'a mut bool);

impl egui::Widget for Toggle<'_> {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        let size = Vec2::new(34.0, 20.0);
        let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
        if resp.clicked() {
            *self.0 = !*self.0;
            resp.mark_changed();
        }
        let t = ui.ctx().animate_bool_with_time(resp.id, *self.0, 0.15);
        let enabled = ui.is_enabled();
        let bg = if *self.0 { color::ACCENT } else { color::BORDER };
        let bg = if enabled { bg } else { bg.gamma_multiply(0.4) };
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(10), bg);
        let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, t);
        p.circle_filled(egui::pos2(x, rect.center().y), 7.5, if enabled { Color32::WHITE } else { color::MUTED });
        if resp.hovered() && enabled {
            p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, color::TEXT.gamma_multiply(0.4)), StrokeKind::Outside);
        }
        resp
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
