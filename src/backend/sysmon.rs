//! Monitor do sistema: CPU, RAM e temperatura.

use super::System;

use windows::core::w;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

const GB: f32 = 1024.0 * 1024.0 * 1024.0;

pub struct SystemMonitor {
    /// (tempo ocioso, tempo total) da última leitura da CPU.
    last_cpu: Option<(u64, u64)>,
    cpu: f32,
    thermal: Option<(PDH_HQUERY, PDH_HCOUNTER)>,
}

impl SystemMonitor {
    pub fn new() -> Self {
        Self { last_cpu: None, cpu: 0.0, thermal: unsafe { open_thermal() } }
    }

    pub fn read(&mut self) -> System {
        let (ram_used_gb, ram_total_gb) = read_ram();
        System {
            cpu: self.read_cpu(),
            ram_used_gb,
            ram_total_gb,
            temp_c: self.thermal.and_then(|(query, counter)| unsafe { read_thermal(query, counter) }),
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

/// A temperatura vem das "zonas térmicas" que a placa-mãe informa ao Windows
/// (o mesmo dado do Monitor de Desempenho). Não precisa de administrador, mas
/// nem todo PC fornece.
unsafe fn open_thermal() -> Option<(PDH_HQUERY, PDH_HCOUNTER)> {
    let mut query = PDH_HQUERY::default();
    if PdhOpenQueryW(None, 0, &mut query) != 0 {
        return None;
    }
    let mut counter = PDH_HCOUNTER::default();
    let path = w!("\\Thermal Zone Information(*)\\Temperature");
    if PdhAddEnglishCounterW(query, path, 0, &mut counter) != 0 {
        return None;
    }
    Some((query, counter))
}

/// Maior temperatura entre as zonas térmicas, em °C.
unsafe fn read_thermal(query: PDH_HQUERY, counter: PDH_HCOUNTER) -> Option<f32> {
    if PdhCollectQueryData(query) != 0 {
        return None;
    }
    let (mut bytes, mut count) = (0u32, 0u32);
    if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None) != PDH_MORE_DATA {
        return None;
    }
    // Vec<u64> garante o alinhamento que a estrutura precisa.
    let mut buffer = vec![0u64; (bytes as usize).div_ceil(8)];
    let items = buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, Some(items)) != 0 {
        return None;
    }
    (0..count as usize)
        .map(|i| (*items.add(i)).FmtValue.Anonymous.doubleValue as f32 - 273.15)
        .filter(|celsius| (1.0..125.0).contains(celsius))
        .reduce(f32::max)
}
