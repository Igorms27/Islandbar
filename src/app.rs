//! A ilha: estados, animações e desenho.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, ecolor::Hsva, epaint::Shadow, pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};

use crate::alarm::{self, Alarm};
use crate::anim::{meter, smoothstep, Spring};
use crate::backend::{Backend, Command, Device, FpsAccess, Game, Snapshot, System, TempSource};
use crate::battery::{self, AlertTracker, LowBatteryAlert, PanelBatteries, Peripheral};
use crate::delivery::{self, Order, Stage};
use crate::map::{self, Map};
use crate::pet;
use crate::platform;
use crate::settings::{Pet, Settings, TempChoice};
use crate::updater;
use crate::weather::{self, Detail, Weather};

/// Tamanho da janela invisível que contém a ilha (em monitores menores que
/// isso, encolhe para caber na tela).
pub const WINDOW_SIZE: [f32; 2] = [1240.0, 640.0];

const TOP_MARGIN: f32 = 10.0;
const PILL_HEIGHT: f32 = 34.0;
const HUD_SIZE: Vec2 = vec2(300.0, 42.0);
const BATTERY_HUD_SIZE: Vec2 = vec2(320.0, 42.0);
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
/// Faixinha compacta de periféricos sem fio (mouse, teclado, controle...).
const BATTERY_ROW_HEIGHT: f32 = 30.0;
const BATTERIES_PER_ROW: usize = 3;
const MAX_STRIP_BATTERIES: usize = 6;
/// Pílula do modo jogo: um pouco menor que a principal.
const GAME_PILL_HEIGHT: f32 = 30.0;
const GAME_PILL_GAP: f32 = 8.0;
const WELCOME_SIZE: Vec2 = vec2(400.0, 434.0);
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
/// Altura da página do clima.
const WEATHER_PAGE_HEIGHT: f32 = 200.0;
/// Linha do botão "Usar localização do Windows" (só com a cidade pela internet).
const WEATHER_BUTTON_HEIGHT: f32 = 34.0;
/// Última página do painel (0 = Som, 1 = Alarme, 2 = Clima).
const LAST_PAGE: f32 = 2.0;
/// Espaço do clima na pílula (ícone + temperatura).
const PILL_WEATHER_WIDTH: f32 = 54.0;
/// Cartão que aparece quando o alarme toca.
const RINGING_SIZE: Vec2 = vec2(360.0, 76.0);
/// Janela do pedido do iFood (abre ao clicar na pílula do pedido).
const ORDER_WIDTH: f32 = 420.0;
/// Até o começo do mapa: cabeçalho, etapa e a linha do andamento.
const ORDER_TOP: f32 = 118.0;
const ORDER_MAP_HEIGHT: f32 = 170.0;
/// Sem os pontos do mapa: um desenho loja → você no lugar.
const ORDER_ROUTE_HEIGHT: f32 = 64.0;
/// Depois de um clique num dispositivo, quanto tempo esperar o Windows confirmar.
const DEVICE_PENDING: Duration = Duration::from_secs(2);
const BARS: usize = 5;

const HUD_SECONDS: f32 = 1.6;
const BATTERY_HUD_SECONDS: f32 = 4.5;
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
/// Vermelho da pílula do pedido (cor de comida/entrega).
const FOOD: Color32 = Color32::from_rgb(234, 29, 44);
/// Previsão de entrega que já passou.
const LATE: Color32 = Color32::from_rgb(255, 159, 10);
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
    /// Janela do pedido do iFood: etapa, previsão, mapa.
    Order,
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
    /// O que aparece na ilha e de qual sensor vem a temperatura.
    settings: Settings,
    /// Último aviso mandado ao backend sobre isso (e o cartão estar aberto).
    sent_stats: Option<(Settings, bool)>,
    /// 0..1, mouse em cima da ilha.
    hover: Spring,
    /// 0 = sem jogo, 1 = pílula do modo jogo aberta.
    game: Spring,
    game_width: Spring,
    /// Último jogo visto (continua desenhado enquanto a pílula fecha).
    last_game: Option<Game>,
    /// 0 = sem pedido, 1 = pílula do pedido do iFood aberta (à esquerda da ilha).
    order: Spring,
    order_width: Spring,
    order_height: Spring,
    /// Último pedido visto (continua desenhado enquanto a pílula fecha).
    last_order: Option<Order>,
    /// Novidade do pedido que já ganhou o "pulinho" da pílula.
    order_bumped: Option<Instant>,
    /// O botão da extensão do iFood já foi clicado no cartão inicial.
    extension_opened: bool,
    /// A extensão do iFood já está instalada (conferido ao abrir o cartão inicial).
    extension_installed: bool,
    /// Onde a pílula (ou janela) do pedido estava no último quadro (para o mouse e o clique).
    order_rect: Option<Rect>,
    /// Mouse em cima da pílula do pedido.
    order_hovered: bool,
    /// 0..1, quanto a pílula do pedido já abriu na janela do pedido.
    order_open: Spring,
    /// Os pedacinhos do mapa da janela do pedido.
    map: Map,

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
    /// Página do painel: 0 = Som, 1 = Alarme, 2 = Clima (valores no meio = deslizando).
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
    /// Controle de avisos de bateria fraca (20% e 10%) na pílula fechada.
    battery_alerts: AlertTracker,
    /// Aviso ativo de bateria fraca sendo exibido no lugar do HUD de volume.
    battery_hud: Option<LowBatteryAlert>,
    /// Último contexto (painel aberto, em jogo) informado à thread de bateria.
    sent_battery_ctx: Option<(bool, bool)>,

    window_pos: Option<Pos2>,
    window_width: f32,
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
        delivery::start(wake(cc.egui_ctx.clone()));
        weather::start(wake(cc.egui_ctx.clone()));
        battery::start(wake(cc.egui_ctx.clone()));
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
            settings: Settings::load(),
            sent_stats: None,
            hover: Spring::smooth(0.0),
            game: Spring::bouncy(0.0),
            game_width: Spring::bouncy(120.0),
            last_game: None,
            order: Spring::bouncy(0.0),
            order_width: Spring::bouncy(160.0),
            order_height: Spring::bouncy(GAME_PILL_HEIGHT),
            last_order: None,
            order_bumped: None,
            extension_opened: false,
            extension_installed: mode == Mode::Welcome && delivery::extension::installed(),
            order_rect: None,
            order_hovered: false,
            order_open: Spring::smooth(0.0),
            map: Map::new(wake(cc.egui_ctx.clone())),
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
            battery_alerts: AlertTracker::default(),
            battery_hud: None,
            sent_battery_ctx: None,
            alarm: Alarm::load(),
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
            window_width: WINDOW_SIZE[0],
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

    /// Mantém a janela centralizada no topo do monitor (e dentro da largura dele).
    fn place_window(&mut self, ctx: &egui::Context) {
        let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) else { return };
        let width = WINDOW_SIZE[0].min(monitor.x).max(EXPANDED_WIDTH + 40.0);
        if (width - self.window_width).abs() > 0.5 {
            self.window_width = width;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(width, WINDOW_SIZE[1])));
        }
        let want = pos2(((monitor.x - width) / 2.0).round(), 0.0);
        if self.window_pos != Some(want) {
            self.window_pos = Some(want);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(want));
        }
    }

    /// O mouse está em cima da ilha (ou da pílula do pedido, `extra`)?
    fn cursor_inside(&self, ctx: &egui::Context, island: Rect, extra: Option<Rect>) -> bool {
        // O vigia do mouse confere um retângulo só: o que cobre as duas pílulas.
        let hit = extra.map_or(island, |r| island.union(r)).expand(6.0);

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

    fn target_size(&self, snap: &Snapshot, strip_len: usize) -> Vec2 {
        match self.mode {
            Mode::Welcome => WELCOME_SIZE,
            Mode::Collapsed | Mode::Order => {
                // Sem música a pílula mostra o uso do PC; quanto mais números, mais larga.
                // (No modo jogo esses números vão para a pílula ao lado.)
                let stats = snap.media.is_none() && snap.game.is_none() && self.notice_timer <= 0.0;
                let shown = if stats { self.pill_stats(&snap.system).len() as f32 } else { 0.0 };
                let mut width = (150.0 + 50.0 * shown).max(250.0);
                if self.alarm.counting() {
                    width += 56.0; // sininho + tempo restante
                }
                if self.pill_weather().is_some() {
                    width += PILL_WEATHER_WIDTH;
                }
                vec2(width, PILL_HEIGHT) + vec2(14.0, 4.0) * self.hover.target
            }
            Mode::Hud => {
                if self.battery_hud.is_some() {
                    BATTERY_HUD_SIZE
                } else {
                    HUD_SIZE
                }
            }
            Mode::Expanded => {
                vec2(EXPANDED_WIDTH, panel_fixed_height(snap, strip_len) + self.middle_height(snap, strip_len))
            }
            Mode::Ringing => RINGING_SIZE,
        }
    }

    /// Altura da parte do painel que desliza, conforme a página aparecendo.
    fn middle_height(&self, snap: &Snapshot, strip_len: usize) -> f32 {
        let heights = [sound_page_height(snap, strip_len), ALARM_PAGE_HEIGHT, weather_page_height()];
        let t = self.page.value.clamp(0.0, LAST_PAGE);
        let from = (t.floor() as usize).min(heights.len() - 2);
        let k = t - from as f32;
        heights[from] * (1.0 - k) + heights[from + 1] * k
    }

    /// O clima da pílula, se foi escolhido e já chegou.
    fn pill_weather(&self) -> Option<std::sync::Arc<Weather>> {
        self.settings.weather.then(weather::current).flatten()
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
        let temp = sys.temp(self.settings.temp);
        let targets = [sys.cpu, sys.gpu.unwrap_or(0.0), sys.ram(), temp.map_or(0.0, |t| temp_fraction(t.celsius))];
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
        // O pedido do iFood (se uma aba do navegador está mostrando um).
        let current_order = delivery::current(now);
        if current_order.is_some() {
            self.last_order = current_order.clone();
        } else {
            if self.mode == Mode::Order {
                self.mode = Mode::Collapsed;
            }
            self.map.clear();
        }
        // Atualização baixada: reinicia quando a ilha não está em uso. Durante
        // um jogo espera ele fechar, para a troca não atrapalhar a partida.
        // (Um alarme marcado continua depois de reiniciar; só espera se estiver tocando.)
        if updater::ready()
            && self.mode == Mode::Collapsed
            && snap.game.is_none()
            && !self.alarm.ringing()
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
                self.battery_hud = None;
                self.mode = Mode::Hud;
                self.hud_timer = HUD_SECONDS;
            }
        }
        self.last_master = Some(master_now);

        // --- Bateria dos periféricos sem fio ---
        let batteries = battery::current();
        let can_notify_battery = snap.game.is_none()
            && self.mode == Mode::Collapsed
            && self.uptime > 1.2
            && self.notice_timer <= 0.0
            && !self.pinned;
        if let Some(alert) = self.battery_alerts.poll(&batteries, can_notify_battery) {
            self.battery_hud = Some(alert);
            self.mode = Mode::Hud;
            self.hud_timer = BATTERY_HUD_SECONDS;
        }
        if self.mode != Mode::Hud && self.hud.value < 0.02 {
            self.battery_hud = None;
        }
        let panel_bat = (self.mode == Mode::Expanded || self.open.value > 0.01)
            .then(|| battery::match_panel_batteries(&batteries, &snap.outputs, &snap.inputs));
        let strip_len = panel_bat.as_ref().map_or(0, |b| b.strip.len());

        // --- Geometria atual da ilha ---
        let size = self.target_size(&snap, strip_len);
        self.width.target = size.x;
        self.height.target = size.y;
        self.width.update(dt);
        self.height.update(dt);
        let (w, h) = (self.width.value.max(8.0), self.height.value.max(8.0));
        let middle = ui.max_rect().center().x;
        let island = Rect::from_min_size(pos2(middle - w / 2.0, TOP_MARGIN), vec2(w, h));
        let radius = (h / 2.0).min(26.0);

        // --- Mouse e troca de estados ---
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        // A pílula (ou janela) do pedido também conta como "em cima da ilha" (Ctrl e clique).
        let order_area =
            self.order_rect.filter(|_| matches!(self.mode, Mode::Collapsed | Mode::Hud | Mode::Expanded | Mode::Order));
        let hovered = self.cursor_inside(&ctx, island, order_area);
        // Ctrl com o mouse em cima: a ilha sai da frente (some e deixa os
        // cliques passarem) até o mouse sair de cima. Dá para soltar o Ctrl e
        // clicar normalmente no que estava atrás.
        let can_ghost =
            matches!(self.mode, Mode::Collapsed | Mode::Hud | Mode::Expanded | Mode::Order) && self.editing.is_none();
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
        let pointer_known = ctx
            .pointer_latest_pos()
            .is_some_and(|p| island.expand(8.0).contains(p) || order_area.is_some_and(|r| r.expand(8.0).contains(p)));
        if cfg!(windows) && hovered && !pointer_known && self.nudge_cooldown == 0.0 {
            self.nudge_step = -self.nudge_step;
            platform::nudge_cursor(self.nudge_step);
            self.nudge_cooldown = 0.08;
        }

        // Diagnóstico (ISLANDBAR_DEBUG_UI=1): registra quando o estado do mouse muda.
        if self.debug_ui {
            let state =
                format!("mode={:?} hovered={hovered} down={pointer_down} pointer_known={pointer_known}", self.mode);
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
            Mode::Order => {
                if hovered || pointer_down {
                    self.leave_timer = 0.0;
                } else {
                    self.leave_timer += dt;
                    if self.leave_timer > LEAVE_SECONDS && !self.pinned {
                        self.mode = Mode::Collapsed;
                    }
                }
            }
            Mode::Collapsed | Mode::Welcome | Mode::Ringing => {}
        }

        let pointer_on_order = order_area.is_some_and(|r| ctx.pointer_latest_pos().is_some_and(|p| r.contains(p)));
        if matches!(self.mode, Mode::Collapsed | Mode::Hud | Mode::Order) && !pointer_on_order {
            let click = ui.interact(island, ui.scope_id().with("island"), Sense::click());
            if click.clicked() {
                self.mode = Mode::Expanded;
                self.leave_timer = 0.0;
            }
            if click.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }
        // Clique na pílula do pedido: abre a janela do pedido a partir dela mesma.
        self.order_hovered = false;
        if let Some(area) = order_area.filter(|_| !self.ghost && self.mode != Mode::Order) {
            let click = ui.interact(area, ui.scope_id().with("order"), Sense::click());
            self.order_hovered = click.hovered();
            if click.clicked() {
                if self.last_order.as_ref().is_some_and(Order::is_login) {
                    // O aviso de login vencido: abre o site do iFood para entrar.
                    delivery::open_order(delivery::LOGIN);
                } else {
                    self.mode = Mode::Order;
                    self.leave_timer = 0.0;
                }
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
        let bat_ctx = (panel_open, snap.game.is_some());
        if self.sent_battery_ctx != Some(bat_ctx) {
            self.sent_battery_ctx = Some(bat_ctx);
            battery::set_context(bat_ctx.0, bat_ctx.1);
        }
        let stats = (self.settings, self.mode == Mode::Welcome);
        if self.sent_stats != Some(stats) {
            self.sent_stats = Some(stats);
            self.backend.send(Command::Stats { settings: stats.0, welcome: stats.1 });
        }

        self.open.target = if self.mode == Mode::Expanded { 1.0 } else { 0.0 };
        self.hud.target = if self.mode == Mode::Hud { 1.0 } else { 0.0 };
        self.hover.target = if hovered && !pointer_on_order { 1.0 } else { 0.0 };
        self.open.update(dt);
        self.welcome.target = if self.mode == Mode::Welcome { 1.0 } else { 0.0 };
        self.welcome.update(dt);
        self.ring.target = if self.mode == Mode::Ringing { 1.0 } else { 0.0 };
        self.ring.update(dt);
        self.page.update(dt);
        self.ghost_fade.target = if self.ghost { 1.0 } else { 0.0 };
        self.ghost_fade.update(dt);
        self.order_open.target = if self.mode == Mode::Order { 1.0 } else { 0.0 };
        self.order_open.update(dt);

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
            Shadow { offset: [0, 0], blur: 26, spread: 1, color: fade(accent, glow * 0.55) }.as_shape(island, radius),
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
            self.draw_welcome(ui, &content, island, &snap, accent);
        }
        if open > 0.01 {
            if let Some(panel_bat) = panel_bat.as_ref() {
                self.draw_panel(ui, &content, island, &snap, panel_bat, accent);
            }
        }
        if ring > 0.01 {
            self.draw_ringing(ui, &content, island, accent, ring, time);
        }

        let order_highlight = self.draw_order_pill(ui, &painter, island, current_order.as_ref(), accent, now, time, dt);

        // --- Quando redesenhar ---
        let springs = [
            &self.width,
            &self.height,
            &self.open,
            &self.hud,
            &self.hover,
            &self.welcome,
            &self.game,
            &self.game_width,
            &self.order,
            &self.order_width,
            &self.order_height,
            &self.order_open,
            &self.ring,
            &self.page,
            &self.ghost_fade,
        ];
        let animating = springs.iter().any(|s| !s.settled());
        // O cartão do alarme tocando não precisa da taxa do monitor (veja abaixo).
        let busy = animating || pointer_down || !matches!(self.mode, Mode::Collapsed | Mode::Ringing);
        // Enquanto o som toca, o sininho balança.
        let swinging = self.mode == Mode::Ringing && self.alarm.sounding();
        // As barrinhas ainda estão se mexendo (ou vão começar)?
        let dancing = level > SOUND || self.master_peak > SOUND || self.bars.iter().any(|bar| *bar > 0.01);
        // O pet faz um gesto de tempos em tempos (só na pílula, fora do modo jogo).
        let pet_awake =
            self.settings.pet != Pet::Bars && self.mode == Mode::Collapsed && snap.game.is_none() && !self.ghost;
        let pet_next = pet_awake.then(|| pet::next_gesture_in(ctx.input(|i| i.time)));
        let gesturing = pet_next == Some(0.0);

        let mode = self.mode;
        self.perf.frame(|| {
            format!("mode={mode:?} hovered={hovered} game={} sound={}", snap.game.is_some(), snap.master_peak > SOUND)
        });

        self.resting = false;
        if busy {
            ctx.request_repaint();
        } else if hovered || dancing || swinging || gesturing || order_highlight {
            ctx.request_repaint_after(AMBIENT_FRAME);
        } else {
            // Nada se mexendo: dorme até alguém avisar que algo mudou.
            self.resting = true;
            let mut wait = REST_FRAME;
            if self.notice_timer > 0.0 {
                wait = wait.min(Duration::from_secs_f32(self.notice_timer));
            }
            if self.uptime < 1.3 && !batteries.is_empty() {
                wait = wait.min(Duration::from_secs_f32(1.3 - self.uptime));
            }
            // Acorda quando o tempo restante na pílula muda e no segundo em
            // que o alarme deve tocar.
            if let Some(left) = self.alarm.remaining(now) {
                wait = wait.min(alarm::next_change(left));
            }
            // Acorda para o próximo gesto do pet.
            if let Some(next) = pet_next {
                wait = wait.min(Duration::from_secs_f64(next));
            }
            // E quando o pedido do iFood deve sumir sozinho.
            if let Some(next) = delivery::next_change(now) {
                wait = wait.min(next.max(Duration::from_millis(1)));
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

        // O pet (ou as barrinhas) no canto esquerdo. No modo jogo ele fica quieto.
        let pose = if snap.game.is_some() { pet::Pose::default() } else { pet::gesture(p.ctx().input(|i| i.time)) };
        pet::draw(p, self.settings.pet, pos2(left + 13.5, cy), 1.0, &self.bars, pose, accent, ISLAND, alpha);

        let clock = chrono::Local::now().format("%H:%M").to_string();
        let mut clock_rect =
            p.text(pos2(right, cy), Align2::RIGHT_CENTER, clock, FontId::proportional(13.5), fade(TEXT, alpha));
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
        // Clima: ícone e temperatura, antes do relógio.
        if let Some(now) = self.pill_weather() {
            let text = p.text(
                pos2(clock_rect.left() - 12.0, cy),
                Align2::RIGHT_CENTER,
                format!("{:.0}°", now.temp),
                FontId::proportional(12.5),
                fade(TEXT, alpha),
            );
            let icon = pos2(text.left() - 11.0, cy);
            weather::draw_icon(p, icon, 17.0, now.sky(), now.day, ISLAND, alpha);
            clock_rect.min.x = icon.x - 10.0;
        }

        if self.notice_timer > 0.0 {
            let x = left + 32.0;
            let notice = match updater::BUILD {
                Some(build) => format!("Atualizado: versão {build}"),
                None => "Atualizado".to_string(),
            };
            text_fit(
                p,
                pos2(x, cy),
                Align2::LEFT_CENTER,
                &notice,
                12.5,
                fade(accent, alpha),
                clock_rect.left() - 10.0 - x,
            );
        } else if let Some(media) = &snap.media {
            let x = left + 32.0;
            let max_width = clock_rect.left() - 10.0 - x;
            let color = if media.playing { TEXT } else { TEXT_DIM };
            text_fit(p, pos2(x, cy), Align2::LEFT_CENTER, &media.title, 12.5, fade(color, alpha), max_width);
        } else if let Some(game) = &snap.game {
            // Jogando sem música: o nome do jogo (os números estão na pílula ao lado).
            let x = left + 32.0;
            text_fit(
                p,
                pos2(x, cy),
                Align2::LEFT_CENTER,
                &game.name,
                12.5,
                fade(TEXT_DIM, alpha),
                clock_rect.left() - 10.0 - x,
            );
        } else {
            // Sem música: mostra o uso do PC (o que foi escolhido no cartão inicial).
            let usage: Vec<String> = self
                .pill_stats(&snap.system)
                .into_iter()
                .map(|(label, value)| if label == "TEMP" { value } else { format!("{label} {value}") })
                .collect();
            let usage = usage.join("  ");
            // Centralizado entre a animação do som e o relógio, com a mesma folga dos dois lados.
            let (start, end) = (left + 27.0 + 12.0, clock_rect.left() - 12.0);
            text_fit(
                p,
                pos2((start + end) / 2.0, cy),
                Align2::CENTER_CENTER,
                &usage,
                12.0,
                fade(TEXT_DIM, alpha),
                end - start,
            );
        }
    }

    /// Aviso rápido na pílula: volume ou bateria fraca de periférico.
    fn draw_hud(&self, p: &Painter, island: Rect, snap: &Snapshot, accent: Color32, alpha: f32) {
        let cy = island.center().y;
        if let Some(alert) = &self.battery_hud {
            let status_color = battery::battery_color(alert.percent, false, TEXT);
            battery::draw_kind_icon(p, pos2(island.left() + 24.0, cy), 8.2, alert.kind, fade(status_color, alpha));

            let pct_rect = p.text(
                pos2(island.right() - 16.0, cy),
                Align2::RIGHT_CENTER,
                format!("{}%", alert.percent),
                FontId::proportional(13.0),
                fade(status_color, alpha),
            );
            battery::draw_mini_battery(p, pos2(pct_rect.left() - 10.0, cy), alert.percent, false, TEXT, alpha);

            let text_x = island.left() + 42.0;
            let max_w = (pct_rect.left() - 24.0 - text_x).max(40.0);
            let sub = if alert.percent <= battery::CRITICAL_THRESHOLD { "Bateria crítica" } else { "Bateria fraca" };
            text_fit(p, pos2(text_x, cy - 6.5), Align2::LEFT_CENTER, &alert.name, 12.0, fade(TEXT, alpha), max_w);
            text_fit(p, pos2(text_x, cy + 7.5), Align2::LEFT_CENTER, sub, 10.5, fade(status_color, alpha), max_w);
            return;
        }

        let level = if snap.master_muted { 0.0 } else { snap.master };
        draw_speaker(p, pos2(island.left() + 26.0, cy), 9.0, level, snap.master_muted, fade(TEXT, alpha));

        let track = Rect::from_min_max(pos2(island.left() + 50.0, cy - 3.5), pos2(island.right() - 56.0, cy + 3.5));
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
    fn draw_panel(
        &mut self,
        ui: &mut Ui,
        p: &Painter,
        island: Rect,
        snap: &Snapshot,
        panel_bat: &PanelBatteries,
        accent: Color32,
    ) {
        let strip_len = panel_bat.strip.len();
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
        let pose = pet::gesture(ui.input(|i| i.time));
        pet::draw(p, self.settings.pet, art.center(), 1.65, &self.bars, pose, accent, ISLAND, a);

        let buttons_left = right - 3.0 * 36.0;
        let text_x = art.right() + 14.0;
        let text_width = buttons_left - 8.0 - text_x;
        let cy = art.center().y;
        match &snap.media {
            Some(media) => {
                let artist = if media.artist.is_empty() { "Tocando agora" } else { &media.artist };
                text_fit(
                    p,
                    pos2(text_x, cy - 10.0),
                    Align2::LEFT_CENTER,
                    &media.title,
                    15.0,
                    fade(TEXT, a),
                    text_width,
                );
                text_fit(p, pos2(text_x, cy + 10.0), Align2::LEFT_CENTER, artist, 12.5, fade(TEXT_DIM, a), text_width);
            }
            None => {
                let date = chrono::Local::now().format("%d/%m/%Y").to_string();
                text_fit(
                    p,
                    pos2(text_x, cy - 10.0),
                    Align2::LEFT_CENTER,
                    "Nada tocando",
                    15.0,
                    fade(TEXT, a),
                    text_width,
                );
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
            let (clicked, scale) =
                icon_button(ui, p, ("media", i), center, 16.0, interactive && has_media, media_alpha);
            draw_media_icon(p, center, 7.5 * scale, icon, fade(TEXT, media_alpha));
            if clicked {
                self.backend.send(cmd);
            }
        }

        let line_y = island.top() + PADDING + HEADER_HEIGHT + 7.0;
        p.line_segment([pos2(left, line_y), pos2(right, line_y)], Stroke::new(1.0, fade(Color32::WHITE, a * 0.08)));

        // ---- Meio do painel: páginas que deslizam (Som | Alarme) ----
        let page_top = line_y + 7.0;
        let middle_bottom = page_top + self.middle_height(snap, strip_len);
        let clip = Rect::from_min_max(
            pos2(island.left() + 1.0, line_y + 1.0),
            pos2(island.right() - 1.0, middle_bottom + 4.0),
        )
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
                at.elapsed() > DEVICE_PENDING
                    || snap.outputs.iter().chain(&snap.inputs).any(|d| d.default && d.id == *id)
            }) {
                self.pending_device = None;
            }
            // As duas fileiras usam a mesma largura de botão, para ficarem alinhadas.
            let columns = snap.outputs.len().max(snap.inputs.len()).clamp(1, DEVICES_PER_ROW);
            for (devices, badges, is_output) in
                [(&snap.outputs, &panel_bat.output_badges, true), (&snap.inputs, &panel_bat.input_badges, false)]
            {
                let shown = &devices[..devices.len().min(MAX_DEVICES)];
                if shown.is_empty() {
                    continue;
                }
                let names = short_names(shown);
                // O destaque vai para o clicado assim que ele for clicado.
                let pending = self
                    .pending_device
                    .as_ref()
                    .map(|(id, _)| id.clone())
                    .filter(|id| shown.iter().any(|d| d.id == *id));
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
                        let badge = badges.get(index).and_then(Option::as_ref);
                        let clicked = self.draw_device_chip(
                            ui,
                            p,
                            chip,
                            device,
                            &names[index],
                            badge,
                            current,
                            accent,
                            a,
                            interactive,
                        );
                        if clicked && !current {
                            self.backend.send(Command::SetDevice(device.id.clone()));
                            self.pending_device = Some((device.id.clone(), Instant::now()));
                        }
                    }
                }
            }

            // ---- Mixer ----
            let mut rows: Vec<(String, String, f32, bool, f32)> =
                vec![(MASTER_KEY.to_string(), "Geral".to_string(), snap.master, snap.master_muted, self.master_peak)];
            for s in snap.sessions.iter().take(apps_shown(snap, strip_len)) {
                let peak = self.app_peaks.get(&s.name).copied().unwrap_or(0.0);
                rows.push((s.name.clone(), s.name.clone(), s.volume, s.muted, peak));
            }

            let mut still_dragging = false;
            for (key, label, volume, muted, peak) in rows {
                let a = next_alpha(open);
                let is_master = key == MASTER_KEY;
                let rect = Rect::from_min_max(pos2(left, y), pos2(right, y + ROW_HEIGHT))
                    .translate(vec2(0.0, (1.0 - a) * 12.0));
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
        if (self.page.value - 1.0).abs() < 0.999 {
            let alarm_dx = (1.0 - self.page.value) * shift;
            let p = &p.with_clip_rect(clip);
            let a = smoothstep(open * 1.9 - 0.25 - 0.09);
            let interactive = interactive && still && self.page.target == 1.0;
            self.draw_alarm_page(ui, p, left + alarm_dx, right + alarm_dx, page_top, accent, a, interactive);
        }

        // ---- Página Clima ----
        if self.page.value > 1.001 {
            let weather_dx = (2.0 - self.page.value) * shift;
            let p = &p.with_clip_rect(clip);
            let a = smoothstep(open * 1.9 - 0.25 - 0.09);
            let interactive = interactive && still && self.page.target == 2.0;
            draw_weather_page(ui, p, left + weather_dx, right + weather_dx, page_top, accent, a, interactive);
        }
        let mut y = middle_bottom;

        // ---- Periféricos sem fio (mouse, teclado, controle...) ----
        if !panel_bat.strip.is_empty() {
            let a = next_alpha(open);
            self.draw_battery_strip(ui, p, left, right, y + 4.0 + (1.0 - a) * 12.0, &panel_bat.strip, a, interactive);
            y += strip_height(strip_len);
        }

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
        for (index, page) in [0.0f32, 1.0, 2.0].into_iter().enumerate() {
            let center = pos2(island.center().x - 16.0 + index as f32 * 16.0, footer_y);
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
        let quit_resp =
            if interactive { Some(ui.interact(quit, ui.scope_id().with("quit"), Sense::click())) } else { None };
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
                if !(0.0..=LAST_PAGE).contains(&value) {
                    value = value.clamp(-0.12, LAST_PAGE + 0.12);
                }
                self.page.value = value;
                self.page.target = value;
            }
            if resp.drag_stopped() || !resp.dragged() {
                self.page_drag = None;
                let moved = self.page.value - start;
                let speed = ui.ctx().input(|i| i.pointer.velocity().x);
                let start = start.round();
                let target = if moved > 0.25 || speed < -400.0 {
                    (start + 1.0).max(self.page.value.round())
                } else if moved < -0.25 || speed > 400.0 {
                    (start - 1.0).min(self.page.value.round())
                } else {
                    start
                };
                self.page.target = target.clamp(0.0, LAST_PAGE);
            }
        }

        // Rodinha horizontal / dois dedos no touchpad.
        if resp.hovered() || ui.rect_contains_pointer(area) {
            let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.x);
            self.wheel = if scroll == 0.0 { self.wheel * 0.9 } else { self.wheel + scroll };
            if self.wheel.abs() > 60.0 {
                let step = if self.wheel < 0.0 { 1.0 } else { -1.0 };
                self.page.target = (self.page.target.round() + step).clamp(0.0, LAST_PAGE);
                self.wheel = 0.0;
            }
        }
    }

    /// Página do alarme: escolher o tempo, ou ver quanto falta.
    #[allow(clippy::too_many_arguments)]
    fn draw_alarm_page(
        &mut self,
        ui: &mut Ui,
        p: &Painter,
        left: f32,
        right: f32,
        top: f32,
        accent: Color32,
        a: f32,
        interactive: bool,
    ) {
        let cx = (left + right) / 2.0;
        let now = Instant::now();
        if let Some(left_time) = self.alarm.remaining(now) {
            // ---- Contando ----
            p.text(
                pos2(cx, top + 52.0),
                Align2::CENTER_CENTER,
                alarm::countdown(left_time),
                FontId::proportional(38.0),
                fade(TEXT, a),
            );
            let at = self.alarm.due_clock().unwrap_or_default();
            p.text(
                pos2(cx, top + 92.0),
                Align2::CENTER_CENTER,
                format!("Toca às {at}"),
                FontId::proportional(13.0),
                fade(TEXT_DIM, a),
            );
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
                && i.pointer
                    .interact_pos()
                    .is_none_or(|pos| !digit_rect(0).contains(pos) && !digit_rect(1).contains(pos))
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
            let shown =
                p.text(pos2(x, digits_y), Align2::CENTER_CENTER, text, FontId::proportional(36.0), fade(color, a));
            // Cursor piscando enquanto digita.
            if typing.is_some() && (time * 2.0) as i64 % 2 == 0 {
                let caret_x = shown.right() + 2.0;
                p.line_segment(
                    [pos2(caret_x, digits_y - 14.0), pos2(caret_x, digits_y + 14.0)],
                    Stroke::new(2.0, fade(accent, a)),
                );
            }
            // "h" / "min" do lado de fora da caixa de digitação (senão a borda passa por cima).
            p.text(
                pos2(rect.right() + 5.0, digits_y + 6.0),
                Align2::LEFT_CENTER,
                unit,
                FontId::proportional(13.0),
                fade(TEXT_DIM, a),
            );
            for (row, direction) in [(-1.0f32, 1), (1.0, -1)] {
                let center = pos2(x, digits_y + row * 34.0);
                let (clicked, scale) =
                    icon_button(ui, p, ("alarm-step", column, direction), center, 13.0, interactive, a);
                draw_chevron(p, center, 5.5 * scale, row < 0.0, fade(TEXT_DIM, a));
                if clicked {
                    self.end_editing(true);
                    self.alarm.change(direction * step);
                }
            }
        }
        p.text(
            pos2(cx - 5.0, digits_y - 2.0),
            Align2::CENTER_CENTER,
            ":",
            FontId::proportional(30.0),
            fade(TEXT_DIM, a),
        );

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
        p.text(
            pos2(text_x, cy + 10.0),
            Align2::LEFT_CENTER,
            subtitle,
            FontId::proportional(12.0),
            fade(TEXT_DIM, alpha),
        );

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
        battery: Option<&Peripheral>,
        current: bool,
        accent: Color32,
        a: f32,
        interactive: bool,
    ) -> bool {
        let id = ui.scope_id().with(("device", &device.id));
        let resp = interactive.then(|| {
            let interact = ui.interact(chip, id, Sense::click());
            if let Some(bat) = battery {
                let extra = if bat.charging { " (carregando)" } else { "" };
                interact.on_hover_text(format!("{} · Bateria: {}%{extra}", device.name, bat.percent))
            } else {
                interact.on_hover_text(&device.name)
            }
        });
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
        let cy = chip.center().y;
        let mut right_limit = chip.right() - 8.0;
        if let Some(bat) = battery {
            let bat_col = battery::battery_color(bat.percent, bat.charging, color);
            let pct_rect = p.text(
                pos2(chip.right() - 8.0, cy),
                Align2::RIGHT_CENTER,
                format!("{}%", bat.percent),
                FontId::proportional(10.5),
                fade(bat_col, a),
            );
            right_limit = pct_rect.left() - 5.0;
            if chip.width() >= 96.0 {
                battery::draw_mini_battery(p, pos2(pct_rect.left() - 8.5, cy), bat.percent, bat.charging, color, a);
                right_limit = pct_rect.left() - 17.5;
            }
        }
        let width = (right_limit - text_x).max(16.0);
        text_fit(p, pos2(text_x, cy), Align2::LEFT_CENTER, label, 11.5, fade(color, a), width);
        resp.is_some_and(|r| r.clicked())
    }

    /// Faixinha compacta acima do monitor do sistema com mouse, teclado e controle sem fio.
    #[allow(clippy::too_many_arguments)]
    fn draw_battery_strip(
        &self,
        ui: &mut Ui,
        p: &Painter,
        left: f32,
        right: f32,
        top_y: f32,
        strip: &[Peripheral],
        a: f32,
        interactive: bool,
    ) {
        let shown = &strip[..strip.len().min(MAX_STRIP_BATTERIES)];
        if shown.is_empty() {
            return;
        }
        let cols = shown.len().clamp(1, BATTERIES_PER_ROW);
        let gap = if cols == 2 { TILE_GAP } else { 6.0 };
        let chip_w = (right - left - gap * (cols - 1) as f32) / cols as f32;

        for (line, chunk) in shown.chunks(BATTERIES_PER_ROW).enumerate() {
            let row_top = top_y + line as f32 * BATTERY_ROW_HEIGHT;
            for (col, item) in chunk.iter().enumerate() {
                let x = left + col as f32 * (chip_w + gap);
                let chip = Rect::from_min_size(pos2(x, row_top), vec2(chip_w, 24.0));
                let id = ui.scope_id().with(("periph", &item.id));
                let resp = interactive.then(|| {
                    let extra = if item.charging { " (carregando)" } else { "" };
                    ui.interact(chip, id, Sense::hover()).on_hover_text(format!(
                        "{} · {} · {}%{extra}",
                        item.name,
                        item.kind.label_pt(),
                        item.percent
                    ))
                });
                let hover = ui.ctx().animate_bool_with_time(id, resp.as_ref().is_some_and(|r| r.hovered()), 0.12);
                let low = !item.charging && item.percent <= battery::LOW_THRESHOLD;
                let bat_col = battery::battery_color(item.percent, item.charging, TEXT);

                if low {
                    p.rect_filled(chip, 10.0, fade(bat_col, a * (0.12 + 0.05 * hover)));
                    p.rect_stroke(chip, 10.0, Stroke::new(1.0, fade(bat_col, a * 0.38)), StrokeKind::Inside);
                } else {
                    p.rect_filled(chip, 10.0, fade(Color32::WHITE, a * (0.055 + 0.045 * hover)));
                    p.rect_stroke(chip, 10.0, Stroke::new(1.0, fade(Color32::WHITE, a * 0.07)), StrokeKind::Inside);
                }

                let cy = chip.center().y;
                let icon_col = if low {
                    bat_col
                } else if hover > 0.5 {
                    TEXT
                } else {
                    TEXT_DIM
                };
                battery::draw_kind_icon(p, pos2(chip.left() + 14.0, cy), 6.6, item.kind, fade(icon_col, a));

                let pct_rect = p.text(
                    pos2(chip.right() - 8.0, cy),
                    Align2::RIGHT_CENTER,
                    format!("{}%", item.percent),
                    FontId::proportional(11.0),
                    fade(bat_col, a),
                );
                battery::draw_mini_battery(
                    p,
                    pos2(pct_rect.left() - 8.5, cy),
                    item.percent,
                    item.charging,
                    TEXT_DIM.lerp_to_gamma(TEXT, 0.5),
                    a,
                );

                let text_x = chip.left() + 26.0;
                let max_w = (pct_rect.left() - 18.0 - text_x).max(18.0);
                let short = battery::short_peripheral_name(&item.name, item.kind);
                let label_col = if low || hover > 0.5 { TEXT } else { TEXT_DIM.lerp_to_gamma(TEXT, 0.35) };
                text_fit(p, pos2(text_x, cy), Align2::LEFT_CENTER, &short, 11.2, fade(label_col, a), max_w);
            }
        }
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
        let mut parts: Vec<(&str, String, &str, Color32)> = Vec::new();
        if let Some(fps) = game.fps {
            parts.push(("FPS", format!("{fps:.0}"), "000", fps_color(fps)));
        }
        for (label, value) in self.pill_stats(&snap.system) {
            parts.push((label, value, if label == "TEMP" { "00°C" } else { "00%" }, TEXT));
        }

        if parts.is_empty() {
            self.game_width.finish();
            return;
        }

        let label_font = FontId::proportional(10.0);
        let value_font = FontId::proportional(12.5);
        // Cada valor ganha pelo menos o espaço de um valor largo típico
        // ("00%"). Assim a pílula não muda de largura (nem precisa ser
        // reanimada) a cada número novo; só em casos raros, como 100%.
        let mut galleys: Vec<_> = parts
            .iter()
            .map(|(label, value, widest, color)| {
                let label = painter.layout_no_wrap(label.to_string(), label_font.clone(), TEXT_DIM);
                let value = painter.layout_no_wrap(value.clone(), value_font.clone(), *color);
                let widest = painter.layout_no_wrap(widest.to_string(), value_font.clone(), *color);
                let space = value.size().x.max(widest.size().x);
                (label, value, space)
            })
            .collect();
        // Espaço até a beirada direita da janela. Em monitores normais cabe com
        // folga; nos muito estreitos (ou com zoom alto) aperta o respiro antes
        // de tirar itens do fim, para nunca cortar a pílula no meio.
        let room = (painter.clip_rect().width() / 2.0 - self.width.target / 2.0 - GAME_PILL_GAP - 14.0).max(80.0);
        let pill_width = |items: &[(std::sync::Arc<egui::Galley>, std::sync::Arc<egui::Galley>, f32)],
                          pad: f32,
                          inner: f32,
                          gap: f32| {
            let content: f32 = items.iter().map(|(label, _, space)| label.size().x + inner + space).sum::<f32>()
                + gap * items.len().saturating_sub(1) as f32;
            ((content + 2.0 * pad) / 8.0).ceil() * 8.0
        };
        let (mut padding, mut inner_gap, mut part_gap) = (14.0, 4.0, 12.0);
        let mut target_width = pill_width(&galleys, padding, inner_gap, part_gap);
        if target_width > room {
            (padding, inner_gap, part_gap) = (10.0, 3.0, 7.0);
            target_width = pill_width(&galleys, padding, inner_gap, part_gap);
            while target_width > room && galleys.len() > 1 {
                galleys.pop();
                target_width = pill_width(&galleys, padding, inner_gap, part_gap);
            }
        }
        self.game_width.target = target_width;
        self.game_width.update(dt);

        let left = island.right() + GAME_PILL_GAP * grow.min(1.0);
        let max_w = (painter.clip_rect().right() - left - 10.0).max(0.0);
        let size = vec2((self.game_width.value * grow.min(1.15)).min(max_w), GAME_PILL_HEIGHT * grow.clamp(0.0, 1.0));
        let center_y = TOP_MARGIN + PILL_HEIGHT / 2.0;
        let rect = Rect::from_min_size(pos2(left, center_y - size.y / 2.0), size);
        let radius = size.y / 2.0;
        let alpha = smoothstep((grow - 0.55) / 0.45);

        painter.add(
            Shadow {
                offset: [0, 4],
                blur: 14,
                spread: 0,
                color: Color32::from_black_alpha((90.0 * grow.min(1.0)) as u8),
            }
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

    /// Pílula do pedido do iFood, à esquerda da ilha: logo, etapa, previsão,
    /// código de entrega e mensagens. Ao clicar, ela mesma se abre na janela
    /// do pedido (sem mexer na pílula central). Devolve se está chamando
    /// atenção (acabou de ter novidade), para a ilha desenhar a animação.
    #[allow(clippy::too_many_arguments)]
    fn draw_order_pill(
        &mut self,
        ui: &mut Ui,
        painter: &Painter,
        island: Rect,
        current: Option<&Order>,
        accent: Color32,
        now: Instant,
        time: f32,
        dt: f32,
    ) -> bool {
        let win_width = painter.clip_rect().width();
        // Espaço até a beirada esquerda da janela (pela largura final da ilha,
        // para não ficar trocando no meio da animação).
        let room = win_width / 2.0 - self.width.target / 2.0 - GAME_PILL_GAP - 14.0;
        // Continua visível mesmo com o painel central aberto (desde que caiba na tela).
        let visible = current.is_some()
            && matches!(self.mode, Mode::Collapsed | Mode::Hud | Mode::Expanded | Mode::Order)
            && (self.mode != Mode::Expanded || room >= 90.0);
        self.order.target = if visible { 1.0 } else { 0.0 };
        // Novidade: a pílula dá um pulinho (quando fechada).
        if let Some(changed) = current.and_then(|o| o.changed) {
            if visible && self.mode != Mode::Order && self.order_bumped != Some(changed) && self.order.value > 0.9 {
                self.order.value = 0.82;
            }
            if self.order.value > 0.5 || !visible {
                self.order_bumped = Some(changed);
            }
        }
        self.order.update(dt);
        let grow = self.order.value;
        if grow <= 0.01 || !visible {
            self.order_rect = None;
        }
        if grow <= 0.01 {
            self.order_width.finish();
            self.order_height.finish();
            return false;
        }
        let Some(order) = self.last_order.clone() else { return false };
        // A novidade aparece na cor de destaque da ilha por alguns segundos.
        let highlight = if visible { order.highlight(now) } else { 0.0 };
        let stage_color =
            if order.is_login() { LATE.lerp_to_gamma(TEXT, 0.25) } else { TEXT.lerp_to_gamma(accent, highlight) };

        // Trechos depois do logo: (texto, tamanho, cor, é o balão de mensagens).
        let stage = (order.stage.text().to_string(), 12.5, stage_color, false);
        let eta = (!order.eta.is_empty() && order.stage < Stage::Arrived).then(|| {
            let color = if eta_passed(&order.eta) { LATE } else { TEXT_DIM };
            (order.eta.replace('-', "–"), 12.0, color, false)
        });
        let code = (order.stage.on_the_way() && !order.code.is_empty())
            .then(|| (format!("código {}", order.code), 12.0, TEXT_DIM.lerp_to_gamma(TEXT, 0.5), false));
        let chat_color = if order.new_chat && highlight > 0.0 { accent } else { TEXT_DIM };
        let chat = (order.chats > 0).then(|| (order.chats.to_string(), 11.5, chat_color, true));
        let logo = ifood_logo_width(painter, 1.0);
        let (padding, gap, bubble) = (9.0, 10.0, 18.0);
        // Se faltar espaço na pílula, sai primeiro a previsão, depois as
        // mensagens, depois o código.
        let mut galleys = Vec::new();
        let mut content = 0.0;
        for drop in 0..4 {
            let parts = [Some(stage.clone()), eta.clone().filter(|_| drop < 1), code.clone().filter(|_| drop < 3)]
                .into_iter()
                .chain([chat.clone().filter(|_| drop < 2)])
                .flatten();
            galleys = parts
                .map(|(text, size, color, is_chat)| {
                    (painter.layout_no_wrap(text, FontId::proportional(size), color), color, is_chat)
                })
                .collect();
            content = logo
                + galleys
                    .iter()
                    .map(|(g, _, is_chat)| gap + g.size().x + if *is_chat { bubble } else { 0.0 })
                    .sum::<f32>();
            if content + padding + 14.0 <= room {
                break;
            }
        }
        let pill_w = (((content + padding + 14.0) / 8.0).ceil() * 8.0).min(room.max(80.0));
        if self.mode == Mode::Order {
            self.order_width.target = ORDER_WIDTH.min((win_width - 24.0).max(240.0));
            self.order_height.target = order_panel_height(Some(&order));
        } else {
            self.order_width.target = pill_w;
            self.order_height.target = GAME_PILL_HEIGHT;
        }
        self.order_width.update(dt);
        self.order_height.update(dt);

        let open = smoothstep(self.order_open.value);
        let size = vec2(
            (self.order_width.value * grow.min(1.15)).max(8.0),
            (self.order_height.value * grow.clamp(0.0, 1.0)).max(8.0),
        );
        let right = island.left() - GAME_PILL_GAP * grow.min(1.0);
        let left = (right - size.x).max(12.0);
        let pill_top = TOP_MARGIN + (PILL_HEIGHT - GAME_PILL_HEIGHT) / 2.0;
        let top = pill_top + (TOP_MARGIN - pill_top) * open;
        let rect = Rect::from_min_size(pos2(left, top), size);
        if visible {
            self.order_rect = Some(rect);
        }
        let radius = (size.y / 2.0).min(26.0);
        let pill_alpha = smoothstep((grow - 0.55) / 0.45) * (1.0 - open * 2.0).clamp(0.0, 1.0);

        // O mesmo visual da ilha: fundo escuro, borda discreta e sombra.
        painter.add(
            Shadow {
                offset: [0, (4.0 + 2.0 * open).round() as i8],
                blur: (14.0 + 4.0 * open).round() as u8,
                spread: 0,
                color: Color32::from_black_alpha(((90.0 + 20.0 * open) * grow.min(1.0)) as u8),
            }
            .as_shape(rect, radius),
        );
        let fill = if self.order_hovered && self.mode != Mode::Order { Color32::from_rgb(22, 22, 26) } else { ISLAND };
        painter.rect_filled(rect, radius, fill);
        painter.rect_stroke(rect, radius, Stroke::new(1.0, fade(accent, 0.18)), StrokeKind::Inside);

        let p = painter.with_clip_rect(rect.shrink(1.0));
        if pill_alpha > 0.01 {
            let center_y = rect.top() + GAME_PILL_HEIGHT / 2.0;
            let mut x = rect.left() + padding;
            draw_ifood_logo(&p, pos2(x, center_y), 1.0, pill_alpha);
            x += logo + gap;
            for (galley, color, is_chat) in galleys {
                if is_chat {
                    draw_chat(&p, pos2(x + 6.5, center_y), fade(color, pill_alpha));
                    x += bubble;
                }
                let pos = pos2(x, center_y - galley.size().y / 2.0);
                x += galley.size().x + gap;
                p.galley_with_override_text_color(pos, galley, fade(color, pill_alpha));
            }
        }
        if open > 0.01 {
            self.draw_order_panel(ui, &p, rect, accent, open, now, time);
        }
        highlight > 0.0
    }

    /// Janela do pedido: loja, etapa, previsão, andamento, mapa e entregador.
    #[allow(clippy::too_many_arguments)]
    fn draw_order_panel(
        &mut self,
        ui: &mut Ui,
        p: &Painter,
        island: Rect,
        accent: Color32,
        open: f32,
        now: Instant,
        time: f32,
    ) {
        let Some(order) = self.last_order.clone() else { return };
        let interactive = self.mode == Mode::Order;
        let a = smoothstep(open * 1.6 - 0.4);
        let left = island.left() + PADDING;
        let right = island.right() - PADDING;
        let top = island.top() + PADDING;

        // Cabeçalho: logo, loja e número do pedido.
        let logo = ifood_logo_width(p, 1.2);
        draw_ifood_logo(p, pos2(left, top + 11.0), 1.2, a);
        let number = (!order.number.is_empty()).then(|| format!("Pedido nº {}", order.number));
        let number_width = number.as_ref().map_or(0.0, |n| {
            p.text(pos2(right, top + 11.0), Align2::RIGHT_CENTER, n, FontId::proportional(11.5), fade(TEXT_DIM, a))
                .width()
                + 12.0
        });
        let store = if order.store.is_empty() { "Seu pedido" } else { &order.store };
        let x = left + logo + 10.0;
        text_fit(p, pos2(x, top + 11.0), Align2::LEFT_CENTER, store, 15.0, fade(TEXT, a), right - number_width - x);

        // Etapa (grande) e previsão de entrega.
        let highlight = order.highlight(now);
        let stage_color = if order.stage == Stage::Cancelled { LATE } else { TEXT.lerp_to_gamma(accent, highlight) };
        p.text(
            pos2(left, top + 50.0),
            Align2::LEFT_CENTER,
            order.stage.text(),
            FontId::proportional(20.0),
            fade(stage_color, a),
        );
        if !order.eta.is_empty() && order.stage < Stage::Arrived {
            let late = eta_passed(&order.eta);
            let label = if late { "Previsão (atrasado)" } else { "Previsão de entrega" };
            p.text(pos2(right, top + 41.0), Align2::RIGHT_CENTER, label, FontId::proportional(10.5), fade(TEXT_DIM, a));
            let color = if late { LATE } else { TEXT };
            let eta = order.eta.replace('-', " – ");
            p.text(pos2(right, top + 58.0), Align2::RIGHT_CENTER, eta, FontId::proportional(15.0), fade(color, a));
        }

        draw_order_steps(p, left, right, top + 86.0, order.stage, accent, a);

        // Mapa (com os pontos que o site informou) ou o desenho loja → você.
        let has_map = order.from.is_some() || order.to.is_some() || order.courier.is_some();
        let height = if has_map { ORDER_MAP_HEIGHT } else { ORDER_ROUTE_HEIGHT };
        let area = Rect::from_min_size(pos2(left, top + ORDER_TOP), vec2(right - left, height));
        if has_map {
            self.draw_order_map(ui.ctx(), p, area, &order, accent, a, time);
        } else {
            draw_route(p, area, order.stage, accent, a, time);
        }

        // Entregador, código e mensagens.
        let mut chips = Vec::new();
        if !order.driver.is_empty() {
            let vehicle = if order.vehicle.is_empty() { String::new() } else { format!(" · {}", order.vehicle) };
            chips.push((format!("{}{vehicle}", order.driver), TEXT));
        } else if order.stage < Stage::Shipping && !order.stage.is_final() {
            chips.push(("Entregador ainda não definido".to_string(), TEXT_DIM));
        }
        if !order.code.is_empty() && !order.stage.is_final() {
            chips.push((format!("Código {}", order.code), TEXT));
        }
        if order.chats > 0 {
            let text = if order.chats == 1 { "1 mensagem".to_string() } else { format!("{} mensagens", order.chats) };
            let color = if order.new_chat && highlight > 0.0 { accent } else { TEXT };
            chips.push((text, color));
        }
        let mut x = left;
        let cy = area.bottom() + 12.0 + 14.0;
        for (text, color) in chips {
            let galley = p.layout_no_wrap(text, FontId::proportional(12.0), color);
            let chip = Rect::from_min_size(pos2(x, cy - 13.0), vec2(galley.size().x + 20.0, 26.0));
            if chip.right() > right {
                break;
            }
            p.rect_filled(chip, 13.0, fade(Color32::WHITE, a * 0.07));
            p.galley_with_override_text_color(chip.center() - galley.size() / 2.0, galley, fade(color, a));
            x = chip.right() + 8.0;
        }

        // Rodapé: abrir a página do pedido no navegador.
        let button = Rect::from_min_max(
            pos2(right - 150.0, island.bottom() - PADDING - 30.0),
            pos2(right, island.bottom() - PADDING),
        );
        if text_button(ui, p, "order-open", button, "Abrir no navegador", false, accent, a, interactive) {
            delivery::open_order(&order.id);
        }
    }

    /// Mapa da janela do pedido: loja, sua casa e o entregador (o que o site informar).
    #[allow(clippy::too_many_arguments)]
    fn draw_order_map(
        &mut self,
        ctx: &egui::Context,
        p: &Painter,
        area: Rect,
        order: &Order,
        accent: Color32,
        a: f32,
        time: f32,
    ) {
        self.map.receive(ctx);
        let points: Vec<(f64, f64)> = [order.from, order.to, order.courier].into_iter().flatten().collect();
        let Some((z, center)) = map::frame(&points, area.width() as f64, area.height() as f64) else { return };
        let origin = (center.0 - area.width() as f64 / 2.0, center.1 - area.height() as f64 / 2.0);
        p.rect_filled(area, 12.0, fade(Color32::from_rgb(26, 26, 30), a));

        // Os azulejos que cobrem o retângulo (os que ainda não chegaram ficam no fundo cinza).
        let clip = p.with_clip_rect(area.intersect(p.clip_rect()));
        let count = 1i64 << z;
        let tiles = |from: f64, size: f32| {
            (from / map::TILE).floor() as i64..=((from + size as f64) / map::TILE).floor() as i64
        };
        for ty in tiles(origin.1, area.height()) {
            if !(0..count).contains(&ty) {
                continue;
            }
            for tx in tiles(origin.0, area.width()) {
                let tile = map::Tile { z, x: tx.rem_euclid(count) as u32, y: ty as u32 };
                if let Some(texture) = self.map.tile(tile) {
                    let min = area.min
                        + vec2((tx as f64 * map::TILE - origin.0) as f32, (ty as f64 * map::TILE - origin.1) as f32);
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    clip.image(
                        texture.id(),
                        Rect::from_min_size(min, Vec2::splat(map::TILE as f32)),
                        uv,
                        fade(Color32::WHITE, a),
                    );
                }
            }
        }

        let at = |point: (f64, f64)| {
            let (x, y) = map::project(point, z);
            area.min + vec2((x - origin.0) as f32, (y - origin.1) as f32)
        };
        if let (Some(from), Some(to)) = (order.from, order.to) {
            clip.extend(Shape::dashed_line(
                &[at(from), at(to)],
                Stroke::new(1.5, fade(Color32::WHITE, a * 0.45)),
                5.0,
                4.0,
            ));
        }
        if let Some(from) = order.from {
            draw_store_pin(&clip, at(from), a);
        }
        if let Some(to) = order.to {
            draw_home_pin(&clip, at(to), a);
        }
        if let Some(courier) = order.courier {
            draw_courier_pin(&clip, at(courier), accent, a, time);
        }

        // Cantos arredondados: uma moldura da cor da ilha por cima das pontas.
        p.rect_stroke(area, 12.0, Stroke::new(6.0, fade(ISLAND, a)), StrokeKind::Outside);
        let credits = p.layout_no_wrap(map::CREDITS.to_string(), FontId::proportional(9.0), TEXT_DIM);
        let box_rect = Rect::from_min_size(
            area.right_bottom() - credits.size() - vec2(14.0, 8.0),
            credits.size() + vec2(8.0, 4.0),
        );
        p.rect_filled(box_rect, 4.0, fade(Color32::from_black_alpha(150), a));
        p.galley_with_override_text_color(box_rect.min + vec2(4.0, 2.0), credits, fade(TEXT_DIM, a));
        if self.map.loading() {
            p.text(
                area.center(),
                Align2::CENTER_CENTER,
                "Carregando mapa…",
                FontId::proportional(11.5),
                fade(TEXT_DIM, a * 0.8),
            );
        }
    }

    /// Linha do painel para liberar a medição de FPS.
    #[allow(clippy::too_many_arguments)]
    fn draw_fps_row(
        &mut self,
        ui: &mut Ui,
        p: &Painter,
        row: Rect,
        access: FpsAccess,
        accent: Color32,
        a: f32,
        interactive: bool,
    ) {
        let cy = row.center().y + (1.0 - a) * 12.0;
        if access == FpsAccess::Requested {
            let text = "Permissão pedida. Saia e entre no Windows para o FPS aparecer.";
            text_fit(p, pos2(row.left(), cy), Align2::LEFT_CENTER, text, 12.0, fade(TEXT_DIM, a), row.width());
            return;
        }

        let button = Rect::from_center_size(pos2(row.right() - 36.0, cy), vec2(72.0, 24.0));
        let text = "Mostrar FPS nos jogos exige uma permissão do Windows";
        text_fit(
            p,
            pos2(row.left(), cy),
            Align2::LEFT_CENTER,
            text,
            12.0,
            fade(TEXT_DIM, a),
            button.left() - 10.0 - row.left(),
        );

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
        let temp = sys.temp(self.settings.temp);
        let temp_label = match temp.map(|t| t.source) {
            Some(TempSource::Gpu) => "Temp GPU",
            Some(TempSource::Disk) => "Temp SSD",
            Some(TempSource::Board) => "Temp placa-mãe",
            None => match self.settings.temp {
                TempChoice::Gpu => "Temp GPU",
                TempChoice::Disk => "Temp SSD",
                TempChoice::Board => "Temp placa-mãe",
                TempChoice::Auto => "Temp",
            },
        };
        let not_available = || "n/d".to_string();
        let tiles = [
            ("CPU", format!("{:.0}%", sys.cpu * 100.0), true),
            ("GPU", sys.gpu.map_or_else(not_available, |g| format!("{:.0}%", g * 100.0)), sys.gpu.is_some()),
            ("RAM", format!("{:.1} / {:.0} GB", sys.ram_used_gb, sys.ram_total_gb), true),
            (temp_label, temp.map_or_else(not_available, |t| format!("{:.0}°C", t.celsius)), temp.is_some()),
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
            p.text(
                pos2(x, rect.center().y - 8.0),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(11.0),
                fade(TEXT_DIM, alpha),
            );
            let value_color = if available { TEXT } else { TEXT_DIM };
            text_fit(
                p,
                pos2(x, rect.center().y + 8.0),
                Align2::LEFT_CENTER,
                &value,
                12.5,
                fade(value_color, alpha),
                rect.right() - 6.0 - x,
            );
        }
    }

    /// Números da pílula, na ordem: (rótulo, valor). Só os escolhidos no
    /// cartão inicial e que o PC informa.
    fn pill_stats(&self, sys: &System) -> Vec<(&'static str, String)> {
        let s = &self.settings;
        let mut stats = Vec::new();
        if s.cpu {
            stats.push(("CPU", format!("{:.0}%", sys.cpu * 100.0)));
        }
        if let Some(gpu) = sys.gpu.filter(|_| s.gpu) {
            stats.push(("GPU", format!("{:.0}%", gpu * 100.0)));
        }
        if s.ram {
            stats.push(("RAM", format!("{:.0}%", sys.ram() * 100.0)));
        }
        if let Some(temp) = sys.temp(s.temp).filter(|_| s.show_temp) {
            stats.push(("TEMP", format!("{:.0}°C", temp.celsius)));
        }
        stats
    }

    /// Cartão inicial: abrir junto com o Windows, o que aparece na ilha, o pet
    /// e de qual sensor vem a temperatura.
    fn draw_welcome(&mut self, ui: &mut Ui, p: &Painter, island: Rect, snap: &Snapshot, accent: Color32) {
        let interactive = self.mode == Mode::Welcome;
        let a = smoothstep(self.welcome.value * 1.6 - 0.4);
        let left = island.left() + PADDING + 4.0;
        let right = island.right() - PADDING - 4.0;
        let top = island.top() + PADDING;

        p.text(pos2(left, top + 10.0), Align2::LEFT_CENTER, "Islandbar", FontId::proportional(17.0), fade(TEXT, a));
        p.text(
            pos2(left, top + 32.0),
            Align2::LEFT_CENTER,
            "Tudo pronto. Escolha como a ilha vai começar:",
            FontId::proportional(12.5),
            fade(TEXT_DIM, a),
        );

        // Linha da opção: clicar em qualquer lugar dela liga/desliga.
        let row = Rect::from_min_max(pos2(left, top + 48.0), pos2(right, top + 80.0));
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

        // O que aparece na ilha fechada e no modo jogo: cada botão liga/desliga.
        let cy = top + 100.0;
        p.text(pos2(left, cy), Align2::LEFT_CENTER, "Mostrar na ilha", FontId::proportional(14.0), fade(TEXT, a));
        let (chip_width, chip_gap) = (46.0, 5.0);
        let shows = [
            ("CPU", self.settings.cpu),
            ("GPU", self.settings.gpu),
            ("RAM", self.settings.ram),
            ("Temp", self.settings.show_temp),
            ("Clima", self.settings.weather),
        ];
        let count = shows.len();
        let mut toggled = None;
        for (i, (label, on)) in shows.into_iter().enumerate() {
            let x = right - (count - i) as f32 * chip_width - (count - 1 - i) as f32 * chip_gap;
            let chip = Rect::from_min_size(pos2(x, cy - 13.0), vec2(chip_width, 26.0));
            if choice_chip(ui, p, ("show", i), chip, label, None, on, accent, a, interactive) {
                toggled = Some(i);
            }
        }
        match toggled {
            Some(0) => self.settings.cpu = !self.settings.cpu,
            Some(1) => self.settings.gpu = !self.settings.gpu,
            Some(2) => self.settings.ram = !self.settings.ram,
            Some(3) => self.settings.show_temp = !self.settings.show_temp,
            Some(4) => self.settings.weather = !self.settings.weather,
            _ => {}
        }

        // O pet: cada botão mostra ele se mexendo (o cartão já desenha a toda velocidade).
        let cy = top + 134.0;
        p.text(pos2(left, cy), Align2::LEFT_CENTER, "Pet", FontId::proportional(14.0), fade(TEXT, a));
        let (pet_width, pet_gap) = (96.0, 6.0);
        let pose = pet::gesture(ui.input(|i| i.time));
        // Com música, os pets do cartão cantam junto.
        let bars = self.bars;
        for (i, pet) in Pet::ALL.into_iter().enumerate() {
            let x = right - (3 - i) as f32 * pet_width - (2 - i) as f32 * pet_gap;
            let chip = Rect::from_min_size(pos2(x, cy - 14.0), vec2(pet_width, 28.0));
            let chosen = self.settings.pet == pet;
            if choice_chip(ui, p, ("pet", i), chip, "", None, chosen, accent, a, interactive) {
                self.settings.pet = pet;
            }
            pet::draw(p, pet, pos2(chip.left() + 20.0, cy), 0.62, &bars, pose, accent, ISLAND, a);
            let name = match pet {
                Pet::Bars => "Barrinhas",
                Pet::Ghost => "Fantasma",
                Pet::Robot => "Robô",
            };
            let color = if chosen { TEXT } else { TEXT_DIM };
            p.text(pos2(chip.left() + 36.0, cy), Align2::LEFT_CENTER, name, FontId::proportional(12.5), fade(color, a));
        }

        // De qual sensor vem a temperatura. Cada botão mostra o valor de agora,
        // para dar para ver qual é qual (o PC pode não ter algum: "n/d").
        p.text(pos2(left, top + 164.0), Align2::LEFT_CENTER, "Temperatura", FontId::proportional(14.0), fade(TEXT, a));
        let sys = &snap.system;
        let column = (right - left - 8.0) / 2.0;
        for (i, choice) in TempChoice::ALL.into_iter().enumerate() {
            let (col, line) = ((i % 2) as f32, (i / 2) as f32);
            let corner = pos2(left + col * (column + 8.0), top + 180.0 + line * 34.0);
            let chip = Rect::from_min_size(corner, vec2(column, 28.0));
            let name = match choice {
                TempChoice::Auto => "Automática",
                TempChoice::Gpu => "Placa de vídeo",
                TempChoice::Board => "Placa-mãe",
                TempChoice::Disk => "SSD / disco",
            };
            let value = sys.temp(choice).map_or_else(|| "n/d".to_string(), |t| format!("{:.0}°C", t.celsius));
            let chosen = self.settings.temp == choice;
            if choice_chip(ui, p, ("temp", i), chip, name, Some(&value), chosen, accent, a, interactive) {
                self.settings.temp = choice;
            }
        }

        // Clima: a cidade encontrada e de onde veio (só um aviso, não dá para clicar).
        let cy = top + 264.0;
        p.text(pos2(left, cy), Align2::LEFT_CENTER, "Clima", FontId::proportional(14.0), fade(TEXT, a));
        let chip = Rect::from_min_max(pos2(right - 230.0, cy - 13.0), pos2(right, cy + 13.0));
        let found = weather::current();
        let (city, from) = match (&found, weather::state()) {
            (Some(now), _) => (
                if now.city.is_empty() { "Sua região" } else { now.city.as_str() },
                match now.source {
                    weather::Source::Windows => "pelo Windows",
                    weather::Source::Internet => "pela internet",
                },
            ),
            (None, weather::State::Offline) => ("Sem internet agora", ""),
            (None, _) => ("Procurando sua cidade…", ""),
        };
        let value = (!from.is_empty()).then_some(from);
        choice_chip(ui, p, ("weather", 0), chip, city, value, found.is_some(), accent, a, false);
        text_fit(
            p,
            pos2(left, top + 289.0),
            Align2::LEFT_CENTER,
            "Pela localização do Windows ou, se ela estiver desligada, pela internet.",
            11.0,
            fade(TEXT_DIM, a),
            right - left,
        );

        // Pedidos do iFood: prepara a extensão do navegador e abre a pasta dela.
        let cy = top + 318.0;
        p.text(pos2(left, cy), Align2::LEFT_CENTER, "Pedidos do iFood", FontId::proportional(14.0), fade(TEXT, a));
        let chip = Rect::from_min_max(pos2(right - 150.0, cy - 13.0), pos2(right, cy + 13.0));
        // Já instalada: o botão vira só um aviso (não dá para clicar).
        let label = match (self.extension_installed, self.extension_opened) {
            (true, _) => "Instalada",
            (false, true) => "Pasta aberta",
            (false, false) => "Instalar extensão",
        };
        let marked = self.extension_installed || self.extension_opened;
        let clickable = interactive && !self.extension_installed;
        if choice_chip(ui, p, ("ifood", 0), chip, label, None, marked, accent, a, clickable) {
            self.extension_opened = true;
            delivery::extension::open();
        }
        let hint: &[&str] = if self.extension_installed {
            &[
                "Pronto: entre uma vez no site do iFood. Depois, com o navegador",
                "aberto, a pílula do pedido aparece sozinha (até sem a aba).",
            ]
        } else if self.extension_opened {
            &[
                "No Chrome/Edge, abra Extensões e ligue o Modo do desenvolvedor.",
                "Clique em Carregar sem compactação e escolha a pasta aberta.",
            ]
        } else {
            &["Com a extensão no navegador, uma pílula ao lado da ilha", "mostra o andamento do seu pedido do iFood."]
        };
        for (i, line) in hint.iter().enumerate() {
            let y = top + 343.0 + i as f32 * 15.0;
            text_fit(p, pos2(left, y), Align2::LEFT_CENTER, line, 11.0, fade(TEXT_DIM, a), right - left);
        }

        // Botão "Iniciar".
        let button = Rect::from_min_max(
            pos2(right - 104.0, island.bottom() - PADDING - 32.0),
            pos2(right, island.bottom() - PADDING),
        );
        text_fit(
            p,
            pos2(left, button.center().y),
            Align2::LEFT_CENTER,
            "Para mudar depois, abra o .exe de novo.",
            11.5,
            fade(TEXT_DIM, a),
            button.left() - 12.0 - left,
        );
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
                self.settings.save();
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
#[derive(Clone, Copy)]
enum MediaIcon {
    Play,
    Pause,
    Next,
    Prev,
}

fn draw_media_icon(p: &Painter, c: Pos2, r: f32, icon: MediaIcon, color: Color32) {
    let triangle = |tip_x: f32, base_x: f32| {
        Shape::convex_polygon(vec![pos2(base_x, c.y - r), pos2(tip_x, c.y), pos2(base_x, c.y + r)], color, Stroke::NONE)
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
                vec![pos2(c.x - dir * r, c.y - r), pos2(c.x + dir * r * 0.6, c.y), pos2(c.x - dir * r, c.y + r)],
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
    let waves = if level > 0.66 {
        3
    } else if level > 0.33 {
        2
    } else if level > 0.0 {
        1
    } else {
        0
    };
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

/// Altura ocupada pela faixinha de periféricos sem fio (0 quando vazia).
fn strip_height(strip_len: usize) -> f32 {
    if strip_len == 0 {
        0.0
    } else {
        let rows = strip_len.min(MAX_STRIP_BATTERIES).div_ceil(BATTERIES_PER_ROW);
        rows as f32 * BATTERY_ROW_HEIGHT + 2.0
    }
}

/// Altura das partes fixas do painel (música, periféricos, números, rodapé).
fn panel_fixed_height(snap: &Snapshot, strip_len: usize) -> f32 {
    let fps_row = if snap.fps_access == FpsAccess::Granted { 0.0 } else { FPS_ROW_HEIGHT };
    PADDING + HEADER_HEIGHT + 14.0 + strip_height(strip_len) + STATS_HEIGHT + fps_row + FOOTER_HEIGHT
}

/// Altura das fileiras de saída de som e microfone.
fn devices_height(snap: &Snapshot) -> f32 {
    let device_rows = |list: &[Device]| list.len().min(MAX_DEVICES).div_ceil(DEVICES_PER_ROW);
    (device_rows(&snap.outputs) + device_rows(&snap.inputs)) as f32 * DEVICE_ROW_HEIGHT
}

/// Altura da página Som: dispositivos + "Geral" + apps.
fn sound_page_height(snap: &Snapshot, strip_len: usize) -> f32 {
    devices_height(snap) + (1 + apps_shown(snap, strip_len)) as f32 * ROW_HEIGHT
}

/// Quantos apps cabem no mixer (além da linha "Geral").
fn apps_shown(snap: &Snapshot, strip_len: usize) -> usize {
    let room = (PANEL_MAX_HEIGHT - panel_fixed_height(snap, strip_len) - devices_height(snap)) / ROW_HEIGHT - 1.0;
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

/// A cidade veio pela internet: a página do clima mostra o botão para usar
/// a localização do Windows (mais precisa).
fn weather_uses_internet() -> bool {
    weather::current().is_some_and(|now| now.source == weather::Source::Internet)
}

/// Altura da página do clima (com o botão da localização, quando ele aparece).
fn weather_page_height() -> f32 {
    WEATHER_PAGE_HEIGHT + if weather_uses_internet() { WEATHER_BUTTON_HEIGHT } else { 0.0 }
}

/// Página do clima: agora, os detalhes e as próximas horas.
#[allow(clippy::too_many_arguments)]
fn draw_weather_page(
    ui: &mut Ui,
    p: &Painter,
    left: f32,
    right: f32,
    top: f32,
    accent: Color32,
    a: f32,
    interactive: bool,
) {
    let Some(now) = weather::current() else {
        let text = match weather::state() {
            weather::State::Offline => "Sem internet para buscar o clima",
            _ => "Procurando o clima da sua cidade…",
        };
        let center = pos2((left + right) / 2.0, top + WEATHER_PAGE_HEIGHT / 2.0 - 8.0);
        p.text(center, Align2::CENTER_CENTER, text, FontId::proportional(13.0), fade(TEXT_DIM, a));
        return;
    };

    // ---- Agora: ícone, temperatura, como está o tempo; cidade e máx/mín à direita ----
    let cy = top + 36.0;
    weather::draw_icon(p, pos2(left + 26.0, cy), 52.0, now.sky(), now.day, ISLAND, a);
    let temp = p.text(
        pos2(left + 60.0, cy + 1.0),
        Align2::LEFT_CENTER,
        format!("{:.0}°", now.temp),
        FontId::proportional(40.0),
        fade(TEXT, a),
    );
    let side_right = right;
    let max_min = p.text(
        pos2(side_right, cy + 9.0),
        Align2::RIGHT_CENTER,
        format!("Máx {:.0}° · Mín {:.0}°", now.max, now.min),
        FontId::proportional(12.0),
        fade(TEXT_DIM, a),
    );
    // A coluna da direita cabe o "Máx · Mín" e a cidade (com o alfinete), até 130 pontos.
    let city_width = if now.city.is_empty() {
        0.0
    } else {
        p.layout_no_wrap(now.city.clone(), FontId::proportional(12.0), TEXT_DIM).size().x.min(116.0)
    };
    let column_left = max_min.left().min(side_right - city_width - 14.0);
    let text_x = temp.right() + 12.0;
    let text_room = column_left - 10.0 - text_x;
    text_fit(p, pos2(text_x, cy - 8.0), Align2::LEFT_CENTER, now.description(), 13.5, fade(TEXT, a), text_room);
    text_fit(
        p,
        pos2(text_x, cy + 9.0),
        Align2::LEFT_CENTER,
        &format!("Sensação de {:.0}°", now.feels),
        12.0,
        fade(TEXT_DIM, a),
        text_room,
    );
    if !now.city.is_empty() {
        text_fit(
            p,
            pos2(side_right, cy - 8.0),
            Align2::RIGHT_CENTER,
            &now.city,
            12.0,
            fade(TEXT_DIM, a),
            city_width + 0.5,
        );
        weather::draw_pin(p, pos2(side_right - city_width - 7.0, cy - 8.0), 11.0, fade(TEXT_DIM, a), ISLAND);
    }

    // ---- Quadradinhos: umidade, vento, chuva, índice UV ----
    let percent = |value: Option<f32>| value.map_or_else(|| "n/d".to_string(), |v| format!("{v:.0}%"));
    let tiles = [
        (Detail::Humidity, "Umidade", format!("{:.0}%", now.humidity)),
        (Detail::Wind, "Vento", format!("{:.0} km/h", now.wind)),
        (Detail::Rain, "Chuva", percent(now.rain)),
        (
            Detail::Uv,
            "Índice UV",
            now.uv.map_or_else(|| "n/d".to_string(), |uv| format!("{uv:.0} {}", weather::uv_word(uv))),
        ),
    ];
    let gap = 8.0;
    let width = (right - left - gap * 3.0) / 4.0;
    for (i, (detail, label, value)) in tiles.into_iter().enumerate() {
        let rect = Rect::from_min_size(pos2(left + i as f32 * (width + gap), top + 76.0), vec2(width, 44.0));
        p.rect_filled(rect, 12.0, fade(Color32::WHITE, a * 0.05));
        weather::draw_detail(p, pos2(rect.left() + 17.0, rect.center().y), 17.0, detail, a);
        let x = rect.left() + 31.0;
        p.text(
            pos2(x, rect.center().y - 8.0),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(10.5),
            fade(TEXT_DIM, a),
        );
        // Sem espaço para tudo (ex.: "9 muito alto"): fica só o número.
        let room = rect.right() - 4.0 - x;
        let fits = p.layout_no_wrap(value.clone(), FontId::proportional(12.5), TEXT).size().x <= room;
        let value = if fits { value.as_str() } else { value.split(' ').next().unwrap_or_default() };
        text_fit(p, pos2(x, rect.center().y + 8.0), Align2::LEFT_CENTER, value, 12.5, fade(TEXT, a), room);
    }

    // ---- Próximas horas: "Agora" e as cinco seguintes ----
    let columns = 1 + now.hours.len().min(weather::HOURS);
    let column = (right - left) / (1 + weather::HOURS) as f32;
    for i in 0..columns {
        let x = left + column * (i as f32 + 0.5);
        let (when, sky, day, temp, rain) = match i {
            0 => ("Agora".to_string(), now.sky(), now.day, now.temp, None),
            _ => {
                let hour = &now.hours[i - 1];
                (format!("{}h", hour.hour), weather::sky(hour.code), hour.day, hour.temp, hour.rain)
            }
        };
        p.text(pos2(x, top + 138.0), Align2::CENTER_CENTER, when, FontId::proportional(11.0), fade(TEXT_DIM, a));
        weather::draw_icon(p, pos2(x, top + 157.0), 20.0, sky, day, ISLAND, a);
        // A chance de chuva só aparece quando vale a pena levar guarda-chuva.
        if let Some(rain) = rain.filter(|r| *r >= 30.0) {
            p.text(
                pos2(x, top + 173.0),
                Align2::CENTER_CENTER,
                format!("{rain:.0}%"),
                FontId::proportional(10.0),
                fade(weather::RAIN, a),
            );
        }
        p.text(
            pos2(x, top + 188.0),
            Align2::CENTER_CENTER,
            format!("{temp:.0}°"),
            FontId::proportional(12.5),
            fade(TEXT, a),
        );
    }

    // ---- Cidade pela internet: botão para usar a localização do Windows ----
    // A janela do Windows perguntando só abre quando você clica aqui.
    if now.source == weather::Source::Internet {
        let permission = weather::permission();
        let label = match permission {
            weather::Permission::Idle => "Usar localização do Windows",
            weather::Permission::Asking => "Responda à janela do Windows…",
            weather::Permission::Denied => "Ligar a localização nas Configurações",
        };
        let rect = Rect::from_center_size(pos2((left + right) / 2.0, top + 214.0), vec2(240.0, 26.0));
        let clickable = interactive && permission != weather::Permission::Asking;
        if text_button(ui, p, "weather-location", rect, label, false, accent, a, clickable) {
            match permission {
                weather::Permission::Denied => weather::open_settings(),
                _ => weather::request_permission(),
            }
        }
    }
}

/// Botão de escolha do cartão inicial: nome à esquerda (centralizado quando
/// não há `value`) e um valor à direita. Marcado, fica com a cor de destaque.
#[allow(clippy::too_many_arguments)]
fn choice_chip(
    ui: &mut Ui,
    p: &Painter,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    rect: Rect,
    label: &str,
    value: Option<&str>,
    chosen: bool,
    accent: Color32,
    a: f32,
    interactive: bool,
) -> bool {
    let id = ui.scope_id().with(id_source);
    let resp = interactive.then(|| ui.interact(rect, id, Sense::click()));
    let hovered = resp.as_ref().is_some_and(|r| r.hovered());
    let hover = ui.ctx().animate_bool_with_time(id, hovered, 0.12);
    let radius = rect.height() / 2.0;
    if chosen {
        p.rect_filled(rect, radius, fade(accent, a * (0.26 + 0.08 * hover)));
        p.rect_stroke(rect, radius, Stroke::new(1.0, fade(accent, a * 0.7)), StrokeKind::Inside);
    } else {
        p.rect_filled(rect, radius, fade(Color32::WHITE, a * (0.06 + 0.08 * hover)));
    }
    let color = if chosen || hover > 0.5 { TEXT } else { TEXT_DIM };
    match value {
        Some(value) => {
            let value_rect = p.text(
                pos2(rect.right() - 12.0, rect.center().y),
                Align2::RIGHT_CENTER,
                value,
                FontId::proportional(12.0),
                fade(color, a * 0.85),
            );
            let x = rect.left() + 12.0;
            text_fit(
                p,
                pos2(x, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                12.5,
                fade(color, a),
                value_rect.left() - 8.0 - x,
            );
        }
        None => {
            p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.5), fade(color, a));
        }
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

/// A previsão de entrega ("20:10-20:25") já passou? Vale o último horário.
fn eta_passed(eta: &str) -> bool {
    use chrono::Timelike;
    let Some((h, m)) = eta.rsplit('-').next().and_then(|t| t.split_once(':')) else { return false };
    let (Ok(h), Ok(m)) = (h.parse::<i32>(), m.parse::<i32>()) else { return false };
    let now = chrono::Local::now();
    let late = (now.hour() as i32 * 60 + now.minute() as i32 - (h * 60 + m)).rem_euclid(24 * 60);
    // Até 12 h "depois" é atraso; mais que isso é a previsão de amanhã (ou de antes da meia-noite).
    late > 0 && late < 12 * 60
}

/// Logo do iFood: "iFood" em branco num quadradinho vermelho. Devolve a largura.
fn ifood_logo_width(p: &Painter, scale: f32) -> f32 {
    p.layout_no_wrap("iFood".to_string(), FontId::proportional(11.5 * scale), Color32::WHITE).size().x + 10.0 * scale
}

/// Desenha o logo com a ponta esquerda no meio da altura em `left_center`.
fn draw_ifood_logo(p: &Painter, left_center: Pos2, scale: f32, a: f32) {
    let galley = p.layout_no_wrap("iFood".to_string(), FontId::proportional(11.5 * scale), Color32::WHITE);
    let size = vec2(galley.size().x + 10.0 * scale, 18.0 * scale);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - size.y / 2.0), size);
    p.rect_filled(rect, 5.0 * scale, fade(FOOD, a));
    let pos = rect.center() - galley.size() / 2.0;
    // "Negrito": a fonte da ilha é fininha, então o texto vai duas vezes, meio pixel ao lado.
    p.galley_with_override_text_color(pos, galley.clone(), fade(Color32::WHITE, a));
    p.galley_with_override_text_color(pos + vec2(0.55 * scale, 0.0), galley, fade(Color32::WHITE, a));
}

/// Balãozinho de conversa só com o contorno e três pontinhos.
fn draw_chat(p: &Painter, c: Pos2, color: Color32) {
    let bubble = Rect::from_center_size(c - vec2(0.0, 1.0), vec2(14.0, 10.0));
    let stroke = Stroke::new(1.2, color);
    p.rect_stroke(bubble, 3.5, stroke, StrokeKind::Inside);
    let tail = vec![
        pos2(bubble.left() + 3.5, bubble.bottom() - 0.6),
        pos2(bubble.left() + 2.5, bubble.bottom() + 2.8),
        pos2(bubble.left() + 6.5, bubble.bottom() - 0.6),
    ];
    p.add(Shape::line(tail, stroke));
    for dx in [-3.0, 0.0, 3.0] {
        p.circle_filled(bubble.center() + vec2(dx, 0.0), 0.95, color);
    }
}

/// Altura da janela do pedido (com ou sem mapa).
fn order_panel_height(order: Option<&Order>) -> f32 {
    let has_map = order.is_some_and(|o| o.from.is_some() || o.to.is_some() || o.courier.is_some());
    let middle = if has_map { ORDER_MAP_HEIGHT } else { ORDER_ROUTE_HEIGHT };
    PADDING + ORDER_TOP + middle + 12.0 + 28.0 + 14.0 + 30.0 + PADDING
}

/// Quantos passos do andamento já aconteceram (-1 = nenhum).
fn order_step(stage: Stage) -> i32 {
    match stage {
        Stage::Unknown | Stage::Placed | Stage::Cancelled | Stage::Login => -1,
        Stage::Confirmed => 0,
        Stage::Preparing | Stage::Driver | Stage::Ready | Stage::Pickup | Stage::Collected => 1,
        Stage::Shipping | Stage::Arrived => 2,
        Stage::Delivered => 3,
    }
}

/// Linha do andamento: Confirmado → Em preparo → A caminho → Entregue.
fn draw_order_steps(p: &Painter, left: f32, right: f32, cy: f32, stage: Stage, accent: Color32, a: f32) {
    let names = ["Confirmado", "Em preparo", "A caminho", "Entregue"];
    let done = order_step(stage);
    let (start, end) = (left + 5.0, right - 5.0);
    let x = |i: usize| start + (end - start) * i as f32 / 3.0;
    p.line_segment([pos2(start, cy), pos2(end, cy)], Stroke::new(2.0, fade(Color32::WHITE, a * 0.12)));
    if done > 0 {
        p.line_segment([pos2(start, cy), pos2(x(done as usize), cy)], Stroke::new(2.0, fade(accent, a)));
    }
    for (i, name) in names.iter().enumerate() {
        let reached = i as i32 <= done;
        if reached {
            p.circle_filled(pos2(x(i), cy), 5.0, fade(accent, a));
        } else {
            p.circle_filled(pos2(x(i), cy), 4.0, fade(Color32::from_rgb(60, 60, 66), a));
        }
        let anchor = match i {
            0 => Align2::LEFT_CENTER,
            3 => Align2::RIGHT_CENTER,
            _ => Align2::CENTER_CENTER,
        };
        let label_x = match i {
            0 => start - 5.0,
            3 => end + 5.0,
            _ => x(i),
        };
        let color = if reached { TEXT } else { TEXT_DIM };
        p.text(pos2(label_x, cy + 15.0), anchor, *name, FontId::proportional(10.5), fade(color, a));
    }
}

/// Sem os pontos do mapa: desenho loja → você, com o entregador pela etapa.
fn draw_route(p: &Painter, area: Rect, stage: Stage, accent: Color32, a: f32, time: f32) {
    p.rect_filled(area, 12.0, fade(Color32::WHITE, a * 0.04));
    let cy = area.center().y - 7.0;
    let (start, end) = (area.left() + 36.0, area.right() - 36.0);
    let progress = match stage {
        Stage::Collected => 0.1,
        Stage::Shipping => 0.55,
        Stage::Arrived | Stage::Delivered => 1.0,
        _ => 0.0,
    };
    p.line_segment([pos2(start, cy), pos2(end, cy)], Stroke::new(2.0, fade(Color32::WHITE, a * 0.14)));
    let courier = pos2(start + (end - start) * progress, cy);
    if progress > 0.0 {
        p.line_segment([pos2(start, cy), courier], Stroke::new(2.0, fade(accent, a)));
    }
    draw_store_pin(p, pos2(start, cy), a);
    draw_home_pin(p, pos2(end, cy), a);
    // O entregador aparece depois de pegar o pedido (antes disso ele está na loja).
    if progress > 0.0 && progress < 1.0 {
        draw_courier_pin(p, courier, accent, a, time);
    }
    for (x, label) in [(start, "Loja"), (end, "Você")] {
        p.text(pos2(x, cy + 20.0), Align2::CENTER_CENTER, label, FontId::proportional(10.5), fade(TEXT_DIM, a));
    }
}

/// Marcador da loja: bolinha laranja com uma sacolinha.
fn draw_store_pin(p: &Painter, c: Pos2, a: f32) {
    p.circle_filled(c, 9.0, fade(Color32::from_rgb(255, 159, 10), a));
    p.circle_stroke(c, 9.0, Stroke::new(1.5, fade(Color32::WHITE, a)));
    let body = Rect::from_center_size(c + vec2(0.0, 1.2), vec2(7.0, 5.5));
    p.rect_filled(body, 1.2, fade(Color32::WHITE, a));
    let handle: Vec<Pos2> = (0..=8)
        .map(|i| {
            let angle = std::f32::consts::PI * i as f32 / 8.0;
            pos2(c.x + 1.8 * angle.cos(), body.top() + 0.3 - 2.0 * angle.sin())
        })
        .collect();
    p.add(Shape::line(handle, Stroke::new(1.0, fade(Color32::WHITE, a))));
}

/// Marcador da sua casa: bolinha branca com uma casinha.
fn draw_home_pin(p: &Painter, c: Pos2, a: f32) {
    p.circle_filled(c, 9.0, fade(TEXT, a));
    let color = fade(ISLAND, a);
    let roof = vec![c + vec2(-4.5, -0.5), c + vec2(0.0, -4.8), c + vec2(4.5, -0.5)];
    p.add(Shape::convex_polygon(roof, color, Stroke::NONE));
    p.rect_filled(Rect::from_min_max(c + vec2(-3.2, -0.8), c + vec2(3.2, 4.0)), 0.5, color);
}

/// Marcador do entregador: bolinha na cor de destaque, com um brilho pulsando.
fn draw_courier_pin(p: &Painter, c: Pos2, accent: Color32, a: f32, time: f32) {
    let pulse = (time * 1.6).fract();
    p.circle_filled(c, 7.0 + 9.0 * pulse, fade(accent, a * 0.35 * (1.0 - pulse)));
    p.circle_filled(c, 7.0, fade(accent, a));
    p.circle_stroke(c, 7.0, Stroke::new(2.0, fade(Color32::WHITE, a)));
}
