//! A ilha: estados, animações e desenho.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, ecolor::Hsva, epaint::Shadow, pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect,
    Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use crate::alarm::{self, Alarm};
use crate::anim::{meter, smoothstep, Spring};
use crate::backend::{Backend, Command, Device, FpsAccess, Game, Snapshot, System, TempSource};
use crate::platform;
use crate::updater;

/// Tamanho da janela invisível que contém a ilha.
pub const WINDOW_SIZE: [f32; 2] = [940.0, 640.0];

const TOP_MARGIN: f32 = 10.0;
const PILL_HEIGHT: f32 = 34.0;
const HUD_SIZE: Vec2 = vec2(300.0, 42.0);
const EXPANDED_WIDTH: f32 = 420.0;
const PADDING: f32 = 18.0;
const HEADER_HEIGHT: f32 = 52.0;
const ROW_HEIGHT: f32 = 38.0;
const TILE_HEIGHT: f32 = 46.0;
const TILE_GAP: f32 = 8.0;
/// Duas fileiras de mostradores + respiro.
const STATS_HEIGHT: f32 = 2.0 * TILE_HEIGHT + TILE_GAP + 12.0;
const FOOTER_HEIGHT: f32 = 30.0;
/// Linha do painel que oferece ativar o FPS (só aparece sem a permissão).
const FPS_ROW_HEIGHT: f32 = 36.0;
/// Pílula do modo jogo: um pouco menor que a principal.
const GAME_PILL_HEIGHT: f32 = 30.0;
const GAME_PILL_GAP: f32 = 8.0;
const WELCOME_SIZE: Vec2 = vec2(380.0, 166.0);
const MAX_APPS: usize = 7;
/// Fileira de botões para escolher a saída de som ou o microfone.
const DEVICE_ROW_HEIGHT: f32 = 32.0;
const DEVICES_PER_ROW: usize = 4;
/// Mais que isso não cabe (2 fileiras de cada tipo).
const MAX_DEVICES: usize = 2 * DEVICES_PER_ROW;
/// Altura máxima do painel dentro da janela (sobra espaço para a sombra).
const PANEL_MAX_HEIGHT: f32 = WINDOW_SIZE[1] - TOP_MARGIN - 16.0;
/// Página do alarme no carrossel do painel.
const ALARM_PAGE_HEIGHT: f32 = 178.0;
/// Cartão que aparece quando o alarme toca.
const RINGING_SIZE: Vec2 = vec2(360.0, 76.0);
/// Depois de um clique num dispositivo, quanto tempo esperar o Windows confirmar.
const DEVICE_PENDING: Duration = Duration::from_secs(2);
const BARS: usize = 5;

const HUD_SECONDS: f32 = 1.6;
const LEAVE_SECONDS: f32 = 0.45;

// --- Ritmo de desenho ---
// Desenhar custa CPU e placa de vídeo, então a ilha só desenha quando há algo
// novo para mostrar:
//  - em uso (animação de abrir/fechar, painel aberto): na taxa do monitor;
//  - só dançando com o som: 30 quadros por segundo;
//  - parada: dorme, e é acordada quando um número muda, a música troca
//    (veja `backend::Wake`) ou o mouse chega perto (`platform::watch_cursor`).

/// Intervalo entre quadros enquanto a ilha só acompanha o som. Um pouco
/// menos que 1/30 s, para não perder a vez em monitores de 60 Hz.
const AMBIENT_FRAME: Duration = Duration::from_millis(30);
/// Parada, a ilha confere se está tudo certo de tempos em tempos, por garantia.
const REST_FRAME: Duration = Duration::from_secs(2);
/// Passo máximo de uma animação (um quadro atrasado não vira um salto).
const MAX_DT: f32 = 0.05;
/// Nível a partir do qual consideramos que há som tocando.
const SOUND: f32 = 0.005;
/// Tempo sem uso até devolver ao Windows a memória que ficou parada.
const TRIM_AFTER: Duration = Duration::from_secs(30);

const ISLAND: Color32 = Color32::from_rgb(9, 9, 11);
const TEXT: Color32 = Color32::from_rgb(245, 245, 247);
const TEXT_DIM: Color32 = Color32::from_rgb(150, 150, 158);
const MASTER_KEY: &str = "\0master";

/// Número do alarme sendo digitado.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Hours,
    Minutes,
}

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
    /// O alarme está tocando.
    Ringing,
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
    /// 0 = sem jogo, 1 = pílula do modo jogo aberta.
    game: Spring,
    game_width: Spring,
    /// Último jogo visto (continua desenhado enquanto a pílula fecha).
    last_game: Option<Game>,

    hud_timer: f32,
    leave_timer: f32,
    uptime: f32,
    last_master: Option<(f32, bool)>,

    /// Valores suavizados para os medidores de áudio.
    master_peak: f32,
    app_peaks: HashMap<String, f32>,
    bars: [f32; BARS],
    /// CPU, GPU, RAM e temperatura suavizados (0..1) para os mostradores.
    gauges: [f32; 4],

    /// Enquanto arrasta um slider, mostramos o valor do mouse (sem atraso).
    dragging: Option<(String, f32)>,
    alarm: Alarm,
    /// Página do painel: 0 = Som, 1 = Alarme (valores no meio = deslizando).
    page: Spring,
    /// Arrastando as páginas: onde o arrasto começou.
    page_drag: Option<f32>,
    /// Rodinha horizontal acumulada (troca de página ao passar de um tanto).
    wheel: f32,
    /// 0..1, opacidade do cartão do alarme tocando.
    ring: Spring,
    /// Número do alarme sendo digitado, e o que já foi digitado.
    editing: Option<(Field, String)>,
    /// Rodinha vertical acumulada sobre um número do alarme.
    digit_wheel: f32,
    /// Janela que tinha o teclado antes de a ilha pegá-lo para a digitação.
    keyboard_from: Option<isize>,
    /// A ilha já recebeu o foco desde que pegou o teclado.
    had_focus: bool,
    /// Pedidos para pegar / devolver o teclado (atendidos em `ui`, que tem o `frame`).
    want_keyboard: bool,
    release_keyboard: bool,
    /// Ctrl com o mouse em cima: a ilha saiu da frente até o mouse sair.
    ghost: bool,
    /// 0..1, quanto a ilha já sumiu.
    ghost_fade: Spring,
    /// Dispositivo clicado que o Windows ainda não confirmou. O destaque já
    /// vai para ele, para o clique responder na hora.
    pending_device: Option<(String, Instant)>,

    window_pos: Option<Pos2>,
    /// Segundos restantes do aviso "Atualizado" na pílula.
    notice_timer: f32,
    restarting: bool,
    /// Espera mínima entre dois deslocamentos do cursor (veja `ui`).
    nudge_cooldown: f32,
    /// Próximo deslocamento, em pixels: alterna entre +1 e -1.
    nudge_step: i32,
    debug_ui: bool,
    debug_last: String,
    perf: crate::perf::Perf,

    /// Momento do quadro anterior.
    last_frame: Option<Instant>,
    /// O quadro anterior terminou com a ilha parada (dormindo).
    resting: bool,
    /// Último aviso mandado ao backend sobre o painel estar aberto.
    panel_open: bool,
    /// Última vez em que a ilha esteve em uso.
    last_busy: Instant,
    /// A memória parada já foi devolvida desde o último uso.
    trimmed: bool,
    /// Variáveis só para testes: estado fixo e fundo cinza.
    pinned: bool,
    debug_bg: bool,
}

impl IslandApp {
    pub fn new(cc: &eframe::CreationContext<'_>, show_welcome: bool, updated: bool) -> Self {
        let mode = match std::env::var("ISLANDBAR_MODE").as_deref() {
            Ok("collapsed") => Mode::Collapsed,
            _ if show_welcome => Mode::Welcome,
            Ok("expanded") => Mode::Expanded,
            Ok("hud") => Mode::Hud,
            _ => Mode::Collapsed,
        };
        // Parada, a interface dorme. Quem a acorda é o backend (algo novo para
        // desenhar) ou o vigia do mouse (o cursor chegou na ilha).
        // (Com um atraso mínimo ela desenha um quadro só; um pedido "para
        // já" sempre rende dois.)
        let wake = |ctx: egui::Context| move || ctx.request_repaint_after(Duration::from_millis(1));
        platform::watch_cursor(wake(cc.egui_ctx.clone()));
        Self {
            backend: Backend::spawn(wake(cc.egui_ctx.clone())),
            mode,
            // Começa minúscula para "nascer" com um quique.
            width: Spring::bouncy(30.0),
            height: Spring::bouncy(10.0),
            open: Spring::smooth(0.0),
            hud: Spring::smooth(0.0),
            welcome: Spring::smooth(0.0),
            autostart: platform::autostart_enabled(),
            hover: Spring::smooth(0.0),
            game: Spring::bouncy(0.0),
            game_width: Spring::bouncy(120.0),
            last_game: None,
            hud_timer: if mode == Mode::Hud { f32::INFINITY } else { 0.0 },
            leave_timer: 0.0,
            uptime: 0.0,
            last_master: None,
            master_peak: 0.0,
            app_peaks: HashMap::new(),
            bars: [0.0; BARS],
            gauges: [0.0; 4],
            dragging: None,
            pending_device: None,
            alarm: Alarm::from_env(),
            page: Spring::smooth(0.0),
            page_drag: None,
            wheel: 0.0,
            ring: Spring::smooth(0.0),
            editing: None,
            digit_wheel: 0.0,
            keyboard_from: None,
            had_focus: false,
            want_keyboard: false,
            release_keyboard: false,
            ghost: false,
            ghost_fade: Spring::smooth(0.0),
            window_pos: None,
            notice_timer: if updated { 6.0 } else { 0.0 },
            restarting: false,
            nudge_cooldown: 0.0,
            nudge_step: 1,
            debug_ui: std::env::var_os("ISLANDBAR_DEBUG_UI").is_some(),
            debug_last: String::new(),
            perf: crate::perf::Perf::new(),
            last_frame: None,
            resting: false,
            panel_open: false,
            last_busy: Instant::now(),
            trimmed: false,
            pinned: std::env::var_os("ISLANDBAR_MODE").is_some(),
            debug_bg: std::env::var_os("ISLANDBAR_DEBUG_BG").is_some(),
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
            if let Some(inner) = inner {
                // Publica a área da ilha em pixels da tela. O vigia do mouse
                // confere a mesma área para acordar a interface na hora certa.
                let scale = ctx.pixels_per_point();
                let screen = hit.translate(inner.min.to_vec2()) * scale;
                platform::set_hover_area(screen.left(), screen.top(), screen.right(), screen.bottom());
                return platform::cursor_over_island();
            }
        }

        ctx.pointer_latest_pos().is_some_and(|p| hit.contains(p))
    }

    fn target_size(&self, snap: &Snapshot) -> Vec2 {
        match self.mode {
            Mode::Welcome => WELCOME_SIZE,
            Mode::Collapsed => {
                // Sem música a pílula mostra o uso do PC; com GPU precisa de mais espaço.
                // (No modo jogo esses números vão para a pílula ao lado.)
                let stats = snap.media.is_none() && snap.game.is_none() && snap.system.gpu.is_some();
                let mut width = if stats { 300.0 } else { 250.0 };
                if self.alarm.counting() {
                    width += 56.0; // sininho + tempo restante
                }
                vec2(width, PILL_HEIGHT) + vec2(14.0, 4.0) * self.hover.target
            }
            Mode::Hud => HUD_SIZE,
            Mode::Expanded => vec2(EXPANDED_WIDTH, panel_fixed_height(snap) + self.middle_height(snap)),
            Mode::Ringing => RINGING_SIZE,
        }
    }

    /// Altura da parte do painel que desliza, conforme a página aparecendo.
    fn middle_height(&self, snap: &Snapshot) -> f32 {
        let t = self.page.value.clamp(0.0, 1.0);
        sound_page_height(snap) * (1.0 - t) + ALARM_PAGE_HEIGHT * t
    }

    /// `level` é o som que a ilha deve acompanhar agora (0 = ficar quieta).
    fn update_meters(&mut self, snap: &Snapshot, level: f32, dt: f32, time: f32) {
        self.master_peak = meter(self.master_peak, level, dt);
        for s in &snap.sessions {
            let peak = self.app_peaks.entry(s.name.clone()).or_default();
            *peak = meter(*peak, if s.muted { 0.0 } else { s.peak }, dt);
        }
        self.app_peaks.retain(|name, _| snap.sessions.iter().any(|s| &s.name == name));

        let sys = &snap.system;
        let targets = [
            sys.cpu,
            sys.gpu.unwrap_or(0.0),
            sys.ram(),
            sys.temp.map_or(0.0, |t| temp_fraction(t.celsius)),
        ];
        for (gauge, target) in self.gauges.iter_mut().zip(targets) {
            *gauge += (target - *gauge) * (1.0 - (-5.0 * dt).exp());
        }

        // Barrinhas: cada uma dança em um ritmo, todas seguem o volume real.
        let energy = (level * 2.2).min(1.0);
        for (i, bar) in self.bars.iter_mut().enumerate() {
            let k = i as f32;
            let wobble = 0.35 + 0.65 * (time * (7.0 + k * 2.3) + k * 1.9).sin().abs();
            *bar = meter(*bar, energy * wobble, dt);
        }
    }
}

impl eframe::App for IslandApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Só para testes: fundo cinza em vez de transparente.
        if self.debug_bg {
            return [0.45, 0.47, 0.52, 1.0];
        }
        [0.0; 4]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        crate::backend::shutdown();
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time as f32);
        // Tempo desde o quadro anterior. Para os relógios vale o tempo real;
        // para as animações, o tempo em que a ilha dormiu não conta (senão
        // elas dariam um salto ao acordar).
        let now = Instant::now();
        let elapsed = self.last_frame.map_or(1.0 / 60.0, |last| (now - last).as_secs_f32());
        let dt = if self.resting { 1.0 / 60.0 } else { elapsed.min(MAX_DT) };
        self.last_frame = Some(now);
        self.uptime += elapsed;
        self.notice_timer = (self.notice_timer - elapsed).max(0.0);

        // Uma ilha mais nova abriu (ex.: depois de atualizar): esta sai de cena.
        if platform::quit_requested() {
            crate::backend::shutdown();
            std::process::exit(0);
        }
        let snap = self.backend.snapshot();
        // Atualização baixada: reinicia quando a ilha não está em uso. Durante
        // um jogo espera ele fechar, para a troca não atrapalhar a partida.
        // (Com um alarme marcado, também espera: reiniciar o perderia.)
        if updater::ready()
            && self.mode == Mode::Collapsed
            && snap.game.is_none()
            && !self.alarm.busy()
            && !self.restarting
        {
            self.restarting = true;
            updater::restart();
        }

        // Deu a hora do alarme: a ilha vira o cartão do alarme (o som já começou).
        if self.alarm.check(now) {
            self.mode = Mode::Ringing;
        }

        self.place_window(&ctx);
        // No modo jogo a ilha fica quieta: não dança com o som e só é
        // redesenhada quando os números mudam. Abrir o painel volta ao normal.
        let quiet = snap.game.is_some() && self.mode != Mode::Expanded;
        let level = if quiet { 0.0 } else { snap.master_peak };
        self.update_meters(&snap, level, dt, time);

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

        // --- Mouse e troca de estados ---
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        let hovered = self.cursor_inside(&ctx, island);
        // Ctrl com o mouse em cima: a ilha sai da frente (some e deixa os
        // cliques passarem) até o mouse sair de cima. Dá para soltar o Ctrl e
        // clicar normalmente no que estava atrás.
        let can_ghost = matches!(self.mode, Mode::Collapsed | Mode::Hud | Mode::Expanded) && self.editing.is_none();
        if hovered && can_ghost && !self.ghost && platform::ctrl_over_island() {
            self.ghost = true;
            platform::set_ghost(true);
        } else if self.ghost && !hovered {
            self.ghost = false;
            platform::set_ghost(false);
        }
        let hovered = hovered && !self.ghost;
        // Fora da ilha, os cliques atravessam a janela e vão para o app de baixo.
        platform::sync_window(frame, (!hovered && !pointer_down) || self.ghost);

        // Enquanto os cliques atravessam a janela, ela não recebe a posição do
        // mouse. Se o cursor já está sobre a ilha mas a interface ainda não
        // sabe onde ele está, um clique seria ignorado. Então deslocamos o
        // cursor 1 pixel (alternando o lado, para ele não sair do lugar): o
        // Windows avisa a janela da posição nova e o clique passa a valer.
        self.nudge_cooldown = (self.nudge_cooldown - dt).max(0.0);
        let pointer_known = ctx.pointer_latest_pos().is_some_and(|p| island.expand(8.0).contains(p));
        if cfg!(windows) && hovered && !pointer_known && self.nudge_cooldown == 0.0 {
            self.nudge_step = -self.nudge_step;
            platform::nudge_cursor(self.nudge_step);
            self.nudge_cooldown = 0.08;
        }

        // Diagnóstico (ISLANDBAR_DEBUG_UI=1): registra quando o estado do mouse muda.
        if self.debug_ui {
            let state = format!(
                "mode={:?} hovered={hovered} down={pointer_down} pointer_known={pointer_known}",
                self.mode
            );
            if state != self.debug_last {
                crate::perf::line(format!("ui: {state}"));
                self.debug_last = state;
            }
        }

        match self.mode {
            Mode::Expanded => {
                // Digitando o alarme, o painel não fecha se o mouse sair.
                if hovered || pointer_down || self.editing.is_some() {
                    self.leave_timer = 0.0;
                } else {
                    self.leave_timer += dt;
                    if self.leave_timer > LEAVE_SECONDS && !self.pinned {
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
            Mode::Collapsed | Mode::Welcome | Mode::Ringing => {}
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

        // Avisa o backend quando o painel abre ou fecha: com ele fechado, o
        // backend não lê o que só o painel mostra.
        let panel_open = self.mode == Mode::Expanded;
        if panel_open != self.panel_open {
            self.panel_open = panel_open;
            self.backend.send(Command::Panel(panel_open));
        }

        self.open.target = if self.mode == Mode::Expanded { 1.0 } else { 0.0 };
        self.hud.target = if self.mode == Mode::Hud { 1.0 } else { 0.0 };
        self.hover.target = if hovered { 1.0 } else { 0.0 };
        self.open.update(dt);
        self.welcome.target = if self.mode == Mode::Welcome { 1.0 } else { 0.0 };
        self.welcome.update(dt);
        self.ring.target = if self.mode == Mode::Ringing { 1.0 } else { 0.0 };
        self.ring.update(dt);
        self.page.update(dt);
        self.ghost_fade.target = if self.ghost { 1.0 } else { 0.0 };
        self.ghost_fade.update(dt);

        // Digitação do alarme: termina se o painel fechou, saiu da página, ou
        // se você clicou em outro programa (ele ficou com o teclado).
        if self.keyboard_from.is_some() {
            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
            if focused {
                self.had_focus = true;
            } else if self.had_focus {
                self.keyboard_from = None;
                self.end_editing(true);
            }
        }
        if self.editing.is_some() {
            if self.mode != Mode::Expanded || self.page.target != 1.0 || self.alarm.counting() {
                self.end_editing(true);
            } else {
                self.handle_typing(&ctx);
            }
        }
        self.hud.update(dt);
        self.hover.update(dt);

        // --- Desenho ---
        let accent = accent_color(time);
        let mut painter = ui.painter().clone();
        // Saindo da frente (Ctrl): tudo vai sumindo junto.
        painter.set_opacity(1.0 - smoothstep(self.ghost_fade.value));

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

        self.draw_game_pill(&painter, island, &snap, accent, dt);

        let content = painter.with_clip_rect(island.shrink(1.0));
        let open = smoothstep(self.open.value);
        let hud = smoothstep(self.hud.value);

        let welcome = smoothstep(self.welcome.value);
        let ring = smoothstep(self.ring.value);
        let pill_alpha = (1.0 - open * 2.0).clamp(0.0, 1.0)
            * (1.0 - hud)
            * (1.0 - welcome * 2.0).max(0.0)
            * (1.0 - ring * 2.0).max(0.0);
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
        if ring > 0.01 {
            self.draw_ringing(ui, &content, island, accent, ring, time);
        }

        // --- Quando redesenhar ---
        let springs = [
            &self.width, &self.height, &self.open, &self.hud, &self.hover, &self.welcome, &self.game, &self.game_width,
            &self.ring, &self.page, &self.ghost_fade,
        ];
        let animating = springs.iter().any(|s| !s.settled());
        // O cartão do alarme tocando não precisa da taxa do monitor (veja abaixo).
        let busy = animating || pointer_down || !matches!(self.mode, Mode::Collapsed | Mode::Ringing);
        // Enquanto o som toca, o sininho balança.
        let swinging = self.mode == Mode::Ringing && self.alarm.sounding();
        // As barrinhas ainda estão se mexendo (ou vão começar)?
        let dancing = level > SOUND || self.master_peak > SOUND || self.bars.iter().any(|bar| *bar > 0.01);

        let mode = self.mode;
        self.perf.frame(|| {
            format!("mode={mode:?} hovered={hovered} game={} sound={}", snap.game.is_some(), snap.master_peak > SOUND)
        });

        self.resting = false;
        if busy {
            ctx.request_repaint();
        } else if hovered || dancing || swinging {
            ctx.request_repaint_after(AMBIENT_FRAME);
        } else {
            // Nada se mexendo: dorme até alguém avisar que algo mudou.
            self.resting = true;
            let mut wait = REST_FRAME;
            if self.notice_timer > 0.0 {
                wait = wait.min(Duration::from_secs_f32(self.notice_timer));
            }
            // Acorda quando o tempo restante na pílula muda e no segundo em
            // que o alarme deve tocar.
            if let Some(left) = self.alarm.remaining(now) {
                wait = wait.min(alarm::next_change(left));
            }
            ctx.request_repaint_after(wait);
        }

        // Pegar/devolver o teclado para a digitação do alarme.
        if std::mem::take(&mut self.want_keyboard) && self.keyboard_from.is_none() {
            self.had_focus = false;
            self.keyboard_from = platform::take_keyboard(frame);
        }
        if std::mem::take(&mut self.release_keyboard) {
            if let Some(previous) = self.keyboard_from.take() {
                platform::give_back_keyboard(frame, previous);
            }
        }

        // Um tempo depois do último uso, devolve ao Windows a memória parada.
        if busy || hovered {
            self.last_busy = now;
            self.trimmed = false;
        } else if !self.trimmed && now - self.last_busy >= TRIM_AFTER {
            self.trimmed = true;
            platform::trim_memory();
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
        let mut clock_rect = p.text(
            pos2(right, cy),
            Align2::RIGHT_CENTER,
            clock,
            FontId::proportional(13.5),
            fade(TEXT, alpha),
        );
        // Alarme marcado: sininho e quanto falta, antes do relógio. (O resto
        // da pílula se ajeita no espaço que sobra à esquerda dele.)
        if let Some(left_time) = self.alarm.remaining(Instant::now()) {
            let text = p.text(
                pos2(clock_rect.left() - 10.0, cy),
                Align2::RIGHT_CENTER,
                alarm::short(left_time),
                FontId::proportional(12.5),
                fade(accent, alpha),
            );
            draw_bell(p, pos2(text.left() - 10.0, cy), 6.5, 0.0, fade(accent, alpha));
            clock_rect.min.x = text.left() - 18.0;
        }

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
        } else if let Some(game) = &snap.game {
            // Jogando sem música: o nome do jogo (os números estão na pílula ao lado).
            let x = left + 32.0;
            text_fit(p, pos2(x, cy), Align2::LEFT_CENTER, &game.name, 12.5, fade(TEXT_DIM, alpha), clock_rect.left() - 10.0 - x);
        } else {
            // Sem música: mostra o uso do PC.
            let sys = &snap.system;
            let usage = match sys.gpu {
                Some(gpu) => format!(
                    "CPU {:.0}%  GPU {:.0}%  RAM {:.0}%",
                    sys.cpu * 100.0,
                    gpu * 100.0,
                    sys.ram() * 100.0
                ),
                None => format!("CPU {:.0}%  RAM {:.0}%", sys.cpu * 100.0, sys.ram() * 100.0),
            };
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

        // ---- Meio do painel: páginas que deslizam (Som | Alarme) ----
        let page_top = line_y + 7.0;
        let middle_bottom = page_top + self.middle_height(snap);
        let clip = Rect::from_min_max(pos2(island.left() + 1.0, line_y + 1.0), pos2(island.right() - 1.0, middle_bottom + 4.0))
            .intersect(p.clip_rect());
        if interactive {
            self.swipe_pages(ui, clip, island.width());
        }
        // Os botões só valem na página que está parada na tela.
        let still = self.page_drag.is_none() && self.page.settled();
        let shift = island.width();

        // ---- Página Som ----
        let sound_dx = -self.page.value * shift;
        if self.page.value < 0.999 {
            let mut y = page_top;
            let p = &p.with_clip_rect(clip);
            let (left, right) = (left + sound_dx, right + sound_dx);
            let interactive = interactive && still && self.page.target == 0.0;

            // ---- Saída de som e microfone ----
            if self.pending_device.as_ref().is_some_and(|(id, at)| {
                at.elapsed() > DEVICE_PENDING || snap.outputs.iter().chain(&snap.inputs).any(|d| d.default && d.id == *id)
            }) {
                self.pending_device = None;
            }
            // As duas fileiras usam a mesma largura de botão, para ficarem alinhadas.
            let columns = snap.outputs.len().max(snap.inputs.len()).clamp(1, DEVICES_PER_ROW);
            for (devices, is_output) in [(&snap.outputs, true), (&snap.inputs, false)] {
                let shown = &devices[..devices.len().min(MAX_DEVICES)];
                if shown.is_empty() {
                    continue;
                }
                let names = short_names(shown);
                // O destaque vai para o clicado assim que ele for clicado.
                let pending = self.pending_device.as_ref().map(|(id, _)| id.clone()).filter(|id| shown.iter().any(|d| d.id == *id));
                let gap = 6.0;
                let chips_left = left + 38.0;
                let chip_width = (right - chips_left - gap * (columns - 1) as f32) / columns as f32;
                for (line, chunk) in shown.chunks(DEVICES_PER_ROW).enumerate() {
                    let a = next_alpha(open);
                    let cy = y + DEVICE_ROW_HEIGHT / 2.0 + (1.0 - a) * 12.0;
                    y += DEVICE_ROW_HEIGHT;
                    if line == 0 {
                        let icon = pos2(left + 14.0, cy);
                        if is_output {
                            draw_speaker(p, icon, 8.5, 0.66, false, fade(TEXT_DIM, a));
                        } else {
                            draw_mic(p, icon, 8.0, fade(TEXT_DIM, a));
                        }
                    }
                    for (column, device) in chunk.iter().enumerate() {
                        let index = line * DEVICES_PER_ROW + column;
                        let x = chips_left + column as f32 * (chip_width + gap);
                        let chip = Rect::from_min_size(pos2(x, cy - 12.0), vec2(chip_width, 24.0));
                        let current = pending.as_ref().map_or(device.default, |id| *id == device.id);
                        let clicked = self.draw_device_chip(ui, p, chip, device, &names[index], current, accent, a, interactive);
                        if clicked && !current {
                            self.backend.send(Command::SetDevice(device.id.clone()));
                            self.pending_device = Some((device.id.clone(), Instant::now()));
                        }
                    }
                }
            }

            // ---- Mixer ----
            let mut rows: Vec<(String, String, f32, bool, f32)> = vec![(
                MASTER_KEY.to_string(),
                "Geral".to_string(),
                snap.master,
                snap.master_muted,
                self.master_peak,
            )];
            for s in snap.sessions.iter().take(apps_shown(snap)) {
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
        }

        // ---- Página Alarme ----
        if self.page.value > 0.001 {
            let alarm_dx = (1.0 - self.page.value) * shift;
            let p = &p.with_clip_rect(clip);
            let a = smoothstep(open * 1.9 - 0.25 - 0.09);
            let interactive = interactive && still && self.page.target == 1.0;
            self.draw_alarm_page(ui, p, left + alarm_dx, right + alarm_dx, page_top, accent, a, interactive);
        }
        let y = middle_bottom;

        // ---- Monitor do sistema ----
        let a = next_alpha(open);
        self.draw_stats(p, pos2(left, y + 6.0 + (1.0 - a) * 12.0), right - left, &snap.system, a);

        // ---- Permissão de FPS (só quando falta) ----
        if snap.fps_access != FpsAccess::Granted {
            let a = next_alpha(open);
            let row = Rect::from_min_size(pos2(left, y + STATS_HEIGHT), vec2(right - left, FPS_ROW_HEIGHT));
            self.draw_fps_row(ui, p, row, snap.fps_access, accent, a, interactive);
        }

        // ---- Rodapé ----
        let a = next_alpha(open);
        let footer_y = island.bottom() - FOOTER_HEIGHT / 2.0 - 3.0;
        let mut clock = chrono::Local::now().format("%H:%M").to_string();
        if let Some(build) = updater::BUILD {
            clock += &format!("  ·  versão {build}");
        }
        p.text(pos2(left, footer_y), Align2::LEFT_CENTER, clock, FontId::proportional(12.0), fade(TEXT_DIM, a));

        // Bolinhas das páginas (clique leva direto à página).
        for (index, page) in [0.0f32, 1.0].into_iter().enumerate() {
            let center = pos2(island.center().x - 8.0 + index as f32 * 16.0, footer_y);
            let closeness = 1.0 - (self.page.value - page).abs().min(1.0);
            let color = fade(Color32::WHITE, 0.25).lerp_to_gamma(accent, closeness);
            p.circle_filled(center, 3.0 + closeness, fade(color, a));
            if interactive {
                let target = Rect::from_center_size(center, vec2(16.0, 22.0));
                let resp = ui.interact(target, ui.scope_id().with(("page", index)), Sense::click());
                if resp.clicked() {
                    self.page.target = page;
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        }

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
    /// Arrastar (ou rodinha horizontal) troca a página do painel.
    ///
    /// Esta área é registrada antes dos botões e sliders da página, então eles
    /// têm prioridade: arrastar um slider mexe no volume, não troca de página.
    /// Ela também "ouve" cliques: assim a biblioteca espera o mouse se mexer
    /// antes de decidir que é um arrasto, e um clique parado vai para o botão
    /// que está em cima (só arrasto faria qualquer apertar virar arrasto).
    fn swipe_pages(&mut self, ui: &mut Ui, area: Rect, width: f32) {
        let resp = ui.interact(area, ui.scope_id().with("pages"), Sense::click_and_drag());
        if resp.drag_started() {
            self.page_drag = Some(self.page.value);
        }
        if let Some(start) = self.page_drag {
            if resp.dragged() {
                // A página acompanha o mouse; além das pontas, resiste.
                let mut value = self.page.value - resp.drag_delta().x / width;
                if !(0.0..=1.0).contains(&value) {
                    value = value.clamp(-0.12, 1.12);
                }
                self.page.value = value;
                self.page.target = value;
            }
            if resp.drag_stopped() || !resp.dragged() {
                self.page_drag = None;
                let moved = self.page.value - start;
                let speed = ui.ctx().input(|i| i.pointer.velocity().x);
                let target = if moved > 0.25 || speed < -400.0 {
                    1.0
                } else if moved < -0.25 || speed > 400.0 {
                    0.0
                } else {
                    start.round()
                };
                self.page.target = target.clamp(0.0, 1.0);
            }
        }

        // Rodinha horizontal / dois dedos no touchpad.
        if resp.hovered() || ui.rect_contains_pointer(area) {
            let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.x);
            self.wheel = if scroll == 0.0 { self.wheel * 0.9 } else { self.wheel + scroll };
            if self.wheel.abs() > 60.0 {
                self.page.target = if self.wheel < 0.0 { 1.0 } else { 0.0 };
                self.wheel = 0.0;
            }
        }
    }

    /// Página do alarme: escolher o tempo, ou ver quanto falta.
    #[allow(clippy::too_many_arguments)]
    fn draw_alarm_page(&mut self, ui: &mut Ui, p: &Painter, left: f32, right: f32, top: f32, accent: Color32, a: f32, interactive: bool) {
        let cx = (left + right) / 2.0;
        let now = Instant::now();
        if let Some(left_time) = self.alarm.remaining(now) {
            // ---- Contando ----
            p.text(pos2(cx, top + 52.0), Align2::CENTER_CENTER, alarm::countdown(left_time), FontId::proportional(38.0), fade(TEXT, a));
            let at = self.alarm.due_clock().unwrap_or_default();
            p.text(pos2(cx, top + 92.0), Align2::CENTER_CENTER, format!("Toca às {at}"), FontId::proportional(13.0), fade(TEXT_DIM, a));
            let row = top + 138.0;
            let more = Rect::from_center_size(pos2(cx - 62.0, row), vec2(112.0, 30.0));
            let cancel = Rect::from_center_size(pos2(cx + 62.0, row), vec2(112.0, 30.0));
            if text_button(ui, p, "alarm-more", more, "+5 min", false, accent, a, interactive) {
                self.alarm.add(5);
            }
            if text_button(ui, p, "alarm-cancel", cancel, "Cancelar", false, accent, a, interactive) {
                self.alarm.cancel();
            }
            return;
        }

        // ---- Escolhendo o tempo ----
        let (hours, minutes) = (self.alarm.chosen / 60, self.alarm.chosen % 60);
        let digits_y = top + 50.0;
        let digit_rect = |column: usize| {
            let x = cx + if column == 0 { -62.0 } else { 52.0 };
            Rect::from_center_size(pos2(x, digits_y), vec2(62.0, 46.0))
        };
        // Apertou o mouse fora dos números: o que foi digitado vale.
        let pressed_outside = ui.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_none_or(|pos| !digit_rect(0).contains(pos) && !digit_rect(1).contains(pos))
        });
        if self.editing.is_some() && pressed_outside {
            self.end_editing(true);
        }
        let time = ui.input(|i| i.time);
        let fields = [(Field::Hours, hours, "h", 60), (Field::Minutes, minutes, "min", 5)];
        for (column, (field, value, unit, step)) in fields.into_iter().enumerate() {
            let x = cx + if column == 0 { -62.0 } else { 52.0 };
            let rect = digit_rect(column);

            // Clicar no número começa a digitar (a ilha pega o teclado).
            if interactive {
                let resp = ui.interact(rect, ui.scope_id().with(("alarm-digits", column)), Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                    // Rodinha em cima do número: soma ou tira 1.
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    self.digit_wheel += scroll;
                    while self.digit_wheel.abs() >= 40.0 {
                        let up = self.digit_wheel > 0.0;
                        self.digit_wheel -= 40.0f32.copysign(self.digit_wheel);
                        self.end_editing(true);
                        self.alarm.change(if up { 1 } else { -1 } * if field == Field::Hours { 60 } else { 1 });
                    }
                }
                if resp.clicked() && self.editing.as_ref().is_none_or(|(f, _)| *f != field) {
                    self.end_editing(true);
                    self.editing = Some((field, String::new()));
                    self.want_keyboard = true;
                }
            }

            let typing = self.editing.as_ref().filter(|(f, _)| *f == field).map(|(_, typed)| typed.clone());
            let (text, color) = match &typing {
                Some(typed) if !typed.is_empty() => (typed.clone(), TEXT),
                Some(_) => (format!("{value:02}"), TEXT_DIM),
                None => (format!("{value:02}"), TEXT),
            };
            if typing.is_some() {
                p.rect_filled(rect, 12.0, fade(accent, a * 0.12));
                p.rect_stroke(rect, 12.0, Stroke::new(1.5, fade(accent, a * 0.8)), StrokeKind::Inside);
            }
            let shown = p.text(pos2(x, digits_y), Align2::CENTER_CENTER, text, FontId::proportional(36.0), fade(color, a));
            // Cursor piscando enquanto digita.
            if typing.is_some() && (time * 2.0) as i64 % 2 == 0 {
                let caret_x = shown.right() + 2.0;
                p.line_segment([pos2(caret_x, digits_y - 14.0), pos2(caret_x, digits_y + 14.0)], Stroke::new(2.0, fade(accent, a)));
            }
            p.text(pos2(x + 26.0, digits_y + 6.0), Align2::LEFT_CENTER, unit, FontId::proportional(13.0), fade(TEXT_DIM, a));
            for (row, direction) in [(-1.0f32, 1), (1.0, -1)] {
                let center = pos2(x, digits_y + row * 34.0);
                let (clicked, scale) = icon_button(ui, p, ("alarm-step", column, direction), center, 13.0, interactive, a);
                draw_chevron(p, center, 5.5 * scale, row < 0.0, fade(TEXT_DIM, a));
                if clicked {
                    self.end_editing(true);
                    self.alarm.change(direction * step);
                }
            }
        }
        p.text(pos2(cx - 5.0, digits_y - 2.0), Align2::CENTER_CENTER, ":", FontId::proportional(30.0), fade(TEXT_DIM, a));

        // Atalhos: somam ao tempo escolhido.
        let presets = [("+5 min", 5), ("+15 min", 15), ("+30 min", 30), ("+1 h", 60)];
        let gap = 6.0;
        let width = (right - left - gap * 3.0) / 4.0;
        for (i, (label, minutes)) in presets.into_iter().enumerate() {
            let rect = Rect::from_min_size(pos2(left + i as f32 * (width + gap), top + 100.0), vec2(width, 26.0));
            if text_button(ui, p, ("alarm-preset", i), rect, label, false, accent, a, interactive) {
                self.alarm.change(minutes);
            }
        }

        let ready = self.alarm.chosen > 0;
        let start = Rect::from_center_size(pos2(cx, top + 154.0), vec2(150.0, 30.0));
        let a_start = if ready { a } else { a * 0.4 };
        if text_button(ui, p, "alarm-start", start, "Iniciar", true, accent, a_start, interactive && ready) {
            self.alarm.start();
        }
    }

    /// Teclas enquanto um número do alarme está sendo digitado.
    fn handle_typing(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for event in events {
            let Some((field, typed)) = self.editing.as_mut() else { return };
            match event {
                egui::Event::Text(text) => {
                    for digit in text.chars().filter(|c| c.is_ascii_digit()) {
                        // Dois dígitos já digitados: o próximo começa de novo.
                        if typed.len() >= 2 {
                            typed.clear();
                        }
                        typed.push(digit);
                    }
                }
                egui::Event::Key { key, pressed: true, .. } => match key {
                    egui::Key::Backspace => {
                        typed.pop();
                    }
                    egui::Key::Tab | egui::Key::ArrowRight if *field == Field::Hours => {
                        self.commit_typed();
                        self.editing = Some((Field::Minutes, String::new()));
                    }
                    egui::Key::ArrowLeft if *field == Field::Minutes => {
                        self.commit_typed();
                        self.editing = Some((Field::Hours, String::new()));
                    }
                    egui::Key::ArrowUp | egui::Key::ArrowDown => {
                        let step = if *field == Field::Hours { 60 } else { 1 };
                        let step = if key == egui::Key::ArrowUp { step } else { -step };
                        self.commit_typed();
                        self.alarm.change(step);
                    }
                    egui::Key::Enter => {
                        self.end_editing(true);
                        self.alarm.start();
                    }
                    egui::Key::Escape => self.end_editing(false),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// Passa o que foi digitado para o alarme (e limpa, para seguir digitando).
    fn commit_typed(&mut self) {
        let Some((field, typed)) = self.editing.as_mut() else { return };
        if let Ok(value) = typed.parse::<u32>() {
            let (hours, minutes) = (self.alarm.chosen / 60, self.alarm.chosen % 60);
            match field {
                Field::Hours => self.alarm.set_parts(value, minutes),
                Field::Minutes => self.alarm.set_parts(hours, value),
            }
        }
        typed.clear();
    }

    /// Termina a digitação (`commit` = vale o que foi digitado) e devolve o teclado.
    fn end_editing(&mut self, commit: bool) {
        if self.editing.is_none() {
            return;
        }
        if commit {
            self.commit_typed();
        }
        self.editing = None;
        self.release_keyboard = true;
    }

    /// Cartão do alarme tocando: sininho | "Alarme" | Soneca, Parar.
    fn draw_ringing(&mut self, ui: &mut Ui, p: &Painter, island: Rect, accent: Color32, alpha: f32, time: f32) {
        let interactive = self.mode == Mode::Ringing;
        let cy = island.center().y;
        let left = island.left() + 18.0;
        let right = island.right() - 16.0;

        let swing = if self.alarm.sounding() { (time * 22.0).sin() * 0.32 } else { 0.0 };
        draw_bell(p, pos2(left + 14.0, cy), 13.0, swing, fade(accent, alpha));

        let text_x = left + 40.0;
        p.text(pos2(text_x, cy - 9.0), Align2::LEFT_CENTER, "Alarme", FontId::proportional(15.0), fade(TEXT, alpha));
        let subtitle = match self.alarm.rang_clock() {
            Some(at) => format!("Marcado para {at}"),
            None => "Deu a hora".to_string(),
        };
        p.text(pos2(text_x, cy + 10.0), Align2::LEFT_CENTER, subtitle, FontId::proportional(12.0), fade(TEXT_DIM, alpha));

        let stop = Rect::from_min_max(pos2(right - 76.0, cy - 15.0), pos2(right, cy + 15.0));
        let snooze = Rect::from_min_max(pos2(stop.left() - 8.0 - 92.0, cy - 15.0), pos2(stop.left() - 8.0, cy + 15.0));
        let snooze_label = format!("Soneca {} min", alarm::SNOOZE_MINUTES);
        if text_button(ui, p, "ring-snooze", snooze, &snooze_label, false, accent, alpha, interactive) {
            self.alarm.snooze();
            self.mode = Mode::Collapsed;
        }
        if text_button(ui, p, "ring-stop", stop, "Parar", true, accent, alpha, interactive) {
            self.alarm.stop();
            self.mode = Mode::Collapsed;
        }
    }

    /// Um botão de dispositivo (fone, TV, microfone...). Devolve se foi clicado.
    #[allow(clippy::too_many_arguments)]
    fn draw_device_chip(
        &self,
        ui: &mut Ui,
        p: &Painter,
        chip: Rect,
        device: &Device,
        label: &str,
        current: bool,
        accent: Color32,
        a: f32,
        interactive: bool,
    ) -> bool {
        let id = ui.scope_id().with(("device", &device.id));
        let resp = interactive.then(|| ui.interact(chip, id, Sense::click()).on_hover_text(&device.name));
        let hover = ui.ctx().animate_bool_with_time(id, resp.as_ref().is_some_and(|r| r.hovered()), 0.12);
        let radius = chip.height() / 2.0;
        if current {
            p.rect_filled(chip, radius, fade(accent, a * (0.26 + 0.08 * hover)));
            p.rect_stroke(chip, radius, Stroke::new(1.0, fade(accent, a * 0.7)), StrokeKind::Inside);
        } else {
            p.rect_filled(chip, radius, fade(Color32::WHITE, a * (0.06 + 0.08 * hover)));
        }
        let mut text_x = chip.left() + 11.0;
        if current {
            p.circle_filled(pos2(chip.left() + 11.0, chip.center().y), 3.0, fade(brighten(accent), a));
            text_x += 8.0;
        }
        let color = if current || hover > 0.5 { TEXT } else { TEXT_DIM };
        let width = chip.right() - 8.0 - text_x;
        text_fit(p, pos2(text_x, chip.center().y), Align2::LEFT_CENTER, label, 11.5, fade(color, a), width);
        resp.is_some_and(|r| r.clicked())
    }

    /// Modo jogo: uma pílula pequena que "brota" do lado direito da ilha com
    /// FPS, CPU, GPU e RAM.
    fn draw_game_pill(&mut self, painter: &Painter, island: Rect, snap: &Snapshot, accent: Color32, dt: f32) {
        if snap.game.is_some() {
            self.last_game = snap.game.clone();
        }
        // Com o painel aberto a pílula some, para não disputar espaço.
        let visible = snap.game.is_some() && matches!(self.mode, Mode::Collapsed | Mode::Hud);
        self.game.target = if visible { 1.0 } else { 0.0 };
        self.game.update(dt);
        let grow = self.game.value;
        if grow <= 0.01 {
            // Escondida, a largura não é mais atualizada: não pode ficar com
            // uma animação pela metade (a ilha nunca voltaria a descansar).
            self.game_width.finish();
            return;
        }
        let Some(game) = &self.last_game else { return };

        // Monta os trechos: (rótulo, valor, valor largo típico, cor do valor).
        let sys = &snap.system;
        let mut parts: Vec<(&str, String, &str, Color32)> = Vec::new();
        if let Some(fps) = game.fps {
            parts.push(("FPS", format!("{fps:.0}"), "000", fps_color(fps)));
        }
        parts.push(("CPU", format!("{:.0}%", sys.cpu * 100.0), "00%", TEXT));
        if let Some(gpu) = sys.gpu {
            parts.push(("GPU", format!("{:.0}%", gpu * 100.0), "00%", TEXT));
        }
        parts.push(("RAM", format!("{:.0}%", sys.ram() * 100.0), "00%", TEXT));

        let label_font = FontId::proportional(10.0);
        let value_font = FontId::proportional(12.5);
        let (padding, inner_gap, part_gap) = (14.0, 4.0, 12.0);
        // Cada valor ganha pelo menos o espaço de um valor largo típico
        // ("00%"). Assim a pílula não muda de largura (nem precisa ser
        // reanimada) a cada número novo; só em casos raros, como 100%.
        let galleys: Vec<_> = parts
            .iter()
            .map(|(label, value, widest, color)| {
                let label = painter.layout_no_wrap(label.to_string(), label_font.clone(), TEXT_DIM);
                let value = painter.layout_no_wrap(value.clone(), value_font.clone(), *color);
                let widest = painter.layout_no_wrap(widest.to_string(), value_font.clone(), *color);
                let space = value.size().x.max(widest.size().x);
                (label, value, space)
            })
            .collect();
        let content_width: f32 = galleys.iter().map(|(label, _, space)| label.size().x + inner_gap + space).sum::<f32>()
            + part_gap * (galleys.len() as f32 - 1.0);
        self.game_width.target = ((content_width + 2.0 * padding) / 8.0).ceil() * 8.0;
        self.game_width.update(dt);

        let size = vec2(self.game_width.value * grow.min(1.15), GAME_PILL_HEIGHT * grow.clamp(0.0, 1.0));
        let left = island.right() + GAME_PILL_GAP * grow.min(1.0);
        let center_y = TOP_MARGIN + PILL_HEIGHT / 2.0;
        let rect = Rect::from_min_size(pos2(left, center_y - size.y / 2.0), size);
        let radius = size.y / 2.0;
        let alpha = smoothstep((grow - 0.55) / 0.45);

        painter.add(
            Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha((90.0 * grow.min(1.0)) as u8) }
                .as_shape(rect, radius),
        );
        painter.rect_filled(rect, radius, ISLAND);
        painter.rect_stroke(rect, radius, Stroke::new(1.0, fade(accent, 0.18)), StrokeKind::Inside);

        let clipped = painter.with_clip_rect(rect.shrink(1.0));
        let mut x = rect.left() + padding;
        for (label, value, space) in galleys {
            let label_pos = pos2(x, center_y - label.size().y / 2.0 + 0.5);
            x += label.size().x + inner_gap;
            let value_pos = pos2(x, center_y - value.size().y / 2.0);
            x += space + part_gap;
            clipped.galley_with_override_text_color(label_pos, label, fade(TEXT_DIM, alpha));
            let color = value.job.sections.first().map_or(TEXT, |s| s.format.color);
            clipped.galley_with_override_text_color(value_pos, value, fade(color, alpha));
        }
    }

    /// Linha do painel para liberar a medição de FPS.
    #[allow(clippy::too_many_arguments)]
    fn draw_fps_row(&mut self, ui: &mut Ui, p: &Painter, row: Rect, access: FpsAccess, accent: Color32, a: f32, interactive: bool) {
        let cy = row.center().y + (1.0 - a) * 12.0;
        if access == FpsAccess::Requested {
            let text = "Permissão pedida. Saia e entre no Windows para o FPS aparecer.";
            text_fit(p, pos2(row.left(), cy), Align2::LEFT_CENTER, text, 12.0, fade(TEXT_DIM, a), row.width());
            return;
        }

        let button = Rect::from_center_size(pos2(row.right() - 36.0, cy), vec2(72.0, 24.0));
        let text = "Mostrar FPS nos jogos exige uma permissão do Windows";
        text_fit(p, pos2(row.left(), cy), Align2::LEFT_CENTER, text, 12.0, fade(TEXT_DIM, a), button.left() - 10.0 - row.left());

        let id = ui.scope_id().with("enable-fps");
        let mut hover = false;
        if interactive {
            let resp = ui.interact(button, id, Sense::click());
            hover = resp.hovered();
            if hover {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                self.backend.send(Command::EnableFps);
            }
        }
        let lift = ui.ctx().animate_bool_with_time(id, hover, 0.12);
        p.rect_filled(button.expand(lift), 12.0, fade(accent, a * (0.8 + 0.2 * lift)));
        p.text(button.center(), Align2::CENTER_CENTER, "Ativar", FontId::proportional(12.5), fade(ISLAND, a));
    }

    /// Quatro mostradores em grade 2x2: CPU e GPU em cima, RAM e temperatura embaixo.
    fn draw_stats(&self, p: &Painter, origin: Pos2, width: f32, sys: &System, alpha: f32) {
        let tile = vec2((width - TILE_GAP) / 2.0, TILE_HEIGHT);
        // O rótulo diz de onde a temperatura veio, para ninguém confundir a
        // do disco com a do processador.
        let temp_label = match sys.temp.map(|t| t.source) {
            Some(TempSource::Gpu) => "Temp GPU",
            Some(TempSource::Disk) => "Temp SSD",
            Some(TempSource::System) | None => "Temp",
        };
        let not_available = || "n/d".to_string();
        let tiles = [
            ("CPU", format!("{:.0}%", sys.cpu * 100.0), true),
            ("GPU", sys.gpu.map_or_else(not_available, |g| format!("{:.0}%", g * 100.0)), sys.gpu.is_some()),
            ("RAM", format!("{:.1} / {:.0} GB", sys.ram_used_gb, sys.ram_total_gb), true),
            (temp_label, sys.temp.map_or_else(not_available, |t| format!("{:.0}°C", t.celsius)), sys.temp.is_some()),
        ];
        for (i, (label, value, available)) in tiles.into_iter().enumerate() {
            let (column, row) = ((i % 2) as f32, (i / 2) as f32);
            let corner = origin + vec2(column * (tile.x + TILE_GAP), row * (tile.y + TILE_GAP));
            let rect = Rect::from_min_size(corner, tile);
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

/// Altura das partes fixas do painel (música, números, rodapé).
fn panel_fixed_height(snap: &Snapshot) -> f32 {
    let fps_row = if snap.fps_access == FpsAccess::Granted { 0.0 } else { FPS_ROW_HEIGHT };
    PADDING + HEADER_HEIGHT + 14.0 + STATS_HEIGHT + fps_row + FOOTER_HEIGHT
}

/// Altura das fileiras de saída de som e microfone.
fn devices_height(snap: &Snapshot) -> f32 {
    let device_rows = |list: &[Device]| list.len().min(MAX_DEVICES).div_ceil(DEVICES_PER_ROW);
    (device_rows(&snap.outputs) + device_rows(&snap.inputs)) as f32 * DEVICE_ROW_HEIGHT
}

/// Altura da página Som: dispositivos + "Geral" + apps.
fn sound_page_height(snap: &Snapshot) -> f32 {
    devices_height(snap) + (1 + apps_shown(snap)) as f32 * ROW_HEIGHT
}

/// Quantos apps cabem no mixer (além da linha "Geral").
fn apps_shown(snap: &Snapshot) -> usize {
    let room = (PANEL_MAX_HEIGHT - panel_fixed_height(snap) - devices_height(snap)) / ROW_HEIGHT - 1.0;
    snap.sessions.len().min(MAX_APPS).min(room.max(0.0) as usize)
}

/// Botão com texto. `filled` = o botão principal (cheio, na cor de destaque).
#[allow(clippy::too_many_arguments)]
fn text_button(
    ui: &mut Ui,
    p: &Painter,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    rect: Rect,
    label: &str,
    filled: bool,
    accent: Color32,
    a: f32,
    interactive: bool,
) -> bool {
    let id = ui.scope_id().with(id_source);
    let resp = interactive.then(|| ui.interact(rect, id, Sense::click()));
    let hovered = resp.as_ref().is_some_and(|r| r.hovered());
    let hover = ui.ctx().animate_bool_with_time(id, hovered, 0.12);
    let radius = rect.height() / 2.0;
    if filled {
        p.rect_filled(rect.expand(hover * 1.5), radius, fade(accent, a * (0.85 + 0.15 * hover)));
        p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(13.5), fade(ISLAND, a));
    } else {
        p.rect_filled(rect, radius, fade(Color32::WHITE, a * (0.07 + 0.08 * hover)));
        let color = if hover > 0.5 { TEXT } else { TEXT_DIM.lerp_to_gamma(TEXT, 0.4) };
        p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.5), fade(color, a));
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.is_some_and(|r| r.clicked())
}

/// Setinha para cima (`up`) ou para baixo.
fn draw_chevron(p: &Painter, c: Pos2, r: f32, up: bool, color: Color32) {
    let dy = if up { -r * 0.5 } else { r * 0.5 };
    let points = vec![pos2(c.x - r, c.y - dy), pos2(c.x, c.y + dy), pos2(c.x + r, c.y - dy)];
    p.add(Shape::line(points, Stroke::new(2.0, color)));
}

/// Sininho. `angle` (em radianos) o inclina, pendurado pelo topo.
fn draw_bell(p: &Painter, c: Pos2, r: f32, angle: f32, color: Color32) {
    let pivot = pos2(c.x, c.y - r);
    let (sin, cos) = angle.sin_cos();
    let at = |x: f32, y: f32| {
        let (dx, dy) = (c.x + x * r - pivot.x, c.y + y * r - pivot.y);
        pos2(pivot.x + dx * cos - dy * sin, pivot.y + dx * sin + dy * cos)
    };
    let body = vec![at(-0.25, -0.8), at(0.25, -0.8), at(0.6, -0.3), at(0.75, 0.5), at(-0.75, 0.5), at(-0.6, -0.3)];
    p.add(Shape::convex_polygon(body, color, Stroke::NONE));
    p.line_segment([at(-0.9, 0.55), at(0.9, 0.55)], Stroke::new((r * 0.2).max(1.5), color));
    p.circle_filled(at(0.0, 0.82), r * 0.17, color);
    p.circle_filled(at(0.0, -0.9), r * 0.12, color);
}

/// Nomes curtos para os botões: "Fones de ouvido (Realtek Audio)" vira
/// "Fones de ouvido". Quando dois ficariam iguais (ex.: "Microfone"), usa o que
/// está entre parênteses, que é o que os diferencia.
fn short_names(devices: &[Device]) -> Vec<String> {
    let split = |name: &str| match name.split_once(" (") {
        Some((head, rest)) => (head.trim().to_string(), Some(rest.trim_end_matches(')').trim().to_string())),
        None => (name.trim().to_string(), None),
    };
    let parts: Vec<_> = devices.iter().map(|d| split(&d.name)).collect();
    parts
        .iter()
        .map(|(head, inside)| {
            let repeated = parts.iter().filter(|(other, _)| other == head).count() > 1;
            match inside {
                Some(inside) if repeated && !inside.is_empty() => inside.clone(),
                _ if head.is_empty() => "Dispositivo".to_string(),
                _ => head.clone(),
            }
        })
        .collect()
}

/// Ícone de microfone: cápsula, suporte em "U" e haste.
fn draw_mic(p: &Painter, c: Pos2, r: f32, color: Color32) {
    let capsule = Rect::from_min_max(pos2(c.x - r * 0.38, c.y - r), pos2(c.x + r * 0.38, c.y + r * 0.2));
    p.rect_filled(capsule, r * 0.38, color);
    let stroke = Stroke::new(1.6, color);
    let arc: Vec<Pos2> = (0..=10)
        .map(|i| {
            let t = std::f32::consts::PI * i as f32 / 10.0;
            pos2(c.x + r * 0.65 * t.cos(), c.y - r * 0.15 + r * 0.6 * t.sin())
        })
        .collect();
    p.add(Shape::line(arc, stroke));
    p.line_segment([pos2(c.x, c.y + r * 0.45), pos2(c.x, c.y + r * 0.85)], stroke);
    p.line_segment([pos2(c.x - r * 0.35, c.y + r * 0.85), pos2(c.x + r * 0.35, c.y + r * 0.85)], stroke);
}

// ---------------------------------------------------------------------------
// Cores
// ---------------------------------------------------------------------------

/// Cor de destaque que passeia devagar pelo arco-íris.
fn accent_color(time: f32) -> Color32 {
    Hsva::new((time * 0.025).fract(), 0.62, 1.0, 1.0).into()
}

/// FPS: verde a partir de 60, amarelo perto de 45, vermelho abaixo de 30.
fn fps_color(fps: f32) -> Color32 {
    let good = smoothstep((fps - 28.0) / 32.0);
    Hsva::new(0.38 * good, 0.6, 1.0, 1.0).into()
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
