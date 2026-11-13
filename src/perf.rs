//! Medidor interno de desempenho. Só liga com ISLANDBAR_DEBUG_PERF=1.
//!
//! A cada 5 segundos escreve no stderr quantos quadros a ilha desenhou por
//! segundo, quanto de CPU o programa gastou (em % de um núcleo) e a memória.
//! Serve para comparar o antes e o depois de uma otimização com números.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_secs(5);
/// Unidades de tempo de CPU por segundo (cada uma vale 100 nanossegundos).
const TICKS_PER_SECOND: f64 = 10_000_000.0;

/// Tempo de CPU já gasto pela thread do backend. Ela mesma atualiza.
static BACKEND_CPU: AtomicU64 = AtomicU64::new(0);
/// Tempo que o backend passou trabalhando (acordado), nas mesmas unidades.
/// Inclui a espera pelas respostas do Windows, que também é trabalho de alguém.
static BACKEND_BUSY: AtomicU64 = AtomicU64::new(0);

/// Partes do trabalho do backend, medidas uma a uma.
pub const SECTIONS: [&str; 6] = ["audio", "gpu_pdh", "gpu_kmt", "game", "media", "volume"];
static SECTION_TIME: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// Soma à parte `name` o tempo passado desde `started`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn add_section(name: &str, started: Instant) {
    if enabled() {
        if let Some(index) = SECTIONS.iter().position(|section| *section == name) {
            SECTION_TIME[index].fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
    }
}

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("ISLANDBAR_DEBUG_PERF").is_some())
}

/// Chamada pela thread do backend, de vez em quando.
pub fn record_backend_thread() {
    if enabled() {
        BACKEND_CPU.store(sys::thread_cpu(), Ordering::Relaxed);
    }
}

/// Chamada pela thread do backend ao fim de cada volta de trabalho.
pub fn add_backend_busy(started: Instant) {
    if enabled() {
        BACKEND_BUSY.fetch_add((started.elapsed().as_nanos() / 100) as u64, Ordering::Relaxed);
    }
}

pub struct Perf {
    since: Instant,
    frames: u32,
    process_cpu: u64,
    backend_cpu: u64,
    backend_busy: u64,
}

impl Perf {
    pub fn new() -> Self {
        Self { since: Instant::now(), frames: 0, process_cpu: sys::process_cpu(), backend_cpu: 0, backend_busy: 0 }
    }

    /// Chamada uma vez por quadro desenhado. `state` descreve o que a ilha
    /// estava fazendo, para dar contexto aos números.
    pub fn frame(&mut self, state: impl FnOnce() -> String) {
        if !enabled() {
            return;
        }
        self.frames += 1;
        let elapsed = self.since.elapsed();
        if elapsed < EVERY {
            return;
        }
        let seconds = elapsed.as_secs_f64();
        let process_cpu = sys::process_cpu();
        let backend_cpu = BACKEND_CPU.load(Ordering::Relaxed);
        let backend_busy = BACKEND_BUSY.load(Ordering::Relaxed);
        let percent = |now: u64, before: u64| now.saturating_sub(before) as f64 / TICKS_PER_SECOND / seconds * 100.0;
        let (working_set, private) = sys::memory();
        let megabytes = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        eprintln!(
            "perf: fps={:.1} cpu={:.2}% backend={:.2}% backend_busy={:.3}% ram={:.1}MB private={:.1}MB | {}",
            self.frames as f64 / seconds,
            percent(process_cpu, self.process_cpu),
            percent(backend_cpu, self.backend_cpu),
            percent(backend_busy, self.backend_busy),
            megabytes(working_set),
            megabytes(private),
            state()
        );
        // Milissegundos de trabalho por segundo em cada parte do backend.
        let sections: Vec<String> = SECTIONS
            .iter()
            .zip(&SECTION_TIME)
            .map(|(name, time)| format!("{name}={:.2}", time.swap(0, Ordering::Relaxed) as f64 / 1e6 / seconds))
            .collect();
        eprintln!("perf backend ms/s: {}", sections.join(" "));
        self.since = Instant::now();
        self.frames = 0;
        self.process_cpu = process_cpu;
        self.backend_cpu = backend_cpu;
        self.backend_busy = backend_busy;
    }
}

#[cfg(windows)]
mod sys {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, GetProcessTimes, GetThreadTimes,
    };

    fn ticks(time: FILETIME) -> u64 {
        ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
    }

    /// Tempo de CPU do programa inteiro (todas as threads).
    pub fn process_cpu() -> u64 {
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        match unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) } {
            Ok(()) => ticks(kernel) + ticks(user),
            Err(_) => 0,
        }
    }

    /// Tempo de CPU da thread que chamou.
    pub fn thread_cpu() -> u64 {
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        match unsafe { GetThreadTimes(GetCurrentThread(), &mut created, &mut exited, &mut kernel, &mut user) } {
            Ok(()) => ticks(kernel) + ticks(user),
            Err(_) => 0,
        }
    }

    /// (memória em uso agora, memória só deste programa), em bytes.
    pub fn memory() -> (u64, u64) {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let pointer = &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS;
        if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), pointer, size) }.as_bool() {
            (counters.WorkingSetSize as u64, counters.PrivateUsage as u64)
        } else {
            (0, 0)
        }
    }
}

#[cfg(not(windows))]
mod sys {
    /// O Linux conta o tempo de CPU em "tiques" de 10 ms.
    const TICK: u64 = 100_000;

    /// Soma dos campos "utime" e "stime" de um arquivo /proc/.../stat.
    fn cpu_from(path: &str) -> u64 {
        let Ok(stat) = std::fs::read_to_string(path) else { return 0 };
        // O nome do programa vem entre parênteses e pode ter espaços.
        let Some(rest) = stat.rsplit(')').next() else { return 0 };
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let field = |i: usize| fields.get(i).and_then(|f| f.parse::<u64>().ok()).unwrap_or(0);
        (field(11) + field(12)) * TICK
    }

    pub fn process_cpu() -> u64 {
        cpu_from("/proc/self/stat")
    }

    pub fn thread_cpu() -> u64 {
        cpu_from("/proc/thread-self/stat")
    }

    pub fn memory() -> (u64, u64) {
        let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else { return (0, 0) };
        let pages: Vec<u64> = statm.split_whitespace().filter_map(|f| f.parse().ok()).collect();
        let page = |i: usize| pages.get(i).copied().unwrap_or(0) * 4096;
        // Residente, e residente menos o que é compartilhado com outros programas.
        (page(1), page(1).saturating_sub(page(2)))
    }
}
