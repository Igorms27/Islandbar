//! Backend real do Windows.
//!
//! - Volume geral e por aplicativo: WASAPI (as mesmas APIs do Mixer de Volume).
//! - "Tocando agora" e botões de mídia: Global System Media Transport Controls
//!   (o mesmo sistema que o Windows usa no flyout de mídia).

use super::sysmon::SystemMonitor;
use super::{Command, Media, Session, Shared, Snapshot, System};
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use windows::core::{Interface, Result, PWSTR};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as MediaSession,
    GlobalSystemMediaTransportControlsSessionManager as MediaManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

const TICK: Duration = Duration::from_millis(33);
const REFRESH_EVERY: Duration = Duration::from_millis(1000);
const MEDIA_EVERY: Duration = Duration::from_millis(500);

/// Um fluxo de áudio de um processo. Um app pode ter vários (ex.: Chrome).
struct Stream {
    name: String,
    volume: ISimpleAudioVolume,
    meter: IAudioMeterInformation,
}

/// Tudo que pegamos do dispositivo de saída padrão.
struct Audio {
    endpoint: IAudioEndpointVolume,
    meter: IAudioMeterInformation,
    streams: Vec<Stream>,
}

pub fn run(state: Shared, rx: Receiver<Command>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let mut names: HashMap<u32, String> = HashMap::new();
    let mut audio: Option<Audio> = None;
    let mut media_mgr: Option<MediaManager> = None;
    let mut media: Option<Media> = None;
    let mut last_refresh: Option<Instant> = None;
    let mut last_media: Option<Instant> = None;
    let mut monitor = SystemMonitor::new();
    let mut system = System::default();

    loop {
        let now = Instant::now();

        // Reabre o dispositivo e relista os apps de tempos em tempos. Assim a
        // gente acompanha apps abrindo/fechando e troca de fone/caixa de som.
        if last_refresh.is_none_or(|t| now - t >= REFRESH_EVERY) {
            last_refresh = Some(now);
            audio = unsafe { open_audio(&mut names) }.ok();
            system = monitor.read();
        }

        if last_media.is_none_or(|t| now - t >= MEDIA_EVERY) {
            last_media = Some(now);
            if media_mgr.is_none() {
                media_mgr = MediaManager::RequestAsync().and_then(|op| op.join()).ok();
            }
            media = media_mgr.as_ref().and_then(|m| read_media(m).ok());
        }

        while let Ok(cmd) = rx.try_recv() {
            let _ = unsafe { apply(cmd, audio.as_ref(), media_mgr.as_ref()) };
            // Faz a mídia atualizar logo depois de um play/pause.
            last_media = None;
        }

        let mut snap = audio
            .as_ref()
            .map(|a| unsafe { read_audio(a) })
            .unwrap_or_default();
        snap.media = media.clone();
        snap.system = system;
        if let Ok(mut shared) = state.lock() {
            *shared = snap;
        }

        std::thread::sleep(TICK);
    }
}

unsafe fn open_audio(names: &mut HashMap<u32, String>) -> Result<Audio> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
    let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
    let meter: IAudioMeterInformation = device.Activate(CLSCTX_ALL, None)?;
    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;

    let list = manager.GetSessionEnumerator()?;
    let mut streams = Vec::new();
    for i in 0..list.GetCount()? {
        let Ok(control) = list.GetSession(i) else { continue };
        if control.GetState() == Ok(AudioSessionStateExpired) {
            continue;
        }
        let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
        let pid = control2.GetProcessId().unwrap_or(0);
        let name = if pid == 0 {
            "Sons do sistema".to_string()
        } else {
            names.entry(pid).or_insert_with(|| process_name(pid)).clone()
        };
        let (Ok(volume), Ok(meter)) = (
            control.cast::<ISimpleAudioVolume>(),
            control.cast::<IAudioMeterInformation>(),
        ) else {
            continue;
        };
        streams.push(Stream { name, volume, meter });
    }

    Ok(Audio { endpoint, meter, streams })
}

unsafe fn read_audio(audio: &Audio) -> Snapshot {
    let mut sessions: Vec<Session> = Vec::new();
    for stream in &audio.streams {
        let volume = stream.volume.GetMasterVolume().unwrap_or(0.0);
        let muted = stream.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        let peak = stream.meter.GetPeakValue().unwrap_or(0.0);
        // Junta os vários fluxos do mesmo app em uma linha só.
        match sessions.iter_mut().find(|s| s.name == stream.name) {
            Some(s) => {
                s.volume = s.volume.max(volume);
                s.muted &= muted;
                s.peak = s.peak.max(peak);
            }
            None => sessions.push(Session { name: stream.name.clone(), volume, muted, peak }),
        }
    }
    sessions.sort_by_key(|s| s.name.to_lowercase());

    Snapshot {
        master: audio.endpoint.GetMasterVolumeLevelScalar().unwrap_or(0.0),
        master_muted: audio.endpoint.GetMute().map(|b| b.as_bool()).unwrap_or(false),
        master_peak: audio.meter.GetPeakValue().unwrap_or(0.0),
        sessions,
        ..Default::default()
    }
}

unsafe fn apply(cmd: Command, audio: Option<&Audio>, media: Option<&MediaManager>) -> Result<()> {
    let no_ctx = std::ptr::null();
    match cmd {
        Command::SetMaster(v) => {
            if let Some(a) = audio {
                a.endpoint.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), no_ctx)?;
            }
        }
        Command::ToggleMasterMute => {
            if let Some(a) = audio {
                let muted = a.endpoint.GetMute()?.as_bool();
                a.endpoint.SetMute(!muted, no_ctx)?;
            }
        }
        Command::SetSession(name, v) => {
            for s in audio.iter().flat_map(|a| &a.streams).filter(|s| s.name == name) {
                let _ = s.volume.SetMasterVolume(v.clamp(0.0, 1.0), no_ctx);
            }
        }
        Command::ToggleSessionMute(name) => {
            let streams: Vec<&Stream> =
                audio.iter().flat_map(|a| &a.streams).filter(|s| s.name == name).collect();
            let all_muted = streams
                .iter()
                .all(|s| s.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false));
            for s in streams {
                let _ = s.volume.SetMute(!all_muted, no_ctx);
            }
        }
        Command::PlayPause => {
            current_session(media)?.TryTogglePlayPauseAsync()?;
        }
        Command::Next => {
            current_session(media)?.TrySkipNextAsync()?;
        }
        Command::Prev => {
            current_session(media)?.TrySkipPreviousAsync()?;
        }
    }
    Ok(())
}

fn current_session(manager: Option<&MediaManager>) -> Result<MediaSession> {
    manager
        .ok_or_else(windows::core::Error::empty)?
        .GetCurrentSession()
}

fn read_media(manager: &MediaManager) -> Result<Media> {
    let session = manager.GetCurrentSession()?;
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == PlaybackStatus::Playing;
    Ok(Media {
        title: props.Title()?.to_string(),
        artist: props.Artist()?.to_string(),
        playing,
    })
}

/// "C:\...\chrome.exe" -> "Chrome"
fn process_name(pid: u32) -> String {
    let path = unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return format!("PID {pid}");
        };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        if ok.is_err() {
            return format!("PID {pid}");
        }
        String::from_utf16_lossy(&buf[..len as usize])
    };
    let stem = std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or(path);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => format!("PID {pid}"),
    }
}
