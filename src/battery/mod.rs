//! Bateria dos periféricos sem fio (mouse, teclado, controle, fone e microfone).
//!
//! Lê dispositivos Bluetooth (propriedade de bateria do Windows), controles
//! XInput/PlayStation/8BitDo e receptores USB 2.4 GHz (HID padrão, Logitech
//! HID++, Razer, HyperX, Compx/ATK/VXE/Kysona/Pulsar/Redragon, Rongyuan/Attack
//! Shark, SteelSeries e Corsair).
//!
//! Para a ilha continuar super leve e nunca atrapalhar partidas:
//! - Roda numa thread separada que passa quase todo o tempo dormindo: a cada
//!   15 s com o painel aberto e a cada 60 s com a pílula fechada.
//! - Durante um jogo em tela cheia (com o painel fechado), não consulta nenhum
//!   receptor USB nem mostra alertas na pílula.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind};

use crate::backend::Device;

#[cfg(windows)]
mod windows;

/// Com o painel aberto, atualiza mais de perto para mostrar se começou a carregar.
const OPEN_EVERY: Duration = Duration::from_secs(15);
/// Com a pílula fechada, 1 vez por minuto basta para vigiar bateria fraca.
const CLOSED_EVERY: Duration = Duration::from_secs(60);
/// Mouses e controles 2.4 GHz adormecem parados; enquanto o receptor USB
/// continuar espetado no PC, guardamos a última leitura por até 20 minutos.
const SLEEP_GRACE: Duration = Duration::from_secs(20 * 60);

/// Níveis de alerta na pílula fechada.
pub const LOW_THRESHOLD: u8 = 20;
pub const CRITICAL_THRESHOLD: u8 = 10;
const RESET_THRESHOLD: u8 = 25;

const AMBER: Color32 = Color32::from_rgb(255, 179, 64);
const RED: Color32 = Color32::from_rgb(255, 89, 82);
const GREEN: Color32 = Color32::from_rgb(76, 217, 100);

/// Categoria do periférico sem fio.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Mouse,
    Keyboard,
    Gamepad,
    Headset,
    Microphone,
}

impl Kind {
    pub fn label_pt(self) -> &'static str {
        match self {
            Self::Mouse => "Mouse",
            Self::Keyboard => "Teclado",
            Self::Gamepad => "Controle",
            Self::Headset => "Fone",
            Self::Microphone => "Microfone",
        }
    }
}

/// Um periférico sem fio conectado com nível de bateria conhecido.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peripheral {
    /// Identificador estável (para não repetir avisos de bateria fraca).
    pub id: String,
    /// Nome limpo do periférico (ex.: "8BitDo Ultimate 2", "Logitech G Pro X").
    pub name: String,
    pub kind: Kind,
    /// Porcentagem de bateria, 0..=100.
    pub percent: u8,
    pub charging: bool,
}

/// Leitura bruta vinda do sistema antes de aplicar o cache de repouso.
#[derive(Clone, Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct RawReading {
    pub peripheral: Peripheral,
    /// Identificador do receptor USB (`vid:pid`), se vier de um dongle 2.4 GHz.
    /// Quando o dongle é tirado do PC, o cache desse dispositivo some na hora.
    pub receiver_tag: Option<u32>,
    /// O receptor respondeu que o periférico está adormecido (sem % agora).
    pub asleep: bool,
}

#[derive(Default)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct PollOutput {
    pub readings: Vec<RawReading>,
    /// Todos os pares `(vid << 16) | pid` presentes no barramento HID agora.
    pub present_tags: HashSet<u32>,
}

/// Guarda a última porcentagem de mouses/controles que adormeceram no dongle.
#[derive(Default)]
struct GraceCache {
    entries: HashMap<String, CachedEntry>,
}

struct CachedEntry {
    peripheral: Peripheral,
    receiver_tag: u32,
    seen_at: Instant,
}

impl GraceCache {
    fn resolve(&mut self, now: Instant, output: PollOutput) -> Vec<Peripheral> {
        // Se o receptor USB foi desconectado do PC, descarta o cache dele na hora.
        self.entries.retain(|_, entry| {
            output.present_tags.contains(&entry.receiver_tag)
                && now.saturating_duration_since(entry.seen_at) <= SLEEP_GRACE
        });

        let mut list: Vec<Peripheral> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();

        for item in output.readings {
            if item.asleep {
                if let Some(cached) = self.entries.get(&item.peripheral.id) {
                    if seen_ids.insert(cached.peripheral.id.clone()) {
                        list.push(cached.peripheral.clone());
                    }
                }
                continue;
            }
            if let Some(tag) = item.receiver_tag {
                self.entries.insert(
                    item.peripheral.id.clone(),
                    CachedEntry { peripheral: item.peripheral.clone(), receiver_tag: tag, seen_at: now },
                );
            }
            if seen_ids.insert(item.peripheral.id.clone()) {
                list.push(item.peripheral);
            }
        }

        // Mantém também dispositivos de receptor cujo dongle segue espetado, mas
        // não respondeu nesta volta por estar em economia de energia.
        for (id, entry) in &self.entries {
            if !seen_ids.contains(id) && output.present_tags.contains(&entry.receiver_tag) {
                seen_ids.insert(id.clone());
                list.push(entry.peripheral.clone());
            }
        }

        list.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
        list
    }
}

static NOW: Mutex<Option<Arc<[Peripheral]>>> = Mutex::new(None);
static PANEL_OPEN: AtomicBool = AtomicBool::new(false);
static IN_GAME: AtomicBool = AtomicBool::new(false);
static WAKE_WORKER: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();

fn worker_signal() -> &'static (Mutex<bool>, Condvar) {
    WAKE_WORKER.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

/// Diagnóstico detalhado no stderr (`ISLANDBAR_DEBUG_BATTERY=1`).
pub fn debug_enabled() -> bool {
    matches!(std::env::var("ISLANDBAR_DEBUG_BATTERY").as_deref(), Ok("1" | "true" | "debug"))
}

/// Lista atual de periféricos sem fio (sem bloquear nem alocar por quadro).
pub fn current() -> Arc<[Peripheral]> {
    NOW.lock().ok().and_then(|guard| guard.clone()).unwrap_or_else(|| Arc::from(&[][..]))
}

/// Avisa a thread de bateria se o painel está aberto e se há jogo em tela cheia.
pub fn set_context(panel_open: bool, in_game: bool) {
    let was_open = PANEL_OPEN.swap(panel_open, Ordering::Relaxed);
    let was_game = IN_GAME.swap(in_game, Ordering::Relaxed);
    // Ao abrir o painel (ou sair de um jogo), acorda a leitura na hora.
    if (panel_open && !was_open) || (!in_game && was_game) {
        let (lock, cvar) = worker_signal();
        if let Ok(mut pending) = lock.lock() {
            *pending = true;
            cvar.notify_one();
        }
    }
}

/// Inicia a leitura em segundo plano.
pub fn start(wake_ui: impl Fn() + Send + 'static) {
    let _ = worker_signal();
    let _ = std::thread::Builder::new().name("islandbar-battery".into()).spawn(move || worker_loop(wake_ui));
}

fn worker_loop(wake_ui: impl Fn()) {
    let debug_mode = std::env::var("ISLANDBAR_DEBUG_BATTERY").ok();
    let mut cache = GraceCache::default();
    let mut previous: Vec<Peripheral> = Vec::new();

    loop {
        let panel = PANEL_OPEN.load(Ordering::Relaxed);
        let in_game = IN_GAME.load(Ordering::Relaxed);

        // Em jogo de tela cheia com o painel fechado, não mexe em nenhum USB HID.
        if !panel && in_game && debug_mode.is_none() {
            wait_next(CLOSED_EVERY);
            continue;
        }

        let started = Instant::now();
        let next = match debug_mode.as_deref() {
            Some("fake") => fake_peripherals(false),
            Some("critical") => fake_peripherals(true),
            _ => {
                let output = poll_platform();
                cache.resolve(Instant::now(), output)
            }
        };
        crate::perf::add_section("battery", started);

        if debug_enabled() {
            crate::perf::line(format!("battery: {} dispositivo(s): {next:?}", next.len()));
        }

        if next != previous {
            previous.clone_from(&next);
            if let Ok(mut slot) = NOW.lock() {
                *slot = Some(Arc::from(next.into_boxed_slice()));
            }
            wake_ui();
        }

        let sleep_for = if PANEL_OPEN.load(Ordering::Relaxed) { OPEN_EVERY } else { CLOSED_EVERY };
        wait_next(sleep_for);
    }
}

fn wait_next(timeout: Duration) {
    let (lock, cvar) = worker_signal();
    let Ok(mut pending) = lock.lock() else {
        std::thread::sleep(timeout);
        return;
    };
    if !*pending {
        if let Ok((guard, _)) = cvar.wait_timeout(pending, timeout) {
            pending = guard;
        } else {
            return;
        }
    }
    *pending = false;
}

fn poll_platform() -> PollOutput {
    #[cfg(windows)]
    {
        windows::poll()
    }
    #[cfg(not(windows))]
    {
        PollOutput::default()
    }
}

fn fake_peripherals(critical: bool) -> Vec<Peripheral> {
    vec![
        Peripheral {
            id: "mock:mouse".into(),
            name: "Logitech G Pro X".into(),
            kind: Kind::Mouse,
            percent: 64,
            charging: false,
        },
        Peripheral {
            id: "mock:keyboard".into(),
            name: "Keychron K2".into(),
            kind: Kind::Keyboard,
            percent: 92,
            charging: true,
        },
        Peripheral {
            id: "mock:gamepad".into(),
            name: "8BitDo Ultimate".into(),
            kind: Kind::Gamepad,
            percent: if critical { 9 } else { 18 },
            charging: false,
        },
        Peripheral {
            id: "mock:headset".into(),
            name: "HyperX Cloud III".into(),
            kind: Kind::Headset,
            percent: 78,
            charging: false,
        },
    ]
}

// ---------------------------------------------------------------------------
// Casamento com dispositivos de Som + Faixinha do painel
// ---------------------------------------------------------------------------

/// Resultado da separação dos periféricos para o painel expandido:
/// - `output_badges[i]`: bateria correspondente a `outputs[i]` na página Som;
/// - `input_badges[i]`: bateria correspondente a `inputs[i]` na página Som;
/// - `strip`: mouse, teclado, controle e qualquer fone/microfone sem fio que
///   não tenha casado com um botão da página Som.
pub struct PanelBatteries {
    pub output_badges: Vec<Option<Peripheral>>,
    pub input_badges: Vec<Option<Peripheral>>,
    pub strip: Vec<Peripheral>,
}

pub fn match_panel_batteries(peripherals: &[Peripheral], outputs: &[Device], inputs: &[Device]) -> PanelBatteries {
    let mut output_badges: Vec<Option<Peripheral>> = vec![None; outputs.len()];
    let mut input_badges: Vec<Option<Peripheral>> = vec![None; inputs.len()];
    let mut strip = Vec::new();

    for p in peripherals {
        match p.kind {
            Kind::Headset => {
                // No modo de demonstração/teste visual, casa com a primeira saída.
                if p.id == "mock:headset" && !outputs.is_empty() && output_badges[0].is_none() {
                    output_badges[0] = Some(p.clone());
                    continue;
                }
                if let Some(idx) = best_audio_match(&p.name, outputs, &output_badges) {
                    output_badges[idx] = Some(p.clone());
                } else if let Some(idx) = best_audio_match(&p.name, inputs, &input_badges) {
                    input_badges[idx] = Some(p.clone());
                } else {
                    strip.push(p.clone());
                }
            }
            Kind::Microphone => {
                if let Some(idx) = best_audio_match(&p.name, inputs, &input_badges) {
                    input_badges[idx] = Some(p.clone());
                } else {
                    strip.push(p.clone());
                }
            }
            Kind::Mouse | Kind::Keyboard | Kind::Gamepad => {
                strip.push(p.clone());
            }
        }
    }

    PanelBatteries { output_badges, input_badges, strip }
}

fn best_audio_match(
    peripheral_name: &str,
    devices: &[Device],
    already_matched: &[Option<Peripheral>],
) -> Option<usize> {
    let p_tokens = meaningful_audio_tokens(peripheral_name);
    if p_tokens.is_empty() {
        return None;
    }

    let mut best: Option<(usize, usize)> = None;
    for (idx, dev) in devices.iter().enumerate() {
        if already_matched[idx].is_some() {
            continue;
        }
        let d_tokens = meaningful_audio_tokens(&dev.name);
        if d_tokens.is_empty() {
            continue;
        }
        let score = p_tokens
            .iter()
            .filter(|pt| {
                d_tokens.iter().any(|dt| {
                    dt == *pt
                        || (pt.len() >= 4 && dt.contains(pt.as_str()))
                        || (dt.len() >= 4 && pt.contains(dt.as_str()))
                })
            })
            .count();
        if score > 0 && best.is_none_or(|(_, best_score)| score > best_score) {
            best = Some((idx, score));
        }
    }
    best.map(|(idx, _)| idx)
}

/// Extrai só palavras de marca/modelo, ignorando termos genéricos do Windows
/// (para nunca confundir um fone Bluetooth com "Fones de ouvido (Realtek Audio)").
fn meaningful_audio_tokens(text: &str) -> Vec<String> {
    const GENERIC: &[&str] = &[
        "fones",
        "fone",
        "ouvido",
        "de",
        "do",
        "da",
        "alto",
        "falantes",
        "alto-falantes",
        "microfone",
        "headset",
        "headphones",
        "headphone",
        "earphone",
        "earphones",
        "earbuds",
        "speakers",
        "speaker",
        "microphone",
        "wireless",
        "gaming",
        "audio",
        "stereo",
        "hands",
        "free",
        "ag",
        "usb",
        "bluetooth",
        "dongle",
        "receiver",
        "2",
        "4g",
        "24g",
        "high",
        "definition",
        "realtek",
        "nvidia",
        "amd",
        "intel",
        "padrao",
        "default",
        "chat",
        "game",
        "sound",
        "device",
        "dispositivo",
        "teste",
    ];

    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|tok| tok.len() >= 2 && !GENERIC.contains(tok))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// Aviso discreto de bateria fraca na pílula fechada
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum WarnStage {
    None,
    Low20,
    Critical10,
}

/// Alerta rápido exibido na pílula quando a bateria chega a 20% ou 10%.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowBatteryAlert {
    pub name: String,
    pub kind: Kind,
    pub percent: u8,
}

#[derive(Default)]
pub struct AlertTracker {
    warned: HashMap<String, WarnStage>,
}

impl AlertTracker {
    /// Verifica se algum periférico acabou de cruzar ≤ 20% ou ≤ 10%.
    /// Se `can_notify` for `false` (ex.: durante um jogo em tela cheia), não
    /// consome o aviso: deixa para avisar quando o jogo fechar.
    pub fn poll(&mut self, peripherals: &[Peripheral], can_notify: bool) -> Option<LowBatteryAlert> {
        for p in peripherals {
            if p.charging || p.percent >= RESET_THRESHOLD {
                self.warned.insert(p.id.clone(), WarnStage::None);
            }
        }

        if !can_notify {
            return None;
        }

        for p in peripherals {
            if p.charging {
                continue;
            }
            let prev = self.warned.get(&p.id).copied().unwrap_or(WarnStage::None);
            let target = if p.percent <= CRITICAL_THRESHOLD {
                WarnStage::Critical10
            } else if p.percent <= LOW_THRESHOLD {
                WarnStage::Low20
            } else {
                WarnStage::None
            };
            if target > prev {
                self.warned.insert(p.id.clone(), target);
                return Some(LowBatteryAlert {
                    name: short_peripheral_name(&p.name, p.kind),
                    kind: p.kind,
                    percent: p.percent,
                });
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Utilitários compartilhados de nome e protocolo
// ---------------------------------------------------------------------------

/// Encurta nomes muito longos para caber bonito na faixinha e no aviso.
pub fn short_peripheral_name(name: &str, kind: Kind) -> String {
    let mut s = strip_receiver_suffix(name.trim());
    for noisy in ["Wireless Gaming ", "Wireless ", "Gaming ", "Bluetooth "] {
        if s.len() > noisy.len() + 4 {
            s = s.replace(noisy, "");
        }
    }
    let s = s.trim();
    if s.is_empty() {
        kind.label_pt().to_string()
    } else {
        s.to_string()
    }
}

pub(crate) fn strip_receiver_suffix(name: &str) -> String {
    let mut s = name.trim().to_string();
    for suffix in [
        " Wireless Receiver",
        " USB Receiver",
        " 2.4G Receiver",
        " 2.4G Dongle",
        " 1K Dongle",
        " 4K Dongle",
        " 8K Dongle",
        " Receiver",
        " Dongle",
        " 2.4G",
    ] {
        if s.len() > suffix.len() && s[..s.len()].to_lowercase().ends_with(&suffix.to_lowercase()) {
            s.truncate(s.len() - suffix.len());
            s = s.trim_end().to_string();
        }
    }
    s
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn strip_bt_service_suffix(name: &str) -> String {
    let mut s = name.trim().to_string();
    for suffix in [" Hands-Free AG Audio", " Hands-Free AG", " Hands-Free", " Stereo", " Avrcp Transport"] {
        if s.len() > suffix.len() && s.to_lowercase().ends_with(&suffix.to_lowercase()) {
            s.truncate(s.len() - suffix.len());
            s = s.trim_end().to_string();
        }
    }
    s
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn guess_kind_from_name(name: &str) -> Option<Kind> {
    let low = name.to_lowercase();
    if low.contains("mic") || low.contains("yeti") || low.contains("quadcast") || low.contains("wave:") {
        return Some(Kind::Microphone);
    }
    if [
        "headset",
        "headphone",
        "earbud",
        "airpods",
        "buds",
        "arctis",
        "cloud",
        "blackshark",
        "kraken",
        "barracuda",
        "wh-1000",
        "wf-1000",
        "pulse 3d",
        "zone ",
        "g733",
        "g535",
        "g435",
        "hs80",
        "virtuoso",
        "void ",
    ]
    .iter()
    .any(|k| low.contains(k))
    {
        return Some(Kind::Headset);
    }
    if [
        "controller",
        "gamepad",
        "8bitdo",
        "xbox",
        "dualsense",
        "dualshock",
        "joy-con",
        "pro controller",
        "wireless pad",
    ]
    .iter()
    .any(|k| low.contains(k))
    {
        return Some(Kind::Gamepad);
    }
    if [
        "keyboard",
        "teclado",
        "keychron",
        "nuphy",
        "blackwidow",
        "deathstalker",
        "huntsman",
        "g915",
        "g715",
        "mx keys",
        "k2",
        "k3",
        "k6",
        "k8",
        "anne pro",
        "akko",
        "aula",
    ]
    .iter()
    .any(|k| low.contains(k))
    {
        return Some(Kind::Keyboard);
    }
    if [
        "mouse",
        "superlight",
        "viper",
        "deathadder",
        "basilisk",
        "naga",
        "orochi",
        "aerox",
        "rival",
        "gladius",
        "harpe",
        "pulsar",
        "kysona",
        "dragonfly",
        "attack shark",
        "mx master",
        "mx anywhere",
        "lift",
        "g305",
        "g502",
        "g703",
        "g pro",
        "scimitar",
        "dark core",
        "katar",
        "haste",
    ]
    .iter()
    .any(|k| low.contains(k))
    {
        return Some(Kind::Mouse);
    }
    None
}

/// Checksum de 17 bytes usado pelos receptores Compx (ATK, VXE, Kysona, Pulsar, Redragon).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn compx_checksum(buf: &[u8]) -> u8 {
    let sum = buf.iter().take(16).fold(0u8, |acc, &b| acc.wrapping_add(b));
    0x55u8.wrapping_sub(sum)
}

/// Curva típica de bateria Li-Ion 1S (mV -> 0..=100%).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn li_ion_percent(mv: u16) -> u8 {
    const MV: [u16; 11] = [3500, 3650, 3700, 3740, 3780, 3820, 3870, 3930, 4000, 4080, 4180];
    const PCT: [u8; 11] = [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
    interpolate_curve(mv, &MV, &PCT)
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn interpolate_curve(mv: u16, xs: &[u16], ys: &[u8]) -> u8 {
    if xs.is_empty() || xs.len() != ys.len() {
        return 0;
    }
    if mv <= xs[0] {
        return ys[0];
    }
    let last = xs.len() - 1;
    if mv >= xs[last] {
        return ys[last];
    }
    for i in 0..last {
        if mv >= xs[i] && mv <= xs[i + 1] {
            let span = (xs[i + 1] - xs[i]).max(1) as u32;
            let pos = (mv - xs[i]) as u32;
            let y0 = ys[i] as u32;
            let y1 = ys[i + 1] as u32;
            return (y0 + (y1 - y0) * pos / span).min(100) as u8;
        }
    }
    ys[last]
}

// ---------------------------------------------------------------------------
// Ícones vetoriais e cores
// ---------------------------------------------------------------------------

/// Cor da porcentagem/bateria conforme carga e estado de carregamento.
pub fn battery_color(percent: u8, charging: bool, normal: Color32) -> Color32 {
    if charging {
        GREEN
    } else if percent <= CRITICAL_THRESHOLD {
        RED
    } else if percent <= LOW_THRESHOLD {
        AMBER
    } else {
        normal
    }
}

/// Desenha uma bateria horizontal pequenininha centrada em `center` (largura ~14 px).
pub fn draw_mini_battery(p: &Painter, center: Pos2, percent: u8, charging: bool, normal: Color32, alpha: f32) {
    let color = battery_color(percent, charging, normal).gamma_multiply(alpha.clamp(0.0, 1.0));
    let outline = normal.gamma_multiply((alpha * 0.65).clamp(0.0, 1.0));
    let body = Rect::from_center_size(pos2(center.x - 1.0, center.y), vec2(12.0, 7.0));
    p.rect_stroke(body, 2.0, Stroke::new(1.1, outline), StrokeKind::Inside);

    // Polo positivo na direita.
    let nub = Rect::from_min_size(pos2(body.right() + 0.6, center.y - 1.6), vec2(1.4, 3.2));
    p.rect_filled(nub, 0.7, outline);

    // Carga interna.
    let inner_max = (body.width() - 3.2).max(1.0);
    let fill_w = (inner_max * (percent.min(100) as f32 / 100.0)).max(if percent > 0 { 1.4 } else { 0.0 });
    if fill_w > 0.0 {
        let fill = Rect::from_min_size(pos2(body.left() + 1.6, body.top() + 1.6), vec2(fill_w, body.height() - 3.2));
        p.rect_filled(fill, 1.0, color);
    }

    // Raio discreto quando está no cabo carregando.
    if charging {
        let c = body.center();
        let bolt = vec![
            pos2(c.x + 0.8, c.y - 3.4),
            pos2(c.x - 1.5, c.y + 0.2),
            pos2(c.x + 0.2, c.y + 0.2),
            pos2(c.x - 0.8, c.y + 3.4),
            pos2(c.x + 1.5, c.y - 0.2),
            pos2(c.x - 0.2, c.y - 0.2),
        ];
        p.add(Shape::convex_polygon(bolt, Color32::WHITE.gamma_multiply(alpha.clamp(0.0, 1.0)), Stroke::NONE));
    }
}

/// Desenha o ícone vetorial do periférico centrado em `c` com escala `r` (~6.5..8.5).
pub fn draw_kind_icon(p: &Painter, c: Pos2, r: f32, kind: Kind, color: Color32) {
    let stroke = Stroke::new(1.35, color);
    match kind {
        Kind::Mouse => {
            let body = Rect::from_center_size(c, vec2(r * 1.25, r * 1.85));
            p.rect_stroke(body, r * 0.62, stroke, StrokeKind::Inside);
            p.line_segment([pos2(c.x, body.top() + r * 0.32), pos2(c.x, c.y - r * 0.05)], stroke);
        }
        Kind::Keyboard => {
            let body = Rect::from_center_size(c, vec2(r * 2.15, r * 1.35));
            p.rect_stroke(body, 2.2, stroke, StrokeKind::Inside);
            for col in [-0.58f32, -0.19, 0.19, 0.58] {
                p.circle_filled(pos2(c.x + col * r, c.y - r * 0.22), (r * 0.11).max(0.75), color);
            }
            p.line_segment([pos2(c.x - r * 0.52, c.y + r * 0.26), pos2(c.x + r * 0.52, c.y + r * 0.26)], stroke);
        }
        Kind::Gamepad => {
            // Corpo central + empunhaduras laterais inclinadas + cruz e botões.
            let top = Rect::from_center_size(pos2(c.x, c.y - r * 0.12), vec2(r * 1.95, r * 1.05));
            p.rect_stroke(top, r * 0.48, stroke, StrokeKind::Inside);
            for side in [-1.0f32, 1.0] {
                p.line_segment(
                    [pos2(c.x + side * r * 0.82, c.y + r * 0.15), pos2(c.x + side * r * 0.96, c.y + r * 0.68)],
                    Stroke::new(1.6, color),
                );
            }
            // Direcional à esquerda.
            let dx = c.x - r * 0.44;
            let dy = c.y - r * 0.12;
            let d = r * 0.22;
            p.line_segment([pos2(dx - d, dy), pos2(dx + d, dy)], Stroke::new(1.2, color));
            p.line_segment([pos2(dx, dy - d), pos2(dx, dy + d)], Stroke::new(1.2, color));
            // Dois botõezinhos à direita.
            p.circle_filled(pos2(c.x + r * 0.48, dy - r * 0.11), (r * 0.12).max(0.8), color);
            p.circle_filled(pos2(c.x + r * 0.30, dy + r * 0.11), (r * 0.12).max(0.8), color);
        }
        Kind::Headset => {
            let arc: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 12.0;
                    pos2(c.x - r * 0.82 * a.cos(), c.y + r * 0.12 - r * 0.88 * a.sin())
                })
                .collect();
            p.add(Shape::line(arc, stroke));
            for side in [-1.0f32, 1.0] {
                let cup = Rect::from_center_size(pos2(c.x + side * r * 0.78, c.y + r * 0.28), vec2(r * 0.42, r * 0.76));
                p.rect_filled(cup, r * 0.2, color);
            }
        }
        Kind::Microphone => {
            let cap = Rect::from_min_max(pos2(c.x - r * 0.36, c.y - r * 0.92), pos2(c.x + r * 0.36, c.y + r * 0.16));
            p.rect_filled(cap, r * 0.36, color);
            let arc: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let t = std::f32::consts::PI * i as f32 / 10.0;
                    pos2(c.x + r * 0.62 * t.cos(), c.y - r * 0.14 + r * 0.56 * t.sin())
                })
                .collect();
            p.add(Shape::line(arc, stroke));
            p.line_segment([pos2(c.x, c.y + r * 0.42), pos2(c.x, c.y + r * 0.82)], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str) -> Device {
        Device { id: name.into(), name: name.into(), default: false }
    }

    fn periph(id: &str, name: &str, kind: Kind, percent: u8, charging: bool) -> Peripheral {
        Peripheral { id: id.into(), name: name.into(), kind, percent, charging }
    }

    #[test]
    fn matches_wireless_headset_to_matching_output_not_realtek() {
        let outputs = vec![
            dev("Fones de ouvido (Realtek(R) Audio)"),
            dev("Fones de ouvido (HyperX Cloud III Wireless)"),
            dev("TV LG (NVIDIA High Definition Audio)"),
        ];
        let inputs = vec![dev("Microfone (HyperX Cloud III Wireless)")];
        let batteries = vec![
            periph("hx", "HyperX Cloud III Wireless", Kind::Headset, 82, false),
            periph("pad", "8BitDo Ultimate 2", Kind::Gamepad, 65, false),
        ];

        let res = match_panel_batteries(&batteries, &outputs, &inputs);
        assert!(res.output_badges[0].is_none());
        assert_eq!(res.output_badges[1].as_ref().map(|p| p.percent), Some(82));
        // Não repete o mesmo headset na linha de microfone quando já apareceu na saída.
        assert!(res.input_badges[0].is_none());
        assert_eq!(res.strip.len(), 1);
        assert_eq!(res.strip[0].kind, Kind::Gamepad);
    }

    #[test]
    fn unmatched_bluetooth_earbuds_fall_back_to_strip() {
        let outputs = vec![dev("Alto-falantes (Realtek(R) Audio)")];
        let batteries = vec![periph("bt:xm5", "WH-1000XM5", Kind::Headset, 55, false)];
        let res = match_panel_batteries(&batteries, &outputs, &[]);
        assert!(res.output_badges[0].is_none());
        assert_eq!(res.strip.len(), 1);
        assert_eq!(res.strip[0].name, "WH-1000XM5");
    }

    #[test]
    fn low_battery_alert_respects_game_mode_and_thresholds() {
        let mut tracker = AlertTracker::default();
        let low = [periph("pad", "8BitDo Ultimate Wireless", Kind::Gamepad, 19, false)];

        // Durante um jogo (`can_notify = false`), não avisa nem gasta o gatilho.
        assert_eq!(tracker.poll(&low, false), None);

        // Saiu do jogo: avisa uma vez para <= 20%.
        let alert = tracker.poll(&low, true).expect("deve avisar em 19%");
        assert_eq!(alert.percent, 19);
        assert_eq!(tracker.poll(&low, true), None);

        // Caiu para <= 10%: avisa mais uma vez.
        let crit = [periph("pad", "8BitDo Ultimate Wireless", Kind::Gamepad, 10, false)];
        let alert2 = tracker.poll(&crit, true).expect("deve avisar em 10%");
        assert_eq!(alert2.percent, 10);
        assert_eq!(tracker.poll(&crit, true), None);

        // Carregou acima de 25%: rearma os avisos futuros.
        let recharged = [periph("pad", "8BitDo Ultimate Wireless", Kind::Gamepad, 40, false)];
        assert_eq!(tracker.poll(&recharged, true), None);
        assert!(tracker.poll(&low, true).is_some());
    }

    #[test]
    fn grace_cache_keeps_sleeping_mouse_until_dongle_unplugged() {
        let mut cache = GraceCache::default();
        let t0 = Instant::now();
        let tag = (0x3554u32 << 16) | 0xF58A;

        let mut first = PollOutput::default();
        first.present_tags.insert(tag);
        first.readings.push(RawReading {
            peripheral: periph("vxe", "VXE Dragonfly R1", Kind::Mouse, 73, false),
            receiver_tag: Some(tag),
            asleep: false,
        });
        assert_eq!(cache.resolve(t0, first).len(), 1);

        // Mouse adormeceu, mas o dongle continua espetado: mantém 73%.
        let mut sleeping = PollOutput::default();
        sleeping.present_tags.insert(tag);
        let kept = cache.resolve(t0 + Duration::from_secs(60), sleeping);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].percent, 73);

        // Dongle desconectado do USB: some na hora.
        let unplugged = PollOutput::default();
        assert!(cache.resolve(t0 + Duration::from_secs(65), unplugged).is_empty());
    }

    #[test]
    fn protocol_helpers_compute_expected_values() {
        let mut req = [0u8; 17];
        req[0] = 0x08;
        req[1] = 0x04;
        req[16] = compx_checksum(&req);
        assert_eq!(req[16], 0x49);
        assert_eq!(li_ion_percent(3500), 0);
        assert_eq!(li_ion_percent(4200), 100);
        assert!((45..=55).contains(&li_ion_percent(3820)));
        assert_eq!(strip_bt_service_suffix("WH-1000XM5 Hands-Free AG Audio"), "WH-1000XM5");
        assert_eq!(guess_kind_from_name("8BitDo Ultimate 2 Wireless"), Some(Kind::Gamepad));
    }
}
