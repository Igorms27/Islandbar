//! Backend real do Windows.
//!
//! - Volume geral e por aplicativo: WASAPI (as mesmas APIs do Mixer de Volume).
//! - "Tocando agora" e botões de mídia: Global System Media Transport Controls
//!   (o mesmo sistema que o Windows usa no flyout de mídia).
//! - Troca de saída de som e de microfone: a interface `IPolicyConfig`, a mesma
//!   que as configurações de som do Windows usam.

use super::game::GameWatcher;
use super::sysmon::{SystemMonitor, Wanted};
use super::{Command, Device, Game, Media, Session, Shared, System, Wake};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use windows::core::{Interface, Result, GUID, PCWSTR, PWSTR};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as MediaSession,
    GlobalSystemMediaTransportControlsSessionManager as MediaManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, AudioSessionStateExpired, EDataFlow,
    IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

// O backend acorda em intervalos ("voltas"). Quanto menos voltas e quanto
// menos coisa em cada uma, menos CPU a ilha gasta.

/// Com som tocando ou o painel aberto: os medidores precisam de leituras frequentes.
const FAST: Duration = Duration::from_millis(33);
/// Em repouso: só acompanha o volume (para o aviso das teclas de volume) e
/// percebe quando um som começa.
const SLOW: Duration = Duration::from_millis(100);
/// CPU, RAM, GPU e detecção de jogo.
const SYSTEM_EVERY: Duration = Duration::from_millis(1000);
/// Lista de apps com som. Só é lida com o painel aberto.
const SESSIONS_EVERY: Duration = Duration::from_millis(1000);
/// Com o painel fechado basta notar a troca de fone/caixa de som.
const DEVICE_EVERY: Duration = Duration::from_millis(2000);
/// Depois de perder o dispositivo, espera um pouco antes de procurar outro.
const DEVICE_RETRY: Duration = Duration::from_millis(250);
const MEDIA_EVERY: Duration = Duration::from_millis(1000);
const MEDIA_EVERY_OPEN: Duration = Duration::from_millis(500);
/// Depois de um play/pause, o Windows leva um instante para refletir a mudança.
const MEDIA_AFTER_COMMAND: Duration = Duration::from_millis(150);
/// Lista de saídas e microfones. Só é lida com o painel aberto; assim percebe
/// um fone conectado ou desconectado sem gastar nada com o painel fechado.
const DEVICES_EVERY: Duration = Duration::from_millis(2000);
/// Pico de áudio a partir do qual consideramos que há som tocando.
const SOUND: f32 = 0.005;
/// Sem som por este tempo, o backend volta ao ritmo de repouso.
const SOUND_LINGER: Duration = Duration::from_millis(1500);
/// Diferença de volume que conta como mudança (a mesma que a interface usa).
const VOLUME_STEP: f32 = 0.004;

/// Um fluxo de áudio de um processo. Um app pode ter vários (ex.: Chrome).
struct Stream {
    name: String,
    volume: ISimpleAudioVolume,
    meter: IAudioMeterInformation,
}

/// O dispositivo de saída padrão.
struct Audio {
    /// Identificador do dispositivo, para notar quando o padrão muda.
    id: Vec<u16>,
    device: IMMDevice,
    endpoint: IAudioEndpointVolume,
    meter: IAudioMeterInformation,
    /// Vazio enquanto o painel está fechado.
    streams: Vec<Stream>,
}

// Fica num módulo à parte só para os nomes das funções seguirem os do Windows.
mod policy {
    #![allow(non_snake_case)]
    use windows::core::{HRESULT, PCWSTR};
    use windows::Win32::Media::Audio::ERole;
    use windows_core::{IUnknown, IUnknown_Vtbl};

    /// Interface do Windows que escolhe o dispositivo padrão. Não é documentada,
    /// mas é a que as próprias configurações de som usam (e programas como
    /// EarTrumpet e SoundSwitch), estável desde o Windows 7.
    ///
    /// Só `SetDefaultEndpoint` é usada; as outras estão aqui para que ela fique na
    /// posição certa da tabela de funções.
    #[windows_core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    pub unsafe trait IPolicyConfig: IUnknown {
        fn GetMixFormat(&self, id: PCWSTR, format: *mut *mut std::ffi::c_void) -> HRESULT;
        fn GetDeviceFormat(&self, id: PCWSTR, default: i32, format: *mut *mut std::ffi::c_void) -> HRESULT;
        fn ResetDeviceFormat(&self, id: PCWSTR) -> HRESULT;
        fn SetDeviceFormat(&self, id: PCWSTR, endpoint: *mut std::ffi::c_void, mix: *mut std::ffi::c_void) -> HRESULT;
        fn GetProcessingPeriod(&self, id: PCWSTR, default: i32, period: *mut i64, minimum: *mut i64) -> HRESULT;
        fn SetProcessingPeriod(&self, id: PCWSTR, period: *mut i64) -> HRESULT;
        fn GetShareMode(&self, id: PCWSTR, mode: *mut std::ffi::c_void) -> HRESULT;
        fn SetShareMode(&self, id: PCWSTR, mode: *mut std::ffi::c_void) -> HRESULT;
        fn GetPropertyValue(&self, id: PCWSTR, key: *const std::ffi::c_void, value: *mut std::ffi::c_void) -> HRESULT;
        fn SetPropertyValue(&self, id: PCWSTR, key: *const std::ffi::c_void, value: *mut std::ffi::c_void) -> HRESULT;
        fn SetDefaultEndpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
        fn SetEndpointVisibility(&self, id: PCWSTR, visible: i32) -> HRESULT;
    }

    /// (As funções geradas para a interface só são visíveis aqui dentro.)
    pub unsafe fn set_default_endpoint(policy: &IPolicyConfig, id: PCWSTR, role: ERole) -> HRESULT {
        policy.SetDefaultEndpoint(id, role)
    }
}
use policy::IPolicyConfig;

const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

pub fn run(state: Shared, rx: Receiver<Command>, wake: Wake) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let mut names: HashMap<u32, String> = HashMap::new();
    let mut enumerator: Option<IMMDeviceEnumerator> = None;
    let mut audio: Option<Audio> = None;
    let mut media_mgr: Option<MediaManager> = None;
    let mut media: Option<Media> = None;
    let mut monitor = SystemMonitor::new();
    let mut system = System::default();
    let mut games = GameWatcher::new();
    let mut game: Option<Game> = None;
    // Para diagnóstico: escreve as leituras no stderr.
    let debug_system = std::env::var_os("ISLANDBAR_DEBUG_SYSMON").is_some();
    // Para o teste visual (a máquina do GitHub não tem som): mostra
    // dispositivos de mentira quando não há nenhum de verdade.
    let fake_devices = std::env::var_os("ISLANDBAR_DEBUG_DEVICES").is_some();
    let mut outputs: Vec<Device> = Vec::new();
    let mut inputs: Vec<Device> = Vec::new();

    // O painel está aberto? (A interface avisa.)
    let mut panel = false;
    // Quando cada leitura periódica deve acontecer de novo.
    let start = Instant::now();
    let (mut audio_due, mut system_due, mut media_due, mut next_tick) = (start, start, start, start);
    let mut devices_due = start;
    let mut last_sound: Option<Instant> = None;
    let mut had_sound = false;
    // Volume que a interface já viu (para acordá-la quando ele mudar).
    let mut seen_volume: Option<(f32, bool)> = None;

    loop {
        // Dorme até a próxima volta. Um comando da interface acorda na hora.
        loop {
            let now = Instant::now();
            if now >= next_tick {
                break;
            }
            match rx.recv_timeout(next_tick - now) {
                Ok(Command::Panel(open)) => {
                    panel = open;
                    if open {
                        // Busca agora mesmo o que só o painel mostra.
                        (audio_due, system_due, media_due, devices_due, next_tick) = (now, now, now, now, now);
                    }
                }
                Ok(Command::EnableFps) => games.request_access(),
                Ok(Command::SetDevice(id)) => {
                    if let Err(err) = unsafe { set_default_device(&id) } {
                        crate::perf::line(format!("islandbar: não deu para trocar o dispositivo: {err}"));
                    }
                    // Abre o dispositivo novo e relê a lista já na próxima volta.
                    (audio_due, devices_due, next_tick) = (now, now, now);
                }
                Ok(cmd) => {
                    if matches!(cmd, Command::PlayPause | Command::Next | Command::Prev) {
                        media_due = now + MEDIA_AFTER_COMMAND;
                    }
                    let _ = unsafe { apply(cmd, audio.as_ref(), media_mgr.as_ref()) };
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let now = Instant::now();

        // Dispositivo de saída e, com o painel aberto, a lista de apps. Assim a
        // gente acompanha apps abrindo/fechando e troca de fone/caixa de som.
        if now >= audio_due {
            audio_due = now + if panel { SESSIONS_EVERY } else { DEVICE_EVERY };
            if unsafe { refresh_audio(&mut enumerator, &mut audio, &mut names, panel) }.is_err() {
                audio = None;
            }
            crate::perf::add_section("audio", now);
        }

        // Algo que a interface mostra mudou nesta volta?
        let mut changed = false;

        // Saídas e microfones (só aparecem no painel).
        if panel && now >= devices_due {
            devices_due = now + DEVICES_EVERY;
            let started = Instant::now();
            let read = |flow| match enumerator.as_ref() {
                Some(enumerator) => unsafe { list_devices(enumerator, flow) }.unwrap_or_default(),
                None => Vec::new(),
            };
            let (mut now_outputs, mut now_inputs) = (read(eRender), read(eCapture));
            if fake_devices && now_outputs.is_empty() && now_inputs.is_empty() {
                now_outputs = fake_list(&["Fones de ouvido (Teste)", "TV (HDMI)", "Alto-falantes (Teste)"]);
                now_inputs = fake_list(&["Microfone (Teste)", "Webcam (Teste)"]);
            }
            if now_outputs != outputs || now_inputs != inputs {
                (outputs, inputs) = (now_outputs, now_inputs);
                changed = true;
            }
            crate::perf::add_section("audio", started);
        }

        if now >= system_due {
            system_due = now + SYSTEM_EVERY;
            let started = Instant::now();
            let fullscreen = games.look();
            crate::perf::add_section("game", started);
            // A GPU aparece na pílula (sem música), no painel e no modo jogo;
            // a temperatura, só no painel.
            let wanted = Wanted { gpu: panel || media.is_none() || fullscreen, temp: panel };
            system = monitor.read(wanted);
            let started = Instant::now();
            game = games.update(&monitor.gpu_by_pid);
            crate::perf::add_section("game", started);
            changed = true;
            crate::perf::record_backend_thread();
            if debug_system {
                crate::perf::line(format!(
                    "{system:?} game={game:?} fps_access={:?} audio={} {wanted:?}",
                    games.access(),
                    audio.is_some()
                ));
            }
        }

        if now >= media_due {
            media_due = now + if panel { MEDIA_EVERY_OPEN } else { MEDIA_EVERY };
            let started = Instant::now();
            if media_mgr.is_none() {
                media_mgr = MediaManager::RequestAsync().and_then(|op| op.join()).ok();
            }
            let current = media_mgr.as_ref().and_then(|m| read_media(m).ok());
            if current != media {
                media = current;
                changed = true;
            }
            crate::perf::add_section("media", started);
        }

        // Volume e pico do som: três consultas rápidas por volta.
        let started = Instant::now();
        let master = match audio.as_ref().map(|a| unsafe { read_master(a) }) {
            Some(Ok(master)) => Some(master),
            Some(Err(_)) => {
                // O dispositivo sumiu (fone desconectado...): procura outro logo.
                audio = None;
                audio_due = now + DEVICE_RETRY;
                None
            }
            None => None,
        };
        // O volume e o pico de cada app só interessam com o painel aberto.
        let sessions = if panel { audio.as_ref().map(|a| unsafe { read_sessions(a) }) } else { None };
        crate::perf::add_section("volume", started);

        let peak = master.map_or(0.0, |(_, _, peak)| peak);
        let sound = peak > SOUND;
        if sound {
            last_sound = Some(now);
        }
        // No modo jogo a ilha fica quieta: não dança com o som.
        let quiet = game.is_some() && !panel;
        if sound && !had_sound && !quiet {
            changed = true;
        }
        had_sound = sound;

        if let Some((volume, muted, _)) = master {
            let moved = seen_volume.is_none_or(|(v, m)| (v - volume).abs() > VOLUME_STEP || m != muted);
            if moved {
                seen_volume = Some((volume, muted));
                changed = true;
            }
        }

        let started = Instant::now();
        if let Ok(mut shared) = state.lock() {
            if let Some((volume, muted, peak)) = master {
                shared.master = volume;
                shared.master_muted = muted;
                shared.master_peak = peak;
            } else {
                shared.master_peak = 0.0;
            }
            if let Some(sessions) = sessions {
                shared.sessions = sessions;
            }
            if changed {
                shared.outputs.clone_from(&outputs);
                shared.inputs.clone_from(&inputs);
                shared.system = system;
                shared.game.clone_from(&game);
                shared.media.clone_from(&media);
            }
            shared.fps_access = games.access();
        }

        if changed {
            wake();
        }
        crate::perf::add_section("other", started);

        crate::perf::add_backend_busy(now);
        let lively = panel || (!quiet && last_sound.is_some_and(|at| now - at < SOUND_LINGER));
        next_tick = now + if lively { FAST } else { SLOW };
    }
}

/// Abre o dispositivo de saída padrão (só se ele mudou) e, se pedido, lista
/// os apps que estão tocando som nele.
unsafe fn refresh_audio(
    enumerator: &mut Option<IMMDeviceEnumerator>,
    audio: &mut Option<Audio>,
    names: &mut HashMap<u32, String>,
    with_sessions: bool,
) -> Result<()> {
    let enumerator = match enumerator {
        Some(enumerator) => enumerator,
        None => enumerator.insert(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?),
    };
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
    let id = device.GetId()?;
    let id_text = id.as_wide().to_vec();
    CoTaskMemFree(Some(id.0 as *const std::ffi::c_void));

    if audio.as_ref().is_none_or(|a| a.id != id_text) {
        *audio = Some(Audio {
            id: id_text,
            endpoint: device.Activate(CLSCTX_ALL, None)?,
            meter: device.Activate(CLSCTX_ALL, None)?,
            device,
            streams: Vec::new(),
        });
    }
    let Some(audio) = audio else { return Ok(()) };
    audio.streams = if with_sessions { list_streams(&audio.device, names)? } else { Vec::new() };
    Ok(())
}

unsafe fn list_streams(device: &IMMDevice, names: &mut HashMap<u32, String>) -> Result<Vec<Stream>> {
    // O gerenciador é aberto de novo a cada vez: um já aberto não enxerga os
    // apps que começaram a tocar depois.
    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
    let list = manager.GetSessionEnumerator()?;
    let mut streams = Vec::new();
    let mut pids = Vec::new();
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
            pids.push(pid);
            names.entry(pid).or_insert_with(|| process_name(pid)).clone()
        };
        let (Ok(volume), Ok(meter)) = (control.cast::<ISimpleAudioVolume>(), control.cast::<IAudioMeterInformation>())
        else {
            continue;
        };
        streams.push(Stream { name, volume, meter });
    }
    // Esquece os programas que fecharam: o Windows reaproveita os números
    // (PIDs), e um nome antigo acabaria aparecendo em outro programa.
    names.retain(|pid, _| pids.contains(pid));
    Ok(streams)
}

/// (volume, mudo, pico) do dispositivo de saída.
unsafe fn read_master(audio: &Audio) -> Result<(f32, bool, f32)> {
    let volume = audio.endpoint.GetMasterVolumeLevelScalar()?;
    let muted = audio.endpoint.GetMute()?.as_bool();
    let peak = audio.meter.GetPeakValue().unwrap_or(0.0);
    Ok((volume, muted, peak))
}

unsafe fn read_sessions(audio: &Audio) -> Vec<Session> {
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
    sessions.sort_by_cached_key(|s| s.name.to_lowercase());
    sessions
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
            let streams: Vec<&Stream> = audio.iter().flat_map(|a| &a.streams).filter(|s| s.name == name).collect();
            let all_muted = streams.iter().all(|s| s.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false));
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
        // Tratados no laço principal.
        Command::EnableFps | Command::Panel(_) | Command::SetDevice(_) => {}
    }
    Ok(())
}

/// Saídas (`eRender`) ou microfones (`eCapture`) ligados agora, marcando o
/// que o Windows está usando.
unsafe fn list_devices(enumerator: &IMMDeviceEnumerator, flow: EDataFlow) -> Result<Vec<Device>> {
    let default = enumerator.GetDefaultAudioEndpoint(flow, eConsole).and_then(|d| device_id(&d)).ok();
    let collection = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
    let mut devices = Vec::new();
    for i in 0..collection.GetCount()? {
        let Ok(device) = collection.Item(i) else { continue };
        let Ok(id) = device_id(&device) else { continue };
        let name = device_name(&device).unwrap_or_else(|_| "Dispositivo".to_string());
        devices.push(Device { default: default.as_ref() == Some(&id), id, name });
    }
    Ok(devices)
}

unsafe fn device_id(device: &IMMDevice) -> Result<String> {
    let id = device.GetId()?;
    let text = id.to_string();
    CoTaskMemFree(Some(id.0 as *const std::ffi::c_void));
    Ok(text.unwrap_or_default())
}

/// Nome como aparece nas configurações de som, ex.: "Fones de ouvido (Realtek(R) Audio)".
unsafe fn device_name(device: &IMMDevice) -> Result<String> {
    let store = device.OpenPropertyStore(STGM_READ)?;
    let value = store.GetValue(&PKEY_Device_FriendlyName)?;
    let text = PropVariantToStringAlloc(&value)?;
    let name = text.to_string().unwrap_or_default();
    CoTaskMemFree(Some(text.0 as *const std::ffi::c_void));
    Ok(name)
}

/// Faz o Windows usar este dispositivo para tudo: jogos e música (`eConsole`,
/// `eMultimedia`) e chamadas (`eCommunications`).
unsafe fn set_default_device(id: &str) -> Result<()> {
    if id.starts_with("fake-") {
        return Ok(());
    }
    let policy: IPolicyConfig = CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)?;
    let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
    for role in [eConsole, eMultimedia, eCommunications] {
        policy::set_default_endpoint(&policy, PCWSTR(wide.as_ptr()), role).ok()?;
    }
    Ok(())
}

/// Dispositivos de mentira para o teste visual; o primeiro está em uso.
fn fake_list(names: &[&str]) -> Vec<Device> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| Device { id: format!("fake-{i}-{name}"), name: name.to_string(), default: i == 0 })
        .collect()
}

fn current_session(manager: Option<&MediaManager>) -> Result<MediaSession> {
    manager.ok_or_else(windows::core::Error::empty)?.GetCurrentSession()
}

fn read_media(manager: &MediaManager) -> Result<Media> {
    let session = manager.GetCurrentSession()?;
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == PlaybackStatus::Playing;
    Ok(Media { title: props.Title()?.to_string(), artist: props.Artist()?.to_string(), playing })
}

/// "C:\...\chrome.exe" -> "Chrome"
fn process_name(pid: u32) -> String {
    let path = unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return format!("PID {pid}");
        };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        if ok.is_err() {
            return format!("PID {pid}");
        }
        String::from_utf16_lossy(&buf[..len as usize])
    };
    let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(path);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => format!("PID {pid}"),
    }
}
