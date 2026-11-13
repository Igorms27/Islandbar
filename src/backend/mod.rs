//! Camada de sistema: lê e controla o áudio/mídia do computador.
//!
//! Roda em uma thread separada para a interface nunca travar. A UI só lê um
//! `Snapshot` (uma "foto" do estado atual) e envia `Command`s.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

#[cfg(not(windows))]
mod mock;
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

/// Uso do computador.
#[derive(Clone, Copy, Debug, Default)]
pub struct System {
    /// Uso da CPU, 0..1.
    pub cpu: f32,
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
    /// Temperatura em °C. `None` quando o PC não informa ao Windows.
    pub temp_c: Option<f32>,
}

impl System {
    pub fn ram(&self) -> f32 {
        if self.ram_total_gb > 0.0 { self.ram_used_gb / self.ram_total_gb } else { 0.0 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub system: System,
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

fn run(state: Shared, rx: Receiver<Command>) {
    #[cfg(windows)]
    windows::run(state, rx);
    #[cfg(not(windows))]
    mock::run(state, rx);
}
