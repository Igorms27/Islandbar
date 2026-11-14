//! Modo jogo: descobre se há um jogo em tela cheia em primeiro plano.
//!
//! Não existe uma API "isto é um jogo", então juntamos sinais:
//!
//! 1. A janela em primeiro plano cobre exatamente um monitor inteiro
//!    (janelas maximizadas passam alguns pixels da borda e ficam de fora).
//! 2. E o programa dono dela está desenhando de verdade: tela cheia
//!    exclusiva, uso relevante da GPU ou quadros sendo entregues.
//!
//! Navegadores e players de vídeo ficam de fora.

use super::fps::{self, FpsTracer, StartError};
use super::{FpsAccess, Game};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
};

/// Uso de GPU a partir do qual um programa em tela cheia conta como jogo.
const GAME_GPU: f32 = 0.10;
/// Quadros por segundo a partir dos quais conta como jogo.
const GAME_FPS: f32 = 20.0;
/// Leituras seguidas necessárias para entrar no (ou sair do) modo jogo.
const CONFIRMATIONS: u32 = 2;
/// Sem tela cheia por este tempo, a medição de FPS é desligada.
const TRACER_IDLE: Duration = Duration::from_secs(10);

/// Janelas do próprio Windows que ocupam a tela toda.
const SHELL_CLASSES: [&str; 4] = ["Progman", "WorkerW", "Shell_TrayWnd", "Windows.UI.Core.CoreWindow"];
/// Programas que ficam em tela cheia desenhando, mas não são jogos.
const NOT_GAMES: [&str; 22] = [
    "chrome", "msedge", "firefox", "opera", "opera_gx", "brave", "vivaldi", "vlc", "mpv", "mpc-hc",
    "mpc-hc64", "mpc-be", "mpc-be64", "potplayermini64", "wmplayer", "video.ui", "microsoft.media.player",
    "kodi", "stremio", "obs64", "explorer", "powerpnt",
];

struct Fullscreen {
    window: isize,
    pid: u32,
    /// Tela cheia exclusiva do DirectX (só jogos usam).
    exclusive: bool,
}

pub struct GameWatcher {
    access: FpsAccess,
    tracer: Option<FpsTracer>,
    tracer_failed: bool,
    last_fullscreen: Option<Instant>,
    last_update: Instant,
    /// Janela candidata e quantas leituras seguidas ela pareceu um jogo.
    candidate: Option<(isize, u32)>,
    /// Janela confirmada como jogo e o nome a mostrar.
    active: Option<(isize, String)>,
    misses: u32,
    /// Tela cheia vista por `look`, esperando o `update`.
    seen: Option<Fullscreen>,
    debug: bool,
}

impl GameWatcher {
    pub fn new() -> Self {
        fps::stop_session();
        let denied = std::env::var_os("ISLANDBAR_DEBUG_FPS_DENIED").is_some() || !fps::has_access();
        Self {
            access: if denied { FpsAccess::Denied } else { FpsAccess::Granted },
            tracer: None,
            tracer_failed: false,
            last_fullscreen: None,
            last_update: Instant::now(),
            candidate: None,
            active: None,
            misses: 0,
            seen: None,
            debug: std::env::var_os("ISLANDBAR_DEBUG_GAME").is_some(),
        }
    }

    pub fn access(&self) -> FpsAccess {
        self.access
    }

    pub fn request_access(&mut self) {
        if self.access == FpsAccess::Denied {
            fps::request_access();
            self.access = FpsAccess::Requested;
        }
    }

    /// Primeiro passo de cada leitura: olha a janela em primeiro plano.
    /// Devolve `true` se há uma tela cheia que pode ser um jogo; só nesse
    /// caso vale a pena medir o uso de GPU de cada programa.
    pub fn look(&mut self) -> bool {
        self.seen = unsafe { fullscreen_window() };
        self.seen.is_some()
    }

    /// Chamar cerca de uma vez por segundo, logo depois de `look`.
    /// `gpu_by_pid` é o uso de GPU de cada programa.
    pub fn update(&mut self, gpu_by_pid: &HashMap<u32, f32>) -> Option<Game> {
        let now = Instant::now();
        let elapsed = (now - self.last_update).as_secs_f32().max(0.001);
        self.last_update = now;

        let fullscreen = self.seen.take();
        self.manage_tracer(fullscreen.is_some(), now);
        let presents = self.tracer.as_ref().map(FpsTracer::take);

        let Some(fullscreen) = fullscreen else {
            return self.miss();
        };
        let fps = presents
            .as_ref()
            .and_then(|p| p.get(&fullscreen.pid))
            .map(|p| p.frames() as f32 / elapsed)
            .filter(|fps| *fps >= 1.0);

        // Já confirmado: continua enquanto for a mesma janela.
        if let Some((window, name)) = &self.active {
            if *window == fullscreen.window {
                self.misses = 0;
                return Some(Game { name: name.clone(), fps });
            }
            return self.miss();
        }

        let gpu = gpu_by_pid.get(&fullscreen.pid).copied().unwrap_or(0.0);
        let looks_like_game = fullscreen.exclusive || gpu >= GAME_GPU || fps.is_some_and(|f| f >= GAME_FPS);
        if self.debug {
            let seen = fps::EVENTS_SEEN.swap(0, std::sync::atomic::Ordering::Relaxed);
            let mut busiest: Vec<_> = presents.iter().flatten().map(|(pid, p)| (p.frames(), exe_stem(*pid))).collect();
            busiest.sort_unstable_by(|a, b| b.cmp(a));
            busiest.truncate(4);
            crate::perf::line(format!(
                "game: fullscreen pid={} ({}) exclusive={} gpu={gpu:.2} fps={fps:?} -> {looks_like_game} | tracer={} events={seen} frames={busiest:?}",
                fullscreen.pid,
                exe_stem(fullscreen.pid),
                fullscreen.exclusive,
                self.tracer.is_some()
            ));
        }
        if !looks_like_game {
            self.candidate = None;
            return None;
        }
        let hits = match self.candidate {
            Some((window, hits)) if window == fullscreen.window => hits + 1,
            _ => 1,
        };
        self.candidate = Some((fullscreen.window, hits));
        if hits < CONFIRMATIONS {
            return None;
        }

        let name = unsafe { window_title(fullscreen.window) }.unwrap_or_else(|| process_name(fullscreen.pid));
        self.active = Some((fullscreen.window, name.clone()));
        self.misses = 0;
        Some(Game { name, fps })
    }

    /// A janela do jogo não está mais em primeiro plano em tela cheia.
    fn miss(&mut self) -> Option<Game> {
        self.candidate = None;
        self.misses += 1;
        if self.misses >= CONFIRMATIONS {
            self.active = None;
        }
        // Tolera uma leitura ruim (ex.: troca rápida de janela) sem piscar a pílula.
        self.active.as_ref().map(|(_, name)| Game { name: name.clone(), fps: None })
    }

    /// Só mede FPS enquanto há algo em tela cheia; fora disso a sessão fica desligada.
    fn manage_tracer(&mut self, fullscreen: bool, now: Instant) {
        if fullscreen {
            self.last_fullscreen = Some(now);
            if self.tracer.is_none() && self.access == FpsAccess::Granted && !self.tracer_failed {
                match FpsTracer::start() {
                    Ok(tracer) => self.tracer = Some(tracer),
                    Err(StartError::AccessDenied) => self.access = FpsAccess::Denied,
                    Err(StartError::Other(code)) => {
                        self.tracer_failed = true;
                        if self.debug {
                            eprintln!("game: fps tracer failed with error {code}");
                        }
                    }
                }
            }
        } else if self.last_fullscreen.is_none_or(|at| now - at >= TRACER_IDLE) {
            self.tracer = None;
        }
    }
}

/// A janela em primeiro plano, se ela cobre exatamente um monitor inteiro.
unsafe fn fullscreen_window() -> Option<Fullscreen> {
    let window = GetForegroundWindow();
    if window.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(window, Some(&mut pid));
    if pid == 0 || pid == GetCurrentProcessId() {
        return None;
    }

    let mut rect = RECT::default();
    GetWindowRect(window, &mut rect).ok()?;
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !GetMonitorInfoW(MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
        return None;
    }
    if rect != info.rcMonitor {
        return None;
    }

    let mut class = [0u16; 128];
    let len = GetClassNameW(window, &mut class) as usize;
    let class = String::from_utf16_lossy(&class[..len]);
    if SHELL_CLASSES.contains(&class.as_str()) {
        return None;
    }
    if NOT_GAMES.contains(&exe_stem(pid).to_lowercase().as_str()) {
        return None;
    }

    let exclusive = SHQueryUserNotificationState() == Ok(QUNS_RUNNING_D3D_FULL_SCREEN);
    Some(Fullscreen { window: window.0 as isize, pid, exclusive })
}

unsafe fn window_title(window: isize) -> Option<String> {
    let mut title = [0u16; 256];
    let len = GetWindowTextW(HWND(window as *mut std::ffi::c_void), &mut title) as usize;
    let title = String::from_utf16_lossy(&title[..len]).trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// "C:\...\Jogo.exe" -> "Jogo". Vazio se o Windows não deixar consultar.
fn exe_stem(pid: u32) -> String {
    let path = unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buffer = [0u16; 512];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        if result.is_err() {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..len as usize])
    };
    std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn process_name(pid: u32) -> String {
    let stem = exe_stem(pid);
    if stem.is_empty() { "Jogo".to_string() } else { stem }
}
