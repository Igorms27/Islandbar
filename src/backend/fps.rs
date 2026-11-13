//! Medição de FPS de outros programas.
//!
//! O Windows registra um evento cada vez que um programa entrega um quadro
//! para a tela ("Present"). Abrimos uma sessão de rastreamento (ETW), ouvimos
//! só esses eventos e contamos quantos cada programa gerou. É o mesmo
//! mecanismo da Xbox Game Bar e do PresentMon; nada é injetado no jogo.
//!
//! O Windows só permite isso para administradores ou para quem está no grupo
//! "Usuários de log de desempenho": por isso existe `request_access`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use base64::Engine;
use windows::core::{w, GUID, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_SUCCESS};
use windows::Win32::System::Diagnostics::Etw::{
    CloseTrace, ControlTraceW, EnableTraceEx2, OpenTraceW, ProcessTrace, StartTraceW,
    CONTROLTRACE_HANDLE, ENABLE_TRACE_PARAMETERS, ENABLE_TRACE_PARAMETERS_VERSION_2,
    EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_FILTER_DESCRIPTOR, EVENT_FILTER_TYPE_EVENT_ID,
    EVENT_RECORD, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES,
    EVENT_TRACE_REAL_TIME_MODE, PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_REAL_TIME,
    TRACE_LEVEL_VERBOSE, WNODE_FLAG_TRACED_GUID,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

const SESSION: PCWSTR = w!("IslandbarFps");

// Quem registra os eventos, e o número do evento "entregou um quadro" em cada um.
/// DirectX 10/11/12.
const DXGI: GUID = GUID::from_u128(0xCA11C036_0102_4A2D_A6AD_F03CFED5D3C9);
const DXGI_PRESENTS: [u16; 2] = [42, 55];
/// DirectX 9.
const D3D9: GUID = GUID::from_u128(0x783ACA0A_790E_4D7F_8451_AA850511C6B9);
const D3D9_PRESENTS: [u16; 1] = [1];
/// Núcleo gráfico do Windows: pega também Vulkan e OpenGL.
const DXGKRNL: GUID = GUID::from_u128(0x802EC45A_1E99_4B83_9920_87C98277BA9D);
const DXGKRNL_PRESENTS: [u16; 1] = [184];

/// Quadros entregues por um programa desde a última leitura.
#[derive(Clone, Copy, Debug, Default)]
pub struct Presents {
    /// Contados pelo DirectX.
    runtime: u32,
    /// Contados pelo núcleo gráfico.
    kernel: u32,
}

impl Presents {
    /// Um mesmo quadro pode aparecer nas duas contagens; vale a maior.
    pub fn frames(&self) -> u32 {
        self.runtime.max(self.kernel)
    }
}

type Counts = Mutex<HashMap<u32, Presents>>;

#[derive(Debug)]
pub enum StartError {
    AccessDenied,
    Other(u32),
}

/// Descrição da sessão, no formato que o Windows pede: a estrutura seguida
/// do espaço para o nome.
#[repr(C)]
struct Properties {
    base: EVENT_TRACE_PROPERTIES,
    name: [u16; 64],
}

fn properties() -> Properties {
    let mut props: Properties = unsafe { std::mem::zeroed() };
    props.base.Wnode.BufferSize = std::mem::size_of::<Properties>() as u32;
    props.base.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    props.base.Wnode.ClientContext = 1; // relógio de alta precisão
    props.base.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
    props.base.LoggerNameOffset = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
    props
}

/// Encerra a sessão, se existir. Uma sessão continua viva mesmo se o programa
/// que a abriu fechar, então chamamos isto ao iniciar e ao sair.
pub fn stop_session() {
    let mut props = properties();
    unsafe {
        let _ = ControlTraceW(CONTROLTRACE_HANDLE::default(), SESSION, &mut props.base, EVENT_TRACE_CONTROL_STOP);
    }
}

unsafe fn start_session() -> Result<CONTROLTRACE_HANDLE, StartError> {
    let mut session = CONTROLTRACE_HANDLE::default();
    let mut props = properties();
    let mut status = StartTraceW(&mut session, SESSION, &mut props.base);
    if status == ERROR_ALREADY_EXISTS {
        // Sobrou de uma execução anterior.
        stop_session();
        props = properties();
        status = StartTraceW(&mut session, SESSION, &mut props.base);
    }
    match status {
        ERROR_SUCCESS => Ok(session),
        ERROR_ACCESS_DENIED => Err(StartError::AccessDenied),
        other => Err(StartError::Other(other.0)),
    }
}

/// Dá para medir FPS neste PC, com este usuário?
pub fn has_access() -> bool {
    match unsafe { start_session() } {
        Ok(_) => {
            stop_session();
            true
        }
        Err(StartError::AccessDenied) => false,
        // Outro tipo de erro não é falta de permissão; não adianta pedir.
        Err(StartError::Other(_)) => true,
    }
}

/// Liga um provedor de eventos na sessão, pedindo só os eventos de `ids`.
unsafe fn enable(session: CONTROLTRACE_HANDLE, provider: &GUID, ids: &[u16]) {
    // Formato EVENT_FILTER_EVENT_ID: FilterIn (1 byte), reservado (1 byte),
    // quantidade (u16) e a lista de números.
    let mut filter: Vec<u16> = vec![1, ids.len() as u16];
    filter.extend_from_slice(ids);
    let mut descriptor = EVENT_FILTER_DESCRIPTOR {
        Ptr: filter.as_ptr() as u64,
        Size: (filter.len() * 2) as u32,
        Type: EVENT_FILTER_TYPE_EVENT_ID,
    };
    let params = ENABLE_TRACE_PARAMETERS {
        Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
        EnableFilterDesc: &mut descriptor,
        FilterDescCount: 1,
        ..Default::default()
    };
    let level = TRACE_LEVEL_VERBOSE as u8;
    let code = EVENT_CONTROL_CODE_ENABLE_PROVIDER.0;
    let status = EnableTraceEx2(session, provider, code, level, 0, 0, 0, Some(&params));
    if status != ERROR_SUCCESS {
        // Sem o filtro chegam mais eventos, mas `on_event` descarta o resto.
        let _ = EnableTraceEx2(session, provider, code, level, 0, 0, 0, None);
    }
}

/// Chamada pelo Windows a cada evento recebido.
unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    let record = &*record;
    let header = &record.EventHeader;
    let id = header.EventDescriptor.Id;
    let provider = header.ProviderId;
    let kernel = provider == DXGKRNL && DXGKRNL_PRESENTS.contains(&id);
    let runtime = (provider == DXGI && DXGI_PRESENTS.contains(&id)) || (provider == D3D9 && D3D9_PRESENTS.contains(&id));
    if !kernel && !runtime {
        return;
    }
    let counts = &*(record.UserContext as *const Counts);
    if let Ok(mut counts) = counts.lock() {
        let entry = counts.entry(header.ProcessId).or_default();
        if kernel {
            entry.kernel += 1;
        } else {
            entry.runtime += 1;
        }
    }
}

/// Sessão de rastreamento ativa. Ao ser descartada, encerra tudo.
pub struct FpsTracer {
    session: CONTROLTRACE_HANDLE,
    counts: Arc<Counts>,
    reader: Option<JoinHandle<()>>,
}

impl FpsTracer {
    pub fn start() -> Result<Self, StartError> {
        unsafe {
            let session = start_session()?;
            enable(session, &DXGI, &DXGI_PRESENTS);
            enable(session, &D3D9, &D3D9_PRESENTS);
            enable(session, &DXGKRNL, &DXGKRNL_PRESENTS);

            let counts = Arc::new(Counts::default());
            // O leitor recebe um ponteiro cru para o contador; a referência
            // extra é devolvida quando ele termina.
            let context = Arc::into_raw(counts.clone()) as usize;
            let reader = std::thread::Builder::new().name("islandbar-fps".into()).spawn(move || {
                let mut name: Vec<u16> = SESSION.as_wide().iter().copied().chain(std::iter::once(0)).collect();
                let mut log = EVENT_TRACE_LOGFILEW { LoggerName: PWSTR(name.as_mut_ptr()), ..Default::default() };
                log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
                log.Anonymous2.EventRecordCallback = Some(on_event);
                log.Context = context as *mut c_void;

                let trace = OpenTraceW(&mut log);
                if trace.Value != u64::MAX {
                    // Fica aqui dentro, recebendo eventos, até a sessão ser encerrada.
                    let _ = ProcessTrace(&[trace], None, None);
                    let _ = CloseTrace(trace);
                }
                drop(Arc::from_raw(context as *const Counts));
            });
            let reader = match reader {
                Ok(reader) => reader,
                Err(_) => {
                    drop(Arc::from_raw(context as *const Counts));
                    stop_session();
                    return Err(StartError::Other(0));
                }
            };
            Ok(Self { session, counts, reader: Some(reader) })
        }
    }

    /// Quadros por programa (PID) desde a última chamada.
    pub fn take(&self) -> HashMap<u32, Presents> {
        self.counts.lock().map(|mut counts| std::mem::take(&mut *counts)).unwrap_or_default()
    }
}

impl Drop for FpsTracer {
    fn drop(&mut self) {
        let mut props = properties();
        unsafe {
            let _ = ControlTraceW(self.session, PCWSTR::null(), &mut props.base, EVENT_TRACE_CONTROL_STOP);
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Pede a permissão para medir FPS: coloca o usuário atual no grupo
/// "Usuários de log de desempenho" (o mesmo que a Xbox Game Bar usa).
/// O Windows mostra a tela de confirmação de administrador, e a mudança só
/// vale depois de sair e entrar de novo na conta.
pub fn request_access() {
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(name)) => format!("{domain}\\{name}"),
        (_, Ok(name)) => name,
        _ => return,
    };
    // S-1-5-32-559 identifica o grupo em qualquer idioma do Windows.
    let script = format!("Add-LocalGroupMember -SID 'S-1-5-32-559' -Member '{}'", user.replace('\'', "''"));
    // O PowerShell aceita o comando codificado, o que evita problemas com aspas.
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    let arguments = HSTRING::from(format!("-NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand {encoded}"));
    unsafe {
        // "runas" = executar como administrador.
        ShellExecuteW(None, w!("runas"), w!("powershell.exe"), &arguments, PCWSTR::null(), SW_HIDE);
    }
}
