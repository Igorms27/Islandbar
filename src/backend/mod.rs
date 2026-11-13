//! Camada de sistema: lê e controla o áudio/mídia do computador.
//!
//! Roda em uma thread separada para a interface nunca travar. A UI só lê um
//! `Snapshot` (uma "foto" do estado atual) e envia `Command`s.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

#[cfg(not(windows))]
mod mock;
#[cfg(windows)]
mod fps;
#[cfg(windows)]
mod game;
#[cfg(windows)]
mod sysmon;
#[cfg(windows)]
mod windows;

/// Um aplicativo que está tocando áudio (uma linha do mixer).
#[derive(Clone, Debug, Default)]
pub struct Session {
    /// Nome amigável do app (ex.: "Spotify"). Também serve de identificador.
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    /// Pico de áudio atual, 0..1 (usado nas animações).
    pub peak: f32,
}

/// A mídia tocando agora (Spotify, navegador, etc.).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    pub title: String,
    pub artist: String,
    pub playing: bool,
}

/// De qual sensor veio a temperatura.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))] // o backend de teste só usa `System`
pub enum TempSource {
    /// Zona térmica informada pela placa-mãe (em geral, perto do processador).
    System,
    /// Placa de vídeo.
    Gpu,
    /// Disco principal (SSD/HD).
    Disk,
}

#[derive(Clone, Copy, Debug)]
pub struct Temp {
    pub celsius: f32,
    pub source: TempSource,
}

/// Uso do computador.
#[derive(Clone, Copy, Debug, Default)]
pub struct System {
    /// Uso da CPU, 0..1.
    pub cpu: f32,
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
    /// Uso da placa de vídeo, 0..1. `None` quando o Windows não informa.
    pub gpu: Option<f32>,
    /// Melhor temperatura disponível. `None` quando o PC não expõe nenhum sensor.
    pub temp: Option<Temp>,
}

impl System {
    pub fn ram(&self) -> f32 {
        if self.ram_total_gb > 0.0 { self.ram_used_gb / self.ram_total_gb } else { 0.0 }
    }
}

/// Jogo em tela cheia que está em primeiro plano.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Game {
    pub name: String,
    /// Quadros por segundo. `None` quando não dá para medir.
    pub fps: Option<f32>,
}

/// O Windows só deixa medir o FPS de outros programas com uma permissão
/// (a mesma que a Xbox Game Bar pede).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum FpsAccess {
    #[default]
    Granted,
    /// Falta a permissão.
    Denied,
    /// A permissão foi pedida; só vale depois de sair e entrar no Windows.
    Requested,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub system: System,
    pub game: Option<Game>,
    pub fps_access: FpsAccess,
    pub master: f32,
    pub master_muted: bool,
    pub master_peak: f32,
    pub sessions: Vec<Session>,
    pub media: Option<Media>,
}

#[derive(Clone, Debug)]
pub enum Command {
    SetMaster(f32),
    ToggleMasterMute,
    SetSession(String, f32),
    ToggleSessionMute(String),
    PlayPause,
    Next,
    Prev,
    /// Pede ao Windows a permissão para medir FPS.
    EnableFps,
}

pub type Shared = Arc<Mutex<Snapshot>>;

pub struct Backend {
    state: Shared,
    tx: Sender<Command>,
}

impl Backend {
    pub fn spawn() -> Self {
        let state: Shared = Arc::default();
        let (tx, rx) = channel();
        let thread_state = state.clone();
        std::thread::Builder::new()
            .name("islandbar-backend".into())
            .spawn(move || run(thread_state, rx))
            .expect("não foi possível iniciar a thread de áudio");
        Self { state, tx }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

/// Desfaz o que não some sozinho quando o programa fecha.
pub fn shutdown() {
    // A sessão de medição de FPS continuaria aberta no Windows.
    #[cfg(windows)]
    fps::stop_session();
}

fn run(state: Shared, rx: Receiver<Command>) {
    #[cfg(windows)]
    windows::run(state, rx);
    #[cfg(not(windows))]
    mock::run(state, rx);
}
