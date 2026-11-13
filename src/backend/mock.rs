//! Backend de mentira para desenvolver/testar a interface fora do Windows.

use super::{Command, Media, Session, Shared, Snapshot};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

pub fn run(state: Shared, rx: Receiver<Command>) {
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

    loop {
        while let Ok(cmd) = rx.try_recv() {
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
            }
        }

        let t = start.elapsed().as_secs_f32();
        let playing = snap.media.as_ref().is_some_and(|m| m.playing);
        let mut master_peak = 0.0f32;
        for (i, s) in snap.sessions.iter_mut().enumerate() {
            let k = i as f32;
            let wave = ((t * (5.0 + k * 1.7)).sin() * (t * (1.3 + k)).sin()).abs();
            let active = if i == 0 { playing } else { i == 1 };
            s.peak = if active && !s.muted { wave * s.volume } else { 0.0 };
            master_peak = master_peak.max(s.peak);
        }
        snap.master_peak = if snap.master_muted { 0.0 } else { master_peak * snap.master };

        if let Ok(mut shared) = state.lock() {
            *shared = snap.clone();
        }
        std::thread::sleep(Duration::from_millis(33));
    }
}
