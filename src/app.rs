//! A ilha: estados, animações e desenho.

use std::collections::HashMap;

use eframe::egui::{
    self, ecolor::Hsva, epaint::Shadow, pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect,
    Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use crate::anim::{meter, smoothstep, Spring};
use crate::backend::{Backend, Command, Snapshot, System};
use crate::platform;
use crate::updater;

/// Tamanho da janela invisível que contém a ilha.
pub const WINDOW_SIZE: [f32; 2] = [560.0, 540.0];

const TOP_MARGIN: f32 = 10.0;
const PILL_HEIGHT: f32 = 34.0;
const HUD_SIZE: Vec2 = vec2(300.0, 42.0);
const EXPANDED_WIDTH: f32 = 420.0;
const PADDING: f32 = 18.0;
const HEADER_HEIGHT: f32 = 52.0;
const ROW_HEIGHT: f32 = 38.0;
const STATS_HEIGHT: f32 = 58.0;
const FOOTER_HEIGHT: f32 = 30.0;
const WELCOME_SIZE: Vec2 = vec2(380.0, 166.0);
/// Quanto do brilho em volta da ilha fica visível, em pontos.
const GLOW_MARGIN: f32 = 30.0;
const MAX_APPS: usize = 7;
const BARS: usize = 5;

const HUD_SECONDS: f32 = 1.6;
const LEAVE_SECONDS: f32 = 0.45;

const ISLAND: Color32 = Color32::from_rgb(9, 9, 11);
const TEXT: Color32 = Color32::from_rgb(245, 245, 247);
const TEXT_DIM: Color32 = Color32::from_rgb(150, 150, 158);
const MASTER_KEY: &str = "\0master";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Cartão inicial: pergunta se deve abrir junto com o Windows.
    Welcome,
    /// Pílula pequena: relógio + visualizador.
    Collapsed,
    /// Aviso rápido de volume (quando você usa as teclas do teclado).
    Hud,
    /// Painel completo: mídia + mixer.
    Expanded,
}

pub struct IslandApp {
    backend: Backend,
    mode: Mode,

    width: Spring,
    height: Spring,
    /// 0 = pílula, 1 = painel aberto. Controla o fade do conteúdo.
    open: Spring,
    /// 0..1, opacidade do aviso de volume.
    hud: Spring,
    /// 0..1, opacidade do cartão de boas-vindas.
    welcome: Spring,
    /// Opção marcada no cartão de boas-vindas.
    autostart: bool,
    /// 0..1, mouse em cima da ilha.
    hover: Spring,

    hud_timer: f32,
    leave_timer: f32,
    uptime: f32,
    last_master: Option<(f32, bool)>,

    /// Valores suavizados para os medidores de áudio.
    master_peak: f32,
    app_peaks: HashMap<String, f32>,
    bars: [f32; BARS],
    /// CPU, RAM e temperatura suavizados (0..1) para os mostradores.
    gauges: [f32; 3],

    /// Enquanto arrasta um slider, mostramos o valor do mouse (sem atraso).
    dragging: Option<(String, f32)>,

    window_pos: Option<Pos2>,
    passthrough: Option<bool>,
    clip: Option<[i32; 5]>,
    clip_size: Vec2,
    /// Segundos restantes do aviso "Atualizado" na pílula.
    notice_timer: f32,
    restarting: bool,
}

impl IslandApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, show_welcome: bool, updated: bool) -> Self {
        let mode = match std::env::var("ISLANDBAR_MODE").as_deref() {
            Ok("collapsed") => Mode::Collapsed,
            _ if show_welcome => Mode::Welcome,
            Ok("expanded") => Mode::Expanded,
            Ok("hud") => Mode::Hud,
            _ => Mode::Collapsed,
        };
        Self {
            backend: Backend::spawn(),
            mode,
            // Começa minúscula para "nascer" com um quique.
            width: Spring::bouncy(30.0),
            height: Spring::bouncy(10.0),
            open: Spring::smooth(0.0),
            hud: Spring::smooth(0.0),
            welcome: Spring::smooth(0.0),
            autostart: platform::autostart_enabled(),
            hover: Spring::smooth(0.0),
            hud_timer: if mode == Mode::Hud { f32::INFINITY } else { 0.0 },
            leave_timer: 0.0,
            uptime: 0.0,
            last_master: None,
            master_peak: 0.0,
            app_peaks: HashMap::new(),
            bars: [0.0; BARS],
            gauges: [0.0; 3],
            dragging: None,
            window_pos: None,
            passthrough: None,
            clip: None,
            clip_size: Vec2::ZERO,
            notice_timer: if updated { 6.0 } else { 0.0 },
            restarting: false,
        }
    }

    /// Mantém a janela centralizada no topo do monitor.
    fn place_window(&mut self, ctx: &egui::Context) {
        let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) else { return };
        let want = pos2(((monitor.x - WINDOW_SIZE[0]) / 2.0).round(), 0.0);
        if self.window_pos != Some(want) {
            self.window_pos = Some(want);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(want));
        }
    }

    /// O mouse está em cima da ilha?
    fn cursor_inside(&self, ctx: &egui::Context, island: Rect) -> bool {
        let hit = island.expand(6.0);

        #[cfg(windows)]
        {
            let inner = ctx.input(|i| i.viewport().inner_rect);
            if let (Some(inner), Some((x, y))) = (inner, crate::platform::cursor_pos()) {
                let scale = ctx.pixels_per_point();
                let local = pos2(x as f32 / scale - inner.min.x, y as f32 / scale - inner.min.y);
                return hit.contains(local);
            }
        }

        ctx.pointer_latest_pos().is_some_and(|p| hit.contains(p))
    }

    /// Fora da ilha, os cliques atravessam a janela e vão para o app de baixo.
    fn set_passthrough(&mut self, ctx: &egui::Context, enabled: bool) {
        if cfg!(windows) && self.passthrough != Some(enabled) {
            self.passthrough = Some(enabled);
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(enabled));
        }
    }

    /// Deixa a janela existir só onde está a ilha (mais o brilho em volta).
    ///
    /// O recorte muda poucas vezes: abre espaço de uma vez quando uma animação
    /// começa e só aperta de novo quando ela termina. Mudar a cada frame fazia
    /// a ilha piscar.
    fn clip_window(&mut self, ctx: &egui::Context, frame: &eframe::Frame, island: Rect) {
        let settled = self.width.settled() && self.height.settled();
        let target = vec2(self.width.target, self.height.target);
        let (size, margin) = if settled {
            (target, GLOW_MARGIN)
        } else {
            // Folga extra para o "quique" da mola não ser cortado.
            (self.clip_size.max(target).max(island.size()), GLOW_MARGIN + 45.0)
        };
        self.clip_size = size;

        let area = Rect::from_min_size(pos2(island.center().x - size.x / 2.0, TOP_MARGIN), size).expand(margin);
        let radius = (size.y / 2.0).min(26.0) + margin;
        let scale = ctx.pixels_per_point();
        let px = |v: f32| (v * scale).round() as i32;
        let clip = [
            px(area.left()),
            // As primeiras linhas da janela ficam de fora: o Windows desenha
            // uma faixa clara ali em janelas sem borda.
            px(area.top().max(TOP_MARGIN - 5.0)),
            px(area.right()),
            px(area.bottom()),
            px(radius * 2.0),
        ];
        if self.clip != Some(clip) {
            self.clip = Some(clip);
            platform::clip_window(frame, [clip[0], clip[1], clip[2], clip[3]], clip[4]);
        }
    }

    fn target_size(&self, snap: &Snapshot) -> Vec2 {
        match self.mode {
            Mode::Welcome => WELCOME_SIZE,
            Mode::Collapsed => {
                vec2(250.0, PILL_HEIGHT) + vec2(14.0, 4.0) * self.hover.target
            }
            Mode::Hud => HUD_SIZE,
            Mode::Expanded => {
                let rows = 1 + snap.sessions.len().min(MAX_APPS);
                let height = PADDING + HEADER_HEIGHT + 14.0 + rows as f32 * ROW_HEIGHT + STATS_HEIGHT + FOOTER_HEIGHT;
                vec2(EXPANDED_WIDTH, height)
            }
        }
    }

    fn update_meters(&mut self, snap: &Snapshot, dt: f32, time: f32) {
        self.master_peak = meter(self.master_peak, snap.master_peak, dt);
        for s in &snap.sessions {
            let peak = self.app_peaks.entry(s.name.clone()).or_default();
            *peak = meter(*peak, if s.muted { 0.0 } else { s.peak }, dt);
        }
        self.app_peaks.retain(|name, _| snap.sessions.iter().any(|s| &s.name == name));

        let sys = &snap.system;
        let targets = [sys.cpu, sys.ram(), sys.temp_c.map_or(0.0, temp_fraction)];
        for (gauge, target) in self.gauges.iter_mut().zip(targets) {
            *gauge += (target - *gauge) * (1.0 - (-5.0 * dt).exp());
        }

        // Barrinhas: cada uma dança em um ritmo, todas seguem o volume real.
        let energy = (snap.master_peak * 2.2).min(1.0);
        for (i, bar) in self.bars.iter_mut().enumerate() {
            let k = i as f32;
            let wobble = 0.35 + 0.65 * (time * (7.0 + k * 2.3) + k * 1.9).sin().abs();
            *bar = meter(*bar, energy * wobble, dt);
        }
    }
}

impl eframe::App for IslandApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Variável só para testes: fundo cinza em vez de transparente.
        if std::env::var_os("ISLANDBAR_DEBUG_BG").is_some() {
            return [0.45, 0.47, 0.52, 1.0];
        }
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let (dt, time) = ctx.input(|i| (i.stable_dt.min(1.0 / 30.0), i.time as f32));
        self.uptime += dt;
        self.notice_timer = (self.notice_timer - dt).max(0.0);

        // Uma ilha mais nova abriu (ex.: depois de atualizar): esta sai de cena.
        if platform::quit_requested() {
            std::process::exit(0);
        }
        // Atualização baixada: reinicia quando a ilha não está em uso.
        if updater::ready() && self.mode == Mode::Collapsed && !self.restarting {
            self.restarting = true;
            updater::restart();
        }
        let snap = self.backend.snapshot();

        self.place_window(&ctx);
        self.update_meters(&snap, dt, time);

        // --- Aviso de volume quando ele muda "por fora" (teclado, outro app) ---
        let master_now = (snap.master, snap.master_muted);
        if let Some(before) = self.last_master {
            let changed = (before.0 - master_now.0).abs() > 0.004 || before.1 != master_now.1;
            let idle = matches!(self.mode, Mode::Collapsed | Mode::Hud);
            if changed && idle && self.uptime > 1.5 {
                self.mode = Mode::Hud;
                self.hud_timer = HUD_SECONDS;
            }
        }
        self.last_master = Some(master_now);

        // --- Geometria atual da ilha ---
        let size = self.target_size(&snap);
        self.width.target = size.x;
        self.height.target = size.y;
        self.width.update(dt);
        self.height.update(dt);
        let (w, h) = (self.width.value.max(8.0), self.height.value.max(8.0));
        let island = Rect::from_min_size(pos2(ui.max_rect().center().x - w / 2.0, TOP_MARGIN), vec2(w, h));
        let radius = (h / 2.0).min(26.0);
        self.clip_window(&ctx, frame, island);

        // --- Mouse e troca de estados ---
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        let hovered = self.cursor_inside(&ctx, island);
        self.set_passthrough(&ctx, !hovered && !pointer_down);

        match self.mode {
            Mode::Expanded => {
                if hovered || pointer_down {
                    self.leave_timer = 0.0;
                } else {
                    self.leave_timer += dt;
                    if self.leave_timer > LEAVE_SECONDS && std::env::var_os("ISLANDBAR_MODE").is_none() {
                        self.mode = Mode::Collapsed;
                    }
                }
            }
            Mode::Hud => {
                self.hud_timer -= dt;
                if self.hud_timer <= 0.0 {
                    self.mode = Mode::Collapsed;
                }
            }
            Mode::Collapsed | Mode::Welcome => {}
        }

        if matches!(self.mode, Mode::Collapsed | Mode::Hud) {
            let click = ui.interact(island, ui.scope_id().with("island"), Sense::click());
            if click.clicked() {
                self.mode = Mode::Expanded;
                self.leave_timer = 0.0;
            }
            if click.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }

        self.open.target = if self.mode == Mode::Expanded { 1.0 } else { 0.0 };
        self.hud.target = if self.mode == Mode::Hud { 1.0 } else { 0.0 };
        self.hover.target = if hovered { 1.0 } else { 0.0 };
        self.open.update(dt);
        self.welcome.target = if self.mode == Mode::Welcome { 1.0 } else { 0.0 };
        self.welcome.update(dt);
        self.hud.update(dt);
        self.hover.update(dt);

        // --- Desenho ---
        let accent = accent_color(time);
        let painter = ui.painter().clone();

        // Brilho colorido que pulsa com o som.
        let glow = (0.10 + self.master_peak * 0.9 + self.hover.value * 0.15).min(1.0);
        painter.add(
            Shadow { offset: [0, 0], blur: 26, spread: 1, color: fade(accent, glow * 0.55) }
                .as_shape(island, radius),
        );
        painter.add(
            Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(110) }
                .as_shape(island, radius),
        );
        painter.rect_filled(island, radius, ISLAND);
        painter.rect_stroke(
            island,
            radius,
            Stroke::new(1.0, fade(accent, 0.18 + self.master_peak * 0.5)),
            StrokeKind::Inside,
        );

        let content = painter.with_clip_rect(island.shrink(1.0));
        let open = smoothstep(self.open.value);
        let hud = smoothstep(self.hud.value);

        let welcome = smoothstep(self.welcome.value);
        let pill_alpha = (1.0 - open * 2.0).clamp(0.0, 1.0) * (1.0 - hud) * (1.0 - welcome * 2.0).max(0.0);
        if pill_alpha > 0.01 {
            self.draw_pill(&content, island, &snap, accent, pill_alpha);
        }
        if hud > 0.01 && open < 0.5 {
            self.draw_hud(&content, island, &snap, accent, hud * (1.0 - open * 2.0));
        }
        if welcome > 0.01 {
            self.draw_welcome(ui, &content, island, accent);
        }
        if open > 0.01 {
            self.draw_panel(ui, &content, island, &snap, accent);
        }

        // --- Quando redesenhar ---
        let animating = [&self.width, &self.height, &self.open, &self.hud, &self.hover, &self.welcome]
            .iter()
            .any(|s| !s.settled());
        if animating || hovered || self.mode != Mode::Collapsed || self.master_peak > 0.005 {
            ctx.request_repaint();
        } else {
            // Parada, a ilha quase não gasta CPU: só confere o mouse 20x/s.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

// ---------------------------------------------------------------------------
// Conteúdo de cada estado
// ---------------------------------------------------------------------------

impl IslandApp {
    /// Pílula: barrinhas | música atual | relógio.
    fn draw_pill(&self, p: &Painter, island: Rect, snap: &Snapshot, accent: Color32, alpha: f32) {
        let cy = island.center().y;
        let left = island.left() + 16.0;
        let right = island.right() - 14.0;

        draw_bars(p, pos2(left, cy), &self.bars, 16.0, fade(accent, alpha));

        let clock = chrono::Local::now().format("%H:%M").to_string();
        let clock_rect = p.text(
            pos2(right, cy),
            Align2::RIGHT_CENTER,
            clock,
            FontId::proportional(13.5),
            fade(TEXT, alpha),
        );

        if self.notice_timer > 0.0 {
            let x = left + 32.0;
            let notice = match updater::BUILD {
                Some(build) => format!("Atualizado: versão {build}"),
                None => "Atualizado".to_string(),
            };
            text_fit(p, pos2(x, cy), Align2::LEFT_CENTER, &notice, 12.5, fade(accent, alpha), clock_rect.left() - 10.0 - x);
        } else if let Some(media) = &snap.media {
            let x = left + 32.0;
            let max_width = clock_rect.left() - 10.0 - x;
            let color = if media.playing { TEXT } else { TEXT_DIM };
            text_fit(p, pos2(x, cy), Align2::LEFT_CENTER, &media.title, 12.5, fade(color, alpha), max_width);
        } else {
            // Sem música: mostra o uso do PC.
            let sys = &snap.system;
            let usage = format!("CPU {:.0}%  RAM {:.0}%", sys.cpu * 100.0, sys.ram() * 100.0);
            let x = left + 32.0;
            text_fit(p, pos2(x, cy), Align2::LEFT_CENTER, &usage, 12.0, fade(TEXT_DIM, alpha), clock_rect.left() - 10.0 - x);
        }
    }

    /// Aviso de volume: ícone | barra | porcentagem.
    fn draw_hud(&self, p: &Painter, island: Rect, snap: &Snapshot, accent: Color32, alpha: f32) {
        let cy = island.center().y;
        let level = if snap.master_muted { 0.0 } else { snap.master };
        draw_speaker(p, pos2(island.left() + 26.0, cy), 9.0, level, snap.master_muted, fade(TEXT, alpha));

        let track = Rect::from_min_max(
            pos2(island.left() + 50.0, cy - 3.5),
            pos2(island.right() - 56.0, cy + 3.5),
        );
        p.rect_filled(track, 4.0, fade(Color32::WHITE, alpha * 0.14));
        if level > 0.0 {
            let fill = Rect::from_min_size(track.min, vec2((track.width() * level).max(7.0), track.height()));
            p.rect_filled(fill, 4.0, fade(accent, alpha));
        }

        let label = if snap.master_muted { "mudo".to_string() } else { format!("{:.0}%", snap.master * 100.0) };
        p.text(
            pos2(island.right() - 18.0, cy),
            Align2::RIGHT_CENTER,
            label,
            FontId::proportional(13.0),
            fade(TEXT, alpha),
        );
    }

    /// Painel aberto: mídia em cima, mixer embaixo.
    fn draw_panel(&mut self, ui: &mut Ui, p: &Painter, island: Rect, snap: &Snapshot, accent: Color32) {
        let interactive = self.mode == Mode::Expanded;
        let left = island.left() + PADDING;
        let right = island.right() - PADDING;
        let mut row = 0usize;
        // Cada linha entra um pouquinho depois da anterior (efeito cascata).
        let mut next_alpha = |open: f32| {
            let a = smoothstep(open * 1.9 - 0.25 - row as f32 * 0.09);
            row += 1;
            a
        };
        let open = self.open.value;

        // ---- Cabeçalho: tocando agora ----
        let a = next_alpha(open);
        let top = island.top() + PADDING - (1.0 - a) * 10.0;
        let art = Rect::from_min_size(pos2(left, top), Vec2::splat(HEADER_HEIGHT));
        p.rect_filled(art, 14.0, fade(accent, a * 0.22));
        p.rect_stroke(art, 14.0, Stroke::new(1.0, fade(accent, a * 0.5)), StrokeKind::Inside);
        let bars_width = BARS as f32 * 6.0 - 3.0;
        draw_bars(p, pos2(art.center().x - bars_width / 2.0, art.center().y), &self.bars, 26.0, fade(accent, a));

        let buttons_left = right - 3.0 * 36.0;
        let text_x = art.right() + 14.0;
        let text_width = buttons_left - 8.0 - text_x;
        let cy = art.center().y;
        match &snap.media {
            Some(media) => {
                let artist = if media.artist.is_empty() { "Tocando agora" } else { &media.artist };
                text_fit(p, pos2(text_x, cy - 10.0), Align2::LEFT_CENTER, &media.title, 15.0, fade(TEXT, a), text_width);
                text_fit(p, pos2(text_x, cy + 10.0), Align2::LEFT_CENTER, artist, 12.5, fade(TEXT_DIM, a), text_width);
            }
            None => {
                let date = chrono::Local::now().format("%d/%m/%Y").to_string();
                text_fit(p, pos2(text_x, cy - 10.0), Align2::LEFT_CENTER, "Nada tocando", 15.0, fade(TEXT, a), text_width);
                text_fit(p, pos2(text_x, cy + 10.0), Align2::LEFT_CENTER, &date, 12.5, fade(TEXT_DIM, a), text_width);
            }
        }

        let has_media = snap.media.is_some();
        let playing = snap.media.as_ref().is_some_and(|m| m.playing);
        let media_alpha = if has_media { a } else { a * 0.35 };
        let icons = [
            (MediaIcon::Prev, Command::Prev),
            (if playing { MediaIcon::Pause } else { MediaIcon::Play }, Command::PlayPause),
            (MediaIcon::Next, Command::Next),
        ];
        for (i, (icon, cmd)) in icons.into_iter().enumerate() {
            let center = pos2(buttons_left + 18.0 + i as f32 * 36.0, cy);
            let (clicked, scale) = icon_button(ui, p, ("media", i), center, 16.0, interactive && has_media, media_alpha);
            draw_media_icon(p, center, 7.5 * scale, icon, fade(TEXT, media_alpha));
            if clicked {
                self.backend.send(cmd);
            }
        }

        let line_y = island.top() + PADDING + HEADER_HEIGHT + 7.0;
        p.line_segment(
            [pos2(left, line_y), pos2(right, line_y)],
            Stroke::new(1.0, fade(Color32::WHITE, a * 0.08)),
        );

        // ---- Mixer ----
        let mut y = line_y + 7.0;
        let mut rows: Vec<(String, String, f32, bool, f32)> = vec![(
            MASTER_KEY.to_string(),
            "Geral".to_string(),
            snap.master,
            snap.master_muted,
            self.master_peak,
        )];
        for s in snap.sessions.iter().take(MAX_APPS) {
            let peak = self.app_peaks.get(&s.name).copied().unwrap_or(0.0);
            rows.push((s.name.clone(), s.name.clone(), s.volume, s.muted, peak));
        }

        let mut still_dragging = false;
        for (key, label, volume, muted, peak) in rows {
            let a = next_alpha(open);
            let is_master = key == MASTER_KEY;
            let rect = Rect::from_min_max(pos2(left, y), pos2(right, y + ROW_HEIGHT)).translate(vec2(0.0, (1.0 - a) * 12.0));
            y += ROW_HEIGHT;
            let cy = rect.center().y;
            let color = if is_master { accent } else { name_color(&label) };
            let dim = if muted { 0.4 } else { 1.0 };

            // Ícone (clique = mudo).
            let icon_center = pos2(left + 14.0, cy);
            let (clicked, scale) = icon_button(ui, p, ("mute", &key), icon_center, 15.0, interactive, a);
            if is_master {
                draw_speaker(p, icon_center, 8.5 * scale, volume, muted, fade(TEXT, a * dim));
            } else {
                p.circle_filled(icon_center, 11.0 * scale, fade(color, a * dim * 0.9));
                let initial = label.chars().next().unwrap_or('?').to_uppercase().to_string();
                p.text(icon_center, Align2::CENTER_CENTER, initial, FontId::proportional(12.0), fade(ISLAND, a));
                if muted {
                    let d = vec2(9.0, 9.0) * scale;
                    p.line_segment([icon_center - d, icon_center + d], Stroke::new(2.0, fade(TEXT, a)));
                }
            }
            if clicked {
                self.backend.send(if is_master {
                    Command::ToggleMasterMute
                } else {
                    Command::ToggleSessionMute(key.clone())
                });
            }

            text_fit(p, pos2(left + 38.0, cy), Align2::LEFT_CENTER, &label, 13.0, fade(TEXT, a * dim), 92.0);

            // Slider.
            let shown = match &self.dragging {
                Some((k, v)) if *k == key => *v,
                _ => volume,
            };
            let slider_rect = Rect::from_min_max(pos2(left + 138.0, rect.top()), pos2(right - 46.0, rect.bottom()));
            let result = slider(ui, p, &key, slider_rect, shown, peak, color, a * dim, interactive);
            if let Some(value) = result.changed {
                self.backend.send(if is_master {
                    Command::SetMaster(value)
                } else {
                    Command::SetSession(key.clone(), value)
                });
                self.dragging = Some((key.clone(), value));
            }
            if result.active && self.dragging.as_ref().is_some_and(|(k, _)| *k == key) {
                still_dragging = true;
            }

            p.text(
                pos2(right, cy),
                Align2::RIGHT_CENTER,
                format!("{:.0}%", shown * 100.0),
                FontId::proportional(12.5),
                fade(TEXT_DIM, a),
            );
        }
        if !still_dragging {
            self.dragging = None;
        }

        // ---- Monitor do sistema ----
        let a = next_alpha(open);
        self.draw_stats(p, pos2(left, y + 6.0 + (1.0 - a) * 12.0), right - left, &snap.system, a);

        // ---- Rodapé ----
        let a = next_alpha(open);
        let footer_y = island.bottom() - FOOTER_HEIGHT / 2.0 - 3.0;
        let mut clock = chrono::Local::now().format("%H:%M").to_string();
        if let Some(build) = updater::BUILD {
            clock += &format!("  ·  versão {build}");
        }
        p.text(pos2(left, footer_y), Align2::LEFT_CENTER, clock, FontId::proportional(12.0), fade(TEXT_DIM, a));

        let quit = Rect::from_min_max(pos2(right - 40.0, footer_y - 11.0), pos2(right, footer_y + 11.0));
        let quit_resp = if interactive {
            Some(ui.interact(quit, ui.scope_id().with("quit"), Sense::click()))
        } else {
            None
        };
        let quit_hover = quit_resp.as_ref().is_some_and(|r| r.hovered());
        let quit_color = if quit_hover { Color32::from_rgb(255, 105, 97) } else { TEXT_DIM };
        p.text(pos2(right, footer_y), Align2::RIGHT_CENTER, "Sair", FontId::proportional(12.0), fade(quit_color, a));
        if quit_resp.is_some_and(|r| r.clicked()) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl IslandApp {
    /// Três mostradores lado a lado: CPU, RAM e temperatura.
    fn draw_stats(&self, p: &Painter, origin: Pos2, width: f32, sys: &System, alpha: f32) {
        let gap = 8.0;
        let tile = vec2((width - 2.0 * gap) / 3.0, STATS_HEIGHT - 12.0);
        let tiles = [
            ("CPU", format!("{:.0}%", sys.cpu * 100.0), true),
            ("RAM", format!("{:.1}/{:.0} GB", sys.ram_used_gb, sys.ram_total_gb), true),
            ("Temp", sys.temp_c.map_or("n/d".to_string(), |t| format!("{t:.0}°C")), sys.temp_c.is_some()),
        ];
        for (i, (label, value, available)) in tiles.into_iter().enumerate() {
            let rect = Rect::from_min_size(origin + vec2(i as f32 * (tile.x + gap), 0.0), tile);
            p.rect_filled(rect, 12.0, fade(Color32::WHITE, alpha * 0.05));

            let level = if available { self.gauges[i] } else { 0.0 };
            let center = pos2(rect.left() + 24.0, rect.center().y);
            draw_ring(p, center, 12.5, level, fade(level_color(level), alpha), fade(Color32::WHITE, alpha * 0.12));

            let x = rect.left() + 46.0;
            p.text(pos2(x, rect.center().y - 8.0), Align2::LEFT_CENTER, label, FontId::proportional(11.0), fade(TEXT_DIM, alpha));
            let value_color = if available { TEXT } else { TEXT_DIM };
            text_fit(p, pos2(x, rect.center().y + 8.0), Align2::LEFT_CENTER, &value, 12.5, fade(value_color, alpha), rect.right() - 6.0 - x);
        }
    }

    /// Cartão inicial com a opção de abrir junto com o Windows.
    fn draw_welcome(&mut self, ui: &mut Ui, p: &Painter, island: Rect, accent: Color32) {
        let interactive = self.mode == Mode::Welcome;
        let a = smoothstep(self.welcome.value * 1.6 - 0.4);
        let left = island.left() + PADDING + 4.0;
        let right = island.right() - PADDING - 4.0;
        let top = island.top() + PADDING;

        p.text(pos2(left, top + 10.0), Align2::LEFT_CENTER, "Islandbar", FontId::proportional(17.0), fade(TEXT, a));
        p.text(
            pos2(left, top + 32.0),
            Align2::LEFT_CENTER,
            "Tudo pronto. Só uma pergunta antes de começar:",
            FontId::proportional(12.5),
            fade(TEXT_DIM, a),
        );

        // Linha da opção: clicar em qualquer lugar dela liga/desliga.
        let row = Rect::from_min_max(pos2(left, top + 52.0), pos2(right, top + 84.0));
        let id = ui.scope_id().with("autostart");
        if interactive {
            let resp = ui.interact(row, id, Sense::click());
            if resp.clicked() {
                self.autostart = !self.autostart;
            }
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }
        let on = ui.ctx().animate_bool_with_time(id, self.autostart, 0.16);
        p.text(
            pos2(left, row.center().y),
            Align2::LEFT_CENTER,
            "Iniciar junto com o Windows",
            FontId::proportional(14.0),
            fade(TEXT, a),
        );
        let switch = Rect::from_center_size(pos2(right - 22.0, row.center().y), vec2(44.0, 24.0));
        p.rect_filled(switch, 12.0, fade(Color32::WHITE, a * 0.16 * (1.0 - on)));
        p.rect_filled(switch, 12.0, fade(accent, a * on));
        let knob = pos2(switch.left() + 12.0 + 20.0 * on, switch.center().y);
        p.circle_filled(knob, 9.0, fade(Color32::WHITE, a));

        // Botão "Iniciar".
        let button = Rect::from_min_max(pos2(right - 104.0, island.bottom() - PADDING - 32.0), pos2(right, island.bottom() - PADDING));
        let button_id = ui.scope_id().with("start");
        let mut hover = false;
        if interactive {
            let resp = ui.interact(button, button_id, Sense::click());
            hover = resp.hovered();
            if hover {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                platform::set_autostart(self.autostart);
                self.mode = Mode::Collapsed;
            }
        }
        let lift = ui.ctx().animate_bool_with_time(button_id, hover, 0.12);
        p.rect_filled(button.expand(lift * 1.5), 16.0, fade(accent, a * (0.85 + 0.15 * lift)));
        p.text(button.center(), Align2::CENTER_CENTER, "Iniciar", FontId::proportional(14.0), fade(ISLAND, a));
    }
}

// ---------------------------------------------------------------------------
// Peças reutilizáveis
// ---------------------------------------------------------------------------

struct SliderResult {
    /// Novo valor, se o usuário mexeu neste frame.
    changed: Option<f32>,
    /// O usuário está segurando este slider.
    active: bool,
}

/// Slider em formato de pílula; a parte clara mostra o som saindo ao vivo.
#[allow(clippy::too_many_arguments)]
fn slider(
    ui: &mut Ui,
    p: &Painter,
    key: &str,
    rect: Rect,
    value: f32,
    peak: f32,
    color: Color32,
    alpha: f32,
    interactive: bool,
) -> SliderResult {
    let id = ui.scope_id().with(("slider", key));
    let mut result = SliderResult { changed: None, active: false };
    let mut hot = false;

    if interactive {
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        result.active = resp.dragged() || resp.is_pointer_button_down_on();
        hot = resp.hovered() || result.active;
        if result.active {
            if let Some(pos) = resp.interact_pointer_pos() {
                let track_left = rect.left() + 4.0;
                let track_width = rect.width() - 8.0;
                result.changed = Some(((pos.x - track_left) / track_width).clamp(0.0, 1.0));
            }
        }
    }

    // A barra engorda quando o mouse passa por cima.
    let grow = ui.ctx().animate_bool_with_time(id, hot, 0.14);
    let thickness = 6.0 + 5.0 * grow;
    let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 8.0, thickness));
    let rounding = thickness / 2.0;
    let value = result.changed.unwrap_or(value).clamp(0.0, 1.0);

    p.rect_filled(track, rounding, fade(Color32::WHITE, alpha * 0.12));
    if value > 0.0 {
        let width = (track.width() * value).max(thickness);
        let fill = Rect::from_min_size(track.min, vec2(width, thickness));
        p.rect_filled(fill, rounding, fade(color, alpha * (0.55 + 0.2 * grow)));

        let live = width * peak.clamp(0.0, 1.0);
        if live > thickness * 0.5 {
            let live_rect = Rect::from_min_size(track.min, vec2(live.max(thickness), thickness));
            p.rect_filled(live_rect, rounding, fade(brighten(color), alpha));
        }
    }
    result
}

/// Área clicável redonda. Devolve (clicou?, escala para animar o ícone).
fn icon_button(
    ui: &mut Ui,
    p: &Painter,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    center: Pos2,
    radius: f32,
    interactive: bool,
    alpha: f32,
) -> (bool, f32) {
    if !interactive {
        return (false, 1.0);
    }
    let id = ui.scope_id().with(id_source);
    let resp = ui.interact(Rect::from_center_size(center, Vec2::splat(radius * 2.0)), id, Sense::click());
    let hover = ui.ctx().animate_bool_with_time(id, resp.hovered(), 0.12);
    let press = ui.ctx().animate_bool_with_time(id.with("press"), resp.is_pointer_button_down_on(), 0.08);
    if hover > 0.0 {
        p.circle_filled(center, radius * (1.0 - 0.1 * press), fade(Color32::WHITE, alpha * 0.12 * hover));
    }
    (resp.clicked(), 1.0 + 0.12 * hover - 0.2 * press)
}

/// Mostrador circular: um anel que enche no sentido horário.
fn draw_ring(p: &Painter, center: Pos2, radius: f32, level: f32, color: Color32, track: Color32) {
    let arc = |fraction: f32| -> Vec<Pos2> {
        let steps = (48.0 * fraction).ceil().max(1.0) as usize;
        (0..=steps)
            .map(|k| {
                let angle = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * fraction * k as f32 / steps as f32;
                center + vec2(angle.cos(), angle.sin()) * radius
            })
            .collect()
    };
    p.add(Shape::line(arc(1.0), Stroke::new(3.5, track)));
    let level = level.clamp(0.0, 1.0);
    if level > 0.005 {
        p.add(Shape::line(arc(level), Stroke::new(3.5, color)));
    }
}

/// Barrinhas de equalizador, a partir da esquerda, centradas na vertical.
fn draw_bars(p: &Painter, left_center: Pos2, bars: &[f32], max_height: f32, color: Color32) {
    for (i, level) in bars.iter().enumerate() {
        let height = 3.0 + level * (max_height - 3.0);
        let rect = Rect::from_center_size(
            pos2(left_center.x + 1.5 + i as f32 * 6.0, left_center.y),
            vec2(3.0, height),
        );
        p.rect_filled(rect, 1.5, color);
    }
}

#[derive(Clone, Copy)]
enum MediaIcon {
    Play,
    Pause,
    Next,
    Prev,
}

fn draw_media_icon(p: &Painter, c: Pos2, r: f32, icon: MediaIcon, color: Color32) {
    let triangle = |tip_x: f32, base_x: f32| {
        Shape::convex_polygon(
            vec![pos2(base_x, c.y - r), pos2(tip_x, c.y), pos2(base_x, c.y + r)],
            color,
            Stroke::NONE,
        )
    };
    match icon {
        MediaIcon::Play => {
            p.add(triangle(c.x + r, c.x - r * 0.7));
        }
        MediaIcon::Pause => {
            for side in [-1.0, 1.0] {
                let bar = Rect::from_center_size(pos2(c.x + side * r * 0.5, c.y), vec2(r * 0.55, r * 2.0));
                p.rect_filled(bar, 1.5, color);
            }
        }
        MediaIcon::Next | MediaIcon::Prev => {
            let dir = if matches!(icon, MediaIcon::Next) { 1.0 } else { -1.0 };
            let r = r * 0.8;
            let tri = Shape::convex_polygon(
                vec![
                    pos2(c.x - dir * r, c.y - r),
                    pos2(c.x + dir * r * 0.6, c.y),
                    pos2(c.x - dir * r, c.y + r),
                ],
                color,
                Stroke::NONE,
            );
            p.add(tri);
            let bar = Rect::from_center_size(pos2(c.x + dir * r * 0.9, c.y), vec2(2.5, r * 2.0));
            p.rect_filled(bar, 1.0, color);
        }
    }
}

/// Alto-falante com ondinhas conforme o volume (ou um X se estiver mudo).
fn draw_speaker(p: &Painter, c: Pos2, r: f32, level: f32, muted: bool, color: Color32) {
    let x = c.x - r * 0.9;
    p.add(Shape::convex_polygon(
        vec![
            pos2(x, c.y - r * 0.35),
            pos2(x + r * 0.45, c.y - r * 0.35),
            pos2(x + r * 0.95, c.y - r * 0.85),
            pos2(x + r * 0.95, c.y + r * 0.85),
            pos2(x + r * 0.45, c.y + r * 0.35),
            pos2(x, c.y + r * 0.35),
        ],
        color,
        Stroke::NONE,
    ));

    let stroke = Stroke::new(1.6, color);
    let origin = pos2(x + r * 0.95, c.y);
    if muted {
        let m = pos2(origin.x + r * 0.6, c.y);
        let d = r * 0.35;
        p.line_segment([m + vec2(-d, -d), m + vec2(d, d)], stroke);
        p.line_segment([m + vec2(-d, d), m + vec2(d, -d)], stroke);
        return;
    }
    let waves = if level > 0.66 { 3 } else if level > 0.33 { 2 } else if level > 0.0 { 1 } else { 0 };
    for i in 0..waves {
        let radius = r * (0.4 + 0.35 * i as f32);
        let points: Vec<Pos2> = (0..=10)
            .map(|k| {
                let angle = -0.85 + 1.7 * k as f32 / 10.0;
                origin + vec2(angle.cos(), angle.sin()) * radius
            })
            .collect();
        p.add(Shape::line(points, stroke));
    }
}

/// Escreve um texto em uma linha, cortando com "…" se não couber.
fn text_fit(p: &Painter, pos: Pos2, anchor: Align2, text: &str, size: f32, color: Color32, max_width: f32) {
    let font = FontId::proportional(size);
    let mut galley = p.layout_no_wrap(text.to_string(), font.clone(), color);
    if galley.size().x > max_width {
        let mut chars: Vec<char> = text.chars().collect();
        while chars.pop().is_some() && !chars.is_empty() {
            let shorter = format!("{}…", chars.iter().collect::<String>().trim_end());
            galley = p.layout_no_wrap(shorter, font.clone(), color);
            if galley.size().x <= max_width {
                break;
            }
        }
    }
    let rect = anchor.anchor_size(pos, galley.size());
    p.galley(rect.min, galley, color);
}

// ---------------------------------------------------------------------------
// Cores
// ---------------------------------------------------------------------------

/// Cor de destaque que passeia devagar pelo arco-íris.
fn accent_color(time: f32) -> Color32 {
    Hsva::new((time * 0.025).fract(), 0.62, 1.0, 1.0).into()
}

/// Verde quando está tranquilo, amarelo no meio, vermelho no limite.
fn level_color(level: f32) -> Color32 {
    let heat = smoothstep((level - 0.45) / 0.45);
    Hsva::new(0.38 * (1.0 - heat), 0.6, 1.0, 1.0).into()
}

/// Converte °C em 0..1 para o mostrador (30 °C = vazio, 95 °C = cheio).
fn temp_fraction(celsius: f32) -> f32 {
    ((celsius - 30.0) / 65.0).clamp(0.0, 1.0)
}

/// Cada app ganha sempre a mesma cor, derivada do nome.
fn name_color(name: &str) -> Color32 {
    let hash = name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    Hsva::new((hash % 360) as f32 / 360.0, 0.55, 0.95, 1.0).into()
}

fn brighten(color: Color32) -> Color32 {
    let mix = |c: u8| (c as f32 + (255.0 - c as f32) * 0.45) as u8;
    Color32::from_rgb(mix(color.r()), mix(color.g()), mix(color.b()))
}

fn fade(color: Color32, alpha: f32) -> Color32 {
    color.gamma_multiply(alpha.clamp(0.0, 1.0))
}
