//! Monitor do sistema: CPU, RAM, GPU e temperatura.
//!
//! Nada aqui precisa de administrador nem de driver extra; tudo vem de
//! informações que o próprio Windows já oferece. O preço disso é que a
//! temperatura depende do que cada PC expõe (veja `read_temp`).

use super::{System, Temp, TempSource};
use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Wdk::Graphics::Direct3D::{
    D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo, D3DKMT_ADAPTER_PERFDATA,
    D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO,
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

pub struct SystemMonitor {
    /// (tempo ocioso, tempo total) da última leitura da CPU.
    last_cpu: Option<(u64, u64)>,
    cpu: f32,
    thermal_zones: Option<Counter>,
    gpu_engines: Option<Counter>,
    /// Uso de GPU (0..1) de cada programa, pelo PID. Atualizado em `read`.
    pub gpu_by_pid: HashMap<u32, f32>,
    gpu_adapters: Vec<LUID>,
    disk_temp: Option<f32>,
    disk_read_at: Option<Instant>,
}

impl SystemMonitor {
    pub fn new() -> Self {
        unsafe {
            Self {
                last_cpu: None,
                cpu: 0.0,
                thermal_zones: Counter::open(w!("\\Thermal Zone Information(*)\\Temperature")),
                gpu_engines: Counter::open(w!("\\GPU Engine(*)\\Utilization Percentage")),
                gpu_by_pid: HashMap::new(),
                gpu_adapters: gpu_adapters(),
                disk_temp: None,
                disk_read_at: None,
            }
        }
    }

    pub fn read(&mut self) -> System {
        let (ram_used_gb, ram_total_gb) = read_ram();
        System {
            cpu: self.read_cpu(),
            ram_used_gb,
            ram_total_gb,
            gpu: self.read_gpu(),
            temp: self.read_temp(),
        }
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
    fn read_gpu(&mut self) -> Option<f32> {
        self.gpu_by_pid.clear();
        let values = unsafe { self.gpu_engines.as_ref()?.read() }?;
        let mut engines: HashMap<&str, f64> = HashMap::new();
        for (name, percent) in &values {
            // "pid_1234_luid_0x0_0xABCD_phys_0_eng_0_engtype_3D"
            let engine = name.find("luid_").map_or(name.as_str(), |start| &name[start..]);
            *engines.entry(engine).or_default() += percent;

            let pid = name.strip_prefix("pid_").and_then(|rest| rest.split('_').next()).and_then(|pid| pid.parse().ok());
            if let Some(pid) = pid {
                let usage = self.gpu_by_pid.entry(pid).or_default();
                *usage = usage.max((*percent / 100.0) as f32);
            }
        }
        let busiest = engines.into_values().fold(0.0, f64::max);
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
        let zone = self.thermal_zones.as_ref().and_then(|zones| {
            let values = unsafe { zones.read() }?;
            // O contador vem em Kelvin; ficamos com a zona mais quente.
            hottest(values.iter().map(|(_, kelvin)| *kelvin as f32 - 273.15))
        });
        if let Some(celsius) = zone {
            return Some(Temp { celsius, source: TempSource::System });
        }

        let gpu = hottest(self.gpu_adapters.iter().filter_map(|luid| unsafe { read_gpu_temp(*luid) }));
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
        Some(Self { query, counter })
    }

    /// (nome da instância, valor) de cada instância com leitura válida.
    unsafe fn read(&self) -> Option<Vec<(String, f64)>> {
        if PdhCollectQueryData(self.query) != 0 {
            return None;
        }
        let (mut bytes, mut count) = (0u32, 0u32);
        if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None) != PDH_MORE_DATA {
            return None;
        }
        // Vec<u64> garante o alinhamento que a estrutura precisa.
        let mut buffer = vec![0u64; (bytes as usize).div_ceil(8)];
        let items = buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, Some(items)) != 0 {
            return None;
        }
        let mut values = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let item = &*items.add(i);
            // 0 = dado válido, 1 = dado novo; o resto são erros.
            if item.FmtValue.CStatus <= 1 {
                let name = item.szName.to_string().unwrap_or_default();
                values.push((name, item.FmtValue.Anonymous.doubleValue));
            }
        }
        Some(values)
    }
}

/// Identificadores das placas de vídeo de verdade (sem as "virtuais" do Windows).
unsafe fn gpu_adapters() -> Vec<LUID> {
    let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return Vec::new() };
    let mut adapters = Vec::new();
    let mut index = 0;
    while let Ok(adapter) = factory.EnumAdapters1(index) {
        index += 1;
        if let Ok(desc) = adapter.GetDesc1() {
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 {
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
