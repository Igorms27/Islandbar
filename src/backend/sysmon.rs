//! Monitor do sistema: CPU, RAM, GPU e temperatura.
//!
//! Nada aqui precisa de administrador nem de driver extra; tudo vem de
//! informações que o próprio Windows já oferece. O preço disso é que a
//! temperatura depende do que cada PC expõe (veja `read_temp`).
//!
//! CPU e RAM custam quase nada e são lidas sempre. GPU e temperatura custam
//! mais, então só são lidas quando pedidas (veja `Wanted`).

use super::{System, Temp, TempSource};
use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Wdk::Graphics::Direct3D::{
    D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo, D3DKMTQueryStatistics,
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO,
    D3DKMT_QUERYSTATISTICS, D3DKMT_QUERYSTATISTICS_ADAPTER, D3DKMT_QUERYSTATISTICS_NODE,
    KMTQAITYPE_ADAPTERPERFDATA,
};
use windows::Win32::Foundation::{CloseHandle, FILETIME, LUID};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    PropertyStandardQuery, StorageDeviceTemperatureProperty, IOCTL_STORAGE_QUERY_PROPERTY,
    STORAGE_PROPERTY_QUERY, STORAGE_TEMPERATURE_DATA_DESCRIPTOR,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::Win32::System::IO::DeviceIoControl;

const GB: f32 = 1024.0 * 1024.0 * 1024.0;
/// Ler a temperatura do disco é um comando para o próprio disco: de vez em quando basta.
const DISK_EVERY: Duration = Duration::from_secs(15);

/// O que vale a pena ler agora. Cada leitura custa um pouco de CPU, então o
/// que não está aparecendo na tela não é lido.
#[derive(Clone, Copy, Debug)]
pub struct Wanted {
    pub gpu: bool,
    pub temp: bool,
}

pub struct SystemMonitor {
    /// (tempo ocioso, tempo total) da última leitura da CPU.
    last_cpu: Option<(u64, u64)>,
    cpu: f32,
    thermal_zones: Lazy,
    gpu_engines: Lazy,
    /// Última leitura de cada um (fica valendo enquanto não é pedida de novo).
    gpu: Option<f32>,
    temp: Option<Temp>,
    /// Uso de GPU (0..1) de cada programa, pelo PID. Atualizado em `read`.
    pub gpu_by_pid: HashMap<u32, f32>,
    /// Soma do uso de cada "motor" da placa: (identificador, porcentagem).
    engines: Vec<(u64, f64)>,
    gpu_adapters: Option<Vec<LUID>>,
    /// Experimento: uso da GPU lido direto do núcleo gráfico (veja `GpuNodes`).
    nodes: GpuNodes,
    pub gpu_kmt: Option<f32>,
    disk_temp: Option<f32>,
    disk_read_at: Option<Instant>,
}

impl SystemMonitor {
    pub fn new() -> Self {
        Self {
            last_cpu: None,
            cpu: 0.0,
            thermal_zones: Lazy::new(w!("\\Thermal Zone Information(*)\\Temperature")),
            gpu_engines: Lazy::new(w!("\\GPU Engine(*)\\Utilization Percentage")),
            gpu: None,
            temp: None,
            gpu_by_pid: HashMap::new(),
            engines: Vec::new(),
            gpu_adapters: None,
            nodes: GpuNodes::default(),
            gpu_kmt: None,
            disk_temp: None,
            disk_read_at: None,
        }
    }

    pub fn read(&mut self, wanted: Wanted) -> System {
        if wanted.gpu {
            let started = Instant::now();
            self.gpu = self.read_gpu();
            crate::perf::add_section("gpu_pdh", started);
            let started = Instant::now();
            self.gpu_kmt = unsafe { self.nodes.read() };
            crate::perf::add_section("gpu_kmt", started);
        } else {
            self.gpu_by_pid.clear();
        }
        if wanted.temp {
            self.temp = self.read_temp();
        }
        let (ram_used_gb, ram_total_gb) = read_ram();
        System { cpu: self.read_cpu(), ram_used_gb, ram_total_gb, gpu: self.gpu, temp: self.temp }
    }

    /// Uso da CPU = fração do tempo, desde a última leitura, em que ela não ficou ociosa.
    fn read_cpu(&mut self) -> f32 {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_err() {
            return self.cpu;
        }
        let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        // O tempo de "kernel" já inclui o tempo ocioso.
        let now = (ticks(idle), ticks(kernel) + ticks(user));
        if let Some(before) = self.last_cpu {
            let total = now.1.saturating_sub(before.1);
            let idle = now.0.saturating_sub(before.0);
            if total > 0 {
                self.cpu = (1.0 - idle as f32 / total as f32).clamp(0.0, 1.0);
            }
        }
        self.last_cpu = Some(now);
        self.cpu
    }

    /// Uso da GPU, do mesmo jeito que o Gerenciador de Tarefas calcula.
    ///
    /// O Windows informa o uso por programa e por "motor" da placa (3D, vídeo,
    /// cópia...). Somamos os programas de cada motor e ficamos com o motor
    /// mais ocupado. De passagem, guarda o uso de cada programa.
    ///
    /// São centenas de linhas por leitura, então os nomes são examinados
    /// direto no formato do Windows (UTF-16), sem criar um texto para cada um.
    fn read_gpu(&mut self) -> Option<f32> {
        self.gpu_by_pid.clear();
        self.engines.clear();
        let just_opened = !self.gpu_engines.opened();
        let counter = self.gpu_engines.get()?;
        if just_opened {
            // Uso é uma "taxa": só existe a partir da segunda coleta.
            return Some(0.0);
        }
        let (engines, by_pid) = (&mut self.engines, &mut self.gpu_by_pid);
        let collected = unsafe {
            counter.read(|name, percent| {
                // "pid_1234_luid_0x0_0xABCD_phys_0_eng_0_engtype_3D"
                let engine = find(name, &LUID).map_or(name, |start| &name[start..]);
                let key = hash(engine);
                match engines.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, total)) => *total += percent,
                    None => engines.push((key, percent)),
                }
                if let Some(pid) = name.strip_prefix(&PID[..]).and_then(leading_number) {
                    let usage = by_pid.entry(pid).or_default();
                    *usage = usage.max((percent / 100.0) as f32);
                }
            })
        };
        if !collected {
            return None;
        }
        let busiest = self.engines.iter().map(|(_, total)| *total).fold(0.0, f64::max);
        Some((busiest / 100.0).clamp(0.0, 1.0) as f32)
    }

    /// Melhor temperatura que este PC oferece, nesta ordem:
    ///
    /// 1. Zona térmica da placa-mãe (notebooks quase sempre têm; muitos
    ///    desktops não).
    /// 2. Placa de vídeo (o mesmo número do Gerenciador de Tarefas; placas
    ///    dedicadas informam, vídeo integrado em geral não).
    /// 3. Disco principal.
    ///
    /// A temperatura exata de cada núcleo do processador só existe através de
    /// um driver, por isso não aparece aqui.
    fn read_temp(&mut self) -> Option<Temp> {
        let zone = self.thermal_zones.get().and_then(|zones| {
            let mut celsius = Vec::new();
            // O contador vem em Kelvin; ficamos com a zona mais quente.
            unsafe { zones.read(|_, kelvin| celsius.push(kelvin as f32 - 273.15)) };
            hottest(celsius.into_iter())
        });
        if let Some(celsius) = zone {
            return Some(Temp { celsius, source: TempSource::System });
        }

        let adapters = self.gpu_adapters.get_or_insert_with(|| unsafe { gpu_adapters(false) });
        let gpu = hottest(adapters.iter().filter_map(|luid| unsafe { read_gpu_temp(*luid) }));
        if let Some(celsius) = gpu {
            return Some(Temp { celsius, source: TempSource::Gpu });
        }

        if self.disk_read_at.is_none_or(|at| at.elapsed() >= DISK_EVERY) {
            self.disk_read_at = Some(Instant::now());
            self.disk_temp = unsafe { read_disk_temp() };
        }
        self.disk_temp.map(|celsius| Temp { celsius, source: TempSource::Disk })
    }
}

const fn wide<const N: usize>(text: &[u8; N]) -> [u16; N] {
    let mut out = [0u16; N];
    let mut i = 0;
    while i < N {
        out[i] = text[i] as u16;
        i += 1;
    }
    out
}

const PID: [u16; 4] = wide(b"pid_");
const LUID: [u16; 5] = wide(b"luid_");

/// Posição de `needle` dentro de `text`.
fn find(text: &[u16], needle: &[u16]) -> Option<usize> {
    text.windows(needle.len()).position(|window| window == needle)
}

/// O número com que `text` começa ("1234_luid..." -> 1234).
fn leading_number(text: &[u16]) -> Option<u32> {
    let digits = text.iter().take_while(|c| (b'0' as u16..=b'9' as u16).contains(c)).count();
    if digits == 0 {
        return None;
    }
    text[..digits].iter().try_fold(0u32, |n, c| n.checked_mul(10)?.checked_add((c - b'0' as u16) as u32))
}

/// Resumo de um nome em um número só (FNV-1a), para comparar rápido.
fn hash(text: &[u16]) -> u64 {
    text.iter().fold(0xcbf2_9ce4_8422_2325, |h, c| (h ^ *c as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Maior valor, ignorando leituras absurdas (sensores inexistentes devolvem 0 ou lixo).
fn hottest(celsius: impl Iterator<Item = f32>) -> Option<f32> {
    celsius.filter(|c| (1.0..125.0).contains(c)).reduce(f32::max)
}

fn read_ram() -> (f32, f32) {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
        return (0.0, 0.0);
    }
    let total = status.ullTotalPhys as f32 / GB;
    let free = status.ullAvailPhys as f32 / GB;
    (total - free, total)
}

/// Um contador do Monitor de Desempenho do Windows com várias instâncias
/// (ex.: uma por zona térmica).
struct Counter {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    /// Espaço para a resposta, reaproveitado entre as leituras.
    /// (Vec<u64> garante o alinhamento que a estrutura precisa.)
    buffer: Vec<u64>,
}

impl Counter {
    unsafe fn open(path: PCWSTR) -> Option<Self> {
        let mut query = PDH_HQUERY::default();
        if PdhOpenQueryW(None, 0, &mut query) != 0 {
            return None;
        }
        let mut counter = PDH_HCOUNTER::default();
        if PdhAddEnglishCounterW(query, path, 0, &mut counter) != 0 {
            return None;
        }
        // Contadores de "taxa" (como o uso da GPU) precisam de uma coleta
        // anterior para comparar; esta é a primeira.
        PdhCollectQueryData(query);
        Some(Self { query, counter, buffer: Vec::new() })
    }

    /// Coleta e chama `each(nome da instância, valor)` para cada instância
    /// com leitura válida. Devolve `false` se a coleta falhou.
    unsafe fn read(&mut self, mut each: impl FnMut(&[u16], f64)) -> bool {
        if PdhCollectQueryData(self.query) != 0 {
            return false;
        }
        let mut count = 0u32;
        // Tenta com o espaço que já temos; se faltar, o Windows diz quanto precisa.
        for _ in 0..3 {
            let mut bytes = (self.buffer.len() * 8) as u32;
            let items = self.buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            let space = (!self.buffer.is_empty()).then_some(items);
            let status = PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, space);
            if status == PDH_MORE_DATA {
                // Com folga, para não repetir a cada programa que abre.
                self.buffer.resize((bytes as usize).div_ceil(8) * 5 / 4, 0);
                continue;
            }
            if status != 0 {
                return false;
            }
            for i in 0..count as usize {
                let item = &*items.add(i);
                // 0 = dado válido, 1 = dado novo; o resto são erros.
                if item.FmtValue.CStatus <= 1 && !item.szName.is_null() {
                    each(item.szName.as_wide(), item.FmtValue.Anonymous.doubleValue);
                }
            }
            return true;
        }
        false
    }
}

/// Um `Counter` aberto só na primeira vez em que alguém precisa dele.
struct Lazy {
    path: PCWSTR,
    /// `None`: ainda não tentou abrir. `Some(None)`: este PC não tem.
    counter: Option<Option<Counter>>,
}

impl Lazy {
    fn new(path: PCWSTR) -> Self {
        Self { path, counter: None }
    }

    /// Já tentou abrir?
    fn opened(&self) -> bool {
        self.counter.is_some()
    }

    /// O contador, aberto agora se for a primeira vez.
    fn get(&mut self) -> Option<&mut Counter> {
        self.counter.get_or_insert_with(|| unsafe { Counter::open(self.path) }).as_mut()
    }
}

/// Uso da GPU lido direto do núcleo gráfico do Windows.
///
/// Cada placa tem "motores" (3D, vídeo, cópia...). O Windows guarda quanto
/// tempo cada motor passou trabalhando; a diferença entre duas leituras,
/// dividida pelo tempo que passou, é o uso. São poucas consultas diretas, em
/// vez de uma lista com centenas de linhas (uma por programa e por motor).
#[derive(Default)]
struct GpuNodes {
    /// (placa, número de motores).
    adapters: Vec<(LUID, u32)>,
    /// Tempo trabalhado de cada motor na leitura anterior, e quando ela foi feita.
    last: Vec<i64>,
    last_at: Option<Instant>,
}

impl GpuNodes {
    unsafe fn read(&mut self) -> Option<f32> {
        if self.adapters.is_empty() {
            self.adapters = gpu_adapters(true).into_iter().filter_map(|luid| Some((luid, node_count(luid)?))).collect();
            self.last_at = None;
        }
        let mut times = Vec::with_capacity(self.last.len());
        for (luid, nodes) in &self.adapters {
            for node in 0..*nodes {
                match node_running_time(*luid, node) {
                    Some(time) => times.push(time),
                    None => {
                        // A lista de placas mudou (driver atualizado, por exemplo).
                        self.adapters.clear();
                        return None;
                    }
                }
            }
        }
        if times.is_empty() {
            return None;
        }
        let now = Instant::now();
        let before = self.last_at.replace(now);
        let last = std::mem::replace(&mut self.last, times);
        // Os tempos vêm em unidades de 100 nanossegundos.
        let elapsed = (now - before?).as_nanos() as f64 / 100.0;
        if last.len() != self.last.len() || elapsed <= 0.0 {
            return None;
        }
        let busiest = self.last.iter().zip(&last).map(|(now, before)| (now - before) as f64 / elapsed).fold(0.0, f64::max);
        Some(busiest.clamp(0.0, 1.0) as f32)
    }
}

unsafe fn node_count(luid: LUID) -> Option<u32> {
    let mut query = D3DKMT_QUERYSTATISTICS { Type: D3DKMT_QUERYSTATISTICS_ADAPTER, AdapterLuid: luid, ..Default::default() };
    D3DKMTQueryStatistics(std::ptr::addr_of_mut!(query)).is_ok().then(|| query.QueryResult.AdapterInformation.NodeCount)
}

unsafe fn node_running_time(luid: LUID, node: u32) -> Option<i64> {
    let mut query = D3DKMT_QUERYSTATISTICS { Type: D3DKMT_QUERYSTATISTICS_NODE, AdapterLuid: luid, ..Default::default() };
    query.Anonymous.QueryNode.NodeId = node;
    D3DKMTQueryStatistics(std::ptr::addr_of_mut!(query))
        .is_ok()
        .then(|| query.QueryResult.NodeInformation.GlobalInformation.RunningTime)
}

/// Identificadores das placas de vídeo. Sem `software`, só as de verdade
/// (fica de fora a "virtual" que o Windows usa quando não há placa).
unsafe fn gpu_adapters(software: bool) -> Vec<LUID> {
    let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return Vec::new() };
    let mut adapters = Vec::new();
    let mut index = 0;
    while let Ok(adapter) = factory.EnumAdapters1(index) {
        index += 1;
        if let Ok(desc) = adapter.GetDesc1() {
            if software || desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 {
                adapters.push(desc.AdapterLuid);
            }
        }
    }
    adapters
}

/// Temperatura que o driver da placa de vídeo informa ao Windows, em °C.
unsafe fn read_gpu_temp(luid: LUID) -> Option<f32> {
    let mut open = D3DKMT_OPENADAPTERFROMLUID { AdapterLuid: luid, hAdapter: 0 };
    if D3DKMTOpenAdapterFromLuid(&mut open).is_err() {
        return None;
    }
    let mut perf = D3DKMT_ADAPTER_PERFDATA::default();
    let mut query = D3DKMT_QUERYADAPTERINFO {
        hAdapter: open.hAdapter,
        Type: KMTQAITYPE_ADAPTERPERFDATA,
        pPrivateDriverData: &mut perf as *mut _ as *mut c_void,
        PrivateDriverDataSize: std::mem::size_of::<D3DKMT_ADAPTER_PERFDATA>() as u32,
    };
    let status = D3DKMTQueryAdapterInfo(&mut query);
    let _ = D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER { hAdapter: open.hAdapter });
    if status.is_err() {
        return None;
    }
    // Vem em décimos de grau.
    Some(perf.Temperature as f32 / 10.0)
}

/// Temperatura do disco principal, em °C.
unsafe fn read_disk_temp() -> Option<f32> {
    // Acesso "0": só consulta informações, não lê nem escreve no disco.
    let disk = CreateFileW(
        w!("\\\\.\\PhysicalDrive0"),
        0,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
        OPEN_EXISTING,
        FILE_FLAGS_AND_ATTRIBUTES(0),
        None,
    )
    .ok()?;

    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceTemperatureProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut buffer = [0u64; 64];
    let mut returned = 0u32;
    let result = DeviceIoControl(
        disk,
        IOCTL_STORAGE_QUERY_PROPERTY,
        Some(&query as *const _ as *const c_void),
        std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
        Some(buffer.as_mut_ptr() as *mut c_void),
        std::mem::size_of_val(&buffer) as u32,
        Some(&mut returned),
        None,
    );
    let _ = CloseHandle(disk);
    result.ok()?;

    if (returned as usize) < std::mem::size_of::<STORAGE_TEMPERATURE_DATA_DESCRIPTOR>() {
        return None;
    }
    let descriptor = &*(buffer.as_ptr() as *const STORAGE_TEMPERATURE_DATA_DESCRIPTOR);
    if descriptor.InfoCount == 0 {
        return None;
    }
    hottest(std::iter::once(descriptor.TemperatureInfo[0].Temperature as f32))
}
