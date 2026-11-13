//! Backend de mentira para desenvolver/testar a interface fora do Windows.
//!
//! Imita o ritmo do backend de verdade: publica o som a cada volta, os
//! números do sistema uma vez por segundo, e acorda a interface só quando
//! algo que ela mostra mudou.

use super::{Command, FpsAccess, Game, Media, Session, Shared, Snapshot, System, Temp, TempSource, Wake};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

pub fn run(state: Shared, rx: Receiver<Command>, wake: Wake) {
    let start = Instant::now();
    let mut snap = Snapshot {
        master: 0.62,
        sessions: ["Spotify", "Chrome", "Discord", "Jogo"]
            .iter()
            .zip([0.8, 0.55, 0.7, 0.4])
            .map(|(name, volume)| Session {
                name: name.to_string(),
                volume,
                ..Default::default()
            })
            .collect(),
        media: Some(Media {
            title: "Faixa de demonstração".into(),
            artist: "Islandbar".into(),
            playing: true,
        }),
        ..Default::default()
    };

    let debug_game = std::env::var_os("ISLANDBAR_DEBUG_GAME").is_some();
    // ISLANDBAR_DEBUG_SILENT=1: nenhum som tocando (para testar a ilha parada).
    let silent = std::env::var_os("ISLANDBAR_DEBUG_SILENT").is_some();
    if std::env::var_os("ISLANDBAR_DEBUG_FPS_DENIED").is_some() {
        snap.fps_access = FpsAccess::Denied;
    }
    if std::env::var_os("ISLANDBAR_DEBUG_NO_MEDIA").is_some() {
        snap.media = None;
    }

    let mut panel = false;
    let mut had_sound = false;
    let mut last_system: Option<Instant> = None;

    loop {
        let started = Instant::now();
        // Algo que a interface mostra mudou nesta volta?
        let mut changed = false;
        while let Ok(cmd) = rx.try_recv() {
            changed = true;
            match cmd {
                Command::SetMaster(v) => snap.master = v,
                Command::ToggleMasterMute => snap.master_muted = !snap.master_muted,
                Command::SetSession(name, v) => {
                    if let Some(s) = snap.sessions.iter_mut().find(|s| s.name == name) {
                        s.volume = v;
                    }
                }
                Command::ToggleSessionMute(name) => {
                    if let Some(s) = snap.sessions.iter_mut().find(|s| s.name == name) {
                        s.muted = !s.muted;
                    }
                }
                Command::PlayPause => {
                    if let Some(m) = &mut snap.media {
                        m.playing = !m.playing;
                    }
                }
                Command::Next | Command::Prev => {}
                Command::EnableFps => snap.fps_access = FpsAccess::Requested,
                Command::Panel(open) => panel = open,
            }
        }

        let t = start.elapsed().as_secs_f32();
        let playing = !silent && snap.media.as_ref().is_some_and(|m| m.playing);
        let mut master_peak = 0.0f32;
        for (i, s) in snap.sessions.iter_mut().enumerate() {
            let k = i as f32;
            let wave = ((t * (5.0 + k * 1.7)).sin() * (t * (1.3 + k)).sin()).abs();
            let active = !silent && if i == 0 { playing } else { i == 1 };
            s.peak = if active && !s.muted { wave * s.volume } else { 0.0 };
            master_peak = master_peak.max(s.peak);
        }
        snap.master_peak = if snap.master_muted { 0.0 } else { master_peak * snap.master };

        // Como no Windows: números do sistema e detecção de jogo uma vez por segundo.
        if last_system.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
            last_system = Some(Instant::now());
            changed = true;
            crate::perf::record_backend_thread();
            snap.system = System {
                cpu: 0.32 + 0.25 * (t * 0.7).sin(),
                ram_used_gb: 9.6 + (t * 0.2).sin(),
                ram_total_gb: 16.0,
                gpu: Some(0.45 + 0.3 * (t * 0.5).sin()),
                temp: Some(Temp { celsius: 58.0 + 9.0 * (t * 0.4).sin(), source: TempSource::System }),
            };
            // ISLANDBAR_DEBUG_GAME=1 simula um jogo em tela cheia depois de 3 segundos.
            if debug_game && t > 3.0 {
                snap.game = Some(Game { name: "Jogo de teste".into(), fps: Some(140.0 + 8.0 * (t * 1.3).sin()) });
            }
        }

        // O som começou: a interface precisa acordar para dançar (fora do modo jogo).
        let sound = snap.master_peak > 0.005;
        if sound && !had_sound && (snap.game.is_none() || panel) {
            changed = true;
        }
        had_sound = sound;

        if let Ok(mut shared) = state.lock() {
            *shared = snap.clone();
        }
        if changed {
            wake();
        }
        crate::perf::add_backend_busy(started);
        std::thread::sleep(Duration::from_millis(33));
    }
}
