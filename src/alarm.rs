//! Alarme: "toca daqui a X horas/minutos".
//!
//! Tudo aqui é só conta de relógio; quem pergunta "já deu a hora?" é a
//! interface, que também se programa para acordar no segundo certo. Assim o
//! alarme não gasta nada enquanto espera.
//!
//! O horário marcado fica guardado no registro (veja `settings::store`): se a
//! ilha fechar ou reiniciar para se atualizar, o alarme continua ao abrir.
//! E, para tocar mesmo com a ilha fechada, a ilha deixa uma tarefa no
//! Agendador de Tarefas do Windows que a abre na hora (veja `wakeup`).

use crate::settings::store;
use std::time::{Duration, Instant};

/// Nome do valor guardado no registro.
const STORE: &str = "Alarm";
/// Se a hora passou com a ilha fechada há pouco, toca ao abrir. Mais que
/// isso (ex.: PC desligado de noite), o alarme é esquecido.
const LATE_LIMIT: Duration = Duration::from_secs(10 * 60);

/// Sem ninguém mexer, o som para sozinho depois disso (o aviso continua).
const RING_FOR: Duration = Duration::from_secs(5 * 60);
pub const SNOOZE_MINUTES: u32 = 5;
/// Limite das setinhas: 23 h 55 min.
const MAX_MINUTES: u32 = 23 * 60 + 55;

pub struct Alarm {
    /// Tempo escolhido na página do alarme, em minutos.
    pub chosen: u32,
    /// Quando vai tocar, e o mesmo horário no relógio (para "Toca às 19:40").
    due: Option<(Instant, chrono::DateTime<chrono::Local>)>,
    /// Desde quando está tocando (o aviso aparece até alguém clicar).
    ringing: Option<Instant>,
    /// O som ainda está tocando.
    sounding: bool,
    /// Horário marcado do alarme que está tocando.
    rang: Option<String>,
}

impl Alarm {
    pub fn new() -> Self {
        Self { chosen: 15, due: None, ringing: None, sounding: false, rang: None }
    }

    /// O alarme guardado (se havia um), ou um alarme de teste
    /// (ISLANDBAR_DEBUG_ALARM=<segundos>).
    pub fn load() -> Self {
        let mut alarm = Self::new();
        if let Some(seconds) = std::env::var("ISLANDBAR_DEBUG_ALARM").ok().and_then(|s| s.parse::<u64>().ok()) {
            alarm.set_in(Duration::from_secs(seconds));
            return alarm;
        }
        let saved = store::read(STORE).and_then(|text| text.trim().parse::<i64>().ok());
        let Some(at) = saved.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)) else { return alarm };
        let at = at.with_timezone(&chrono::Local);
        let now = chrono::Local::now();
        match (at - now).to_std() {
            // Ainda vai tocar: continua contando de onde parou.
            Ok(left) => alarm.due = Some((Instant::now() + left, at)),
            // Passou há pouco com a ilha fechada: toca já.
            Err(_) if (now - at).to_std().is_ok_and(|late| late <= LATE_LIMIT) => {
                alarm.due = Some((Instant::now(), at));
            }
            // Passou faz tempo: esquece.
            Err(_) => store::delete(STORE),
        }
        crate::perf::line(format!("alarm: restored {} (counting: {})", at.format("%H:%M:%S"), alarm.counting()));
        alarm
    }

    /// Ajusta o tempo escolhido (as setinhas e os atalhos).
    pub fn change(&mut self, minutes: i32) {
        self.chosen = (self.chosen as i32 + minutes).clamp(0, MAX_MINUTES as i32) as u32;
    }

    /// Define o tempo escolhido (o que foi digitado). Passou do limite, fica no máximo.
    pub fn set_parts(&mut self, hours: u32, minutes: u32) {
        self.chosen = (hours.min(23) * 60 + minutes.min(59)).min(MAX_MINUTES);
    }

    /// Começa a contar o tempo escolhido.
    pub fn start(&mut self) {
        if self.chosen > 0 {
            self.set_in(Duration::from_secs(self.chosen as u64 * 60));
        }
    }

    fn set_in(&mut self, wait: Duration) {
        let at = chrono::Local::now() + chrono::Duration::from_std(wait).unwrap_or_default();
        self.due = Some((Instant::now() + wait, at));
        self.save();
    }

    /// Guarda o horário marcado (ou apaga, se não há alarme).
    fn save(&self) {
        match self.due {
            Some((_, at)) => store::write(STORE, &at.timestamp().to_string()),
            None => store::delete(STORE),
        }
        wakeup::schedule(self.due.map(|(_, at)| at));
    }

    /// Soma minutos a um alarme que já está contando.
    pub fn add(&mut self, minutes: u32) {
        if let Some((due, at)) = &mut self.due {
            *due += Duration::from_secs(minutes as u64 * 60);
            *at += chrono::Duration::minutes(minutes as i64);
        }
        self.save();
    }

    pub fn cancel(&mut self) {
        self.due = None;
        self.save();
    }

    /// Para o aviso e o som.
    pub fn stop(&mut self) {
        self.ringing = None;
        self.silence();
    }

    /// Para agora e toca de novo daqui a alguns minutos.
    pub fn snooze(&mut self) {
        self.stop();
        self.set_in(Duration::from_secs(SNOOZE_MINUTES as u64 * 60));
    }

    /// Chamada a cada quadro. Devolve `true` no instante em que o alarme dispara.
    pub fn check(&mut self, now: Instant) -> bool {
        if self.sounding && self.ringing.is_some_and(|since| now - since >= RING_FOR) {
            self.silence();
        }
        match self.due {
            Some((due, at)) if now >= due => {
                self.due = None;
                self.save();
                self.rang = Some(at.format("%H:%M").to_string());
                self.ringing = Some(now);
                self.sounding = true;
                sound::play();
                true
            }
            _ => false,
        }
    }

    fn silence(&mut self) {
        if self.sounding {
            self.sounding = false;
            sound::stop();
        }
    }

    /// Está contando?
    pub fn counting(&self) -> bool {
        self.due.is_some()
    }

    /// O som ainda está tocando?
    pub fn sounding(&self) -> bool {
        self.sounding
    }

    pub fn ringing(&self) -> bool {
        self.ringing.is_some()
    }

    /// Quanto falta para tocar.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.due.map(|(due, _)| due.saturating_duration_since(now))
    }

    /// Horário em que tocou (para o cartão), ex.: "19:40".
    pub fn rang_clock(&self) -> Option<String> {
        self.rang.clone()
    }

    /// Horário marcado, ex.: "19:40".
    pub fn due_clock(&self) -> Option<String> {
        self.due.map(|(_, at)| at.format("%H:%M").to_string())
    }
}

impl Drop for Alarm {
    fn drop(&mut self) {
        self.silence();
    }
}

/// "1:14:59" ou "14:59".
pub fn countdown(left: Duration) -> String {
    let seconds = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Curto, para a pílula: "1h15", "25 min", "45 s".
pub fn short(left: Duration) -> String {
    let seconds = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    if seconds < 60 {
        return format!("{seconds} s");
    }
    let minutes = seconds.div_ceil(60);
    if minutes < 60 {
        format!("{minutes} min")
    } else {
        format!("{}h{:02}", minutes / 60, minutes % 60)
    }
}

/// Quanto a ilha pode dormir sem que o texto da pílula fique errado.
pub fn next_change(left: Duration) -> Duration {
    if left.as_secs() < 60 {
        // Último minuto: os segundos aparecem.
        Duration::from_millis(left.subsec_millis() as u64 + 1)
    } else {
        // Só os minutos aparecem: até o próximo minuto virar.
        Duration::from_millis(left.as_millis() as u64 % 60_000 + 1)
    }
}

/// Tarefa do Agendador de Tarefas do Windows que abre a ilha na hora do
/// alarme (com `--alarm`), caso ela esteja fechada. Se a ilha estiver aberta,
/// a cópia aberta pela tarefa vê isso e fecha na hora (veja `main.rs`), e a
/// ilha aberta toca o alarme como sempre.
///
/// A tarefa é da conta do usuário (não precisa de administrador), só roda com
/// ele conectado e expira 10 minutos depois da hora (o mesmo limite de
/// `LATE_LIMIT`): com o PC desligado ou dormindo, não abre a ilha à toa depois.
mod wakeup {
    /// Argumento com que o Windows abre a ilha na hora do alarme.
    pub const ARG: &str = "--alarm";

    #[cfg(windows)]
    pub fn schedule(at: Option<chrono::DateTime<chrono::Local>>) {
        use std::sync::mpsc::{channel, Sender};
        use std::sync::{Mutex, OnceLock};
        // Uma thread só, que faz os pedidos na ordem (marcar e logo cancelar
        // não pode terminar com a tarefa criada). Fica parada esperando.
        static WORKER: OnceLock<Mutex<Sender<Option<chrono::DateTime<chrono::Local>>>>> = OnceLock::new();
        let worker = WORKER.get_or_init(|| {
            let (tx, rx) = channel();
            let _ =
                std::thread::Builder::new().name("islandbar-wakeup".into()).stack_size(256 * 1024).spawn(move || {
                    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
                    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                    while let Ok(at) = rx.recv() {
                        let result = unsafe { apply(at) };
                        crate::perf::line(match (&result, at) {
                            (Ok(()), Some(at)) => format!("alarm: wake-up task set for {}", at.format("%H:%M:%S")),
                            (Ok(()), None) => "alarm: wake-up task removed".to_string(),
                            (Err(err), _) => format!("alarm: wake-up task failed: {err}"),
                        });
                    }
                });
            Mutex::new(tx)
        });
        if let Ok(tx) = worker.lock() {
            let _ = tx.send(at);
        }
    }

    #[cfg(windows)]
    const NAME: &str = "Islandbar - alarme";

    #[cfg(windows)]
    unsafe fn apply(at: Option<chrono::DateTime<chrono::Local>>) -> windows::core::Result<()> {
        use windows::core::BSTR;
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        use windows::Win32::System::TaskScheduler::{
            ITaskService, TaskScheduler, TASK_CREATE_OR_UPDATE, TASK_LOGON_INTERACTIVE_TOKEN,
        };
        use windows::Win32::System::Variant::VARIANT;

        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let none = VARIANT::default();
        service.Connect(&none, &none, &none, &none)?;
        let folder = service.GetFolder(&BSTR::from("\\"))?;
        match at {
            Some(at) => {
                let exe = std::env::current_exe().map_err(|_| windows::core::Error::from_thread())?;
                let xml = task_xml(at, &exe.display().to_string());
                folder.RegisterTask(
                    &BSTR::from(NAME),
                    &BSTR::from(xml),
                    TASK_CREATE_OR_UPDATE.0,
                    &none,
                    &none,
                    TASK_LOGON_INTERACTIVE_TOKEN,
                    &none,
                )?;
                Ok(())
            }
            // Sem tarefa para apagar também está certo.
            None => match folder.DeleteTask(&BSTR::from(NAME), 0) {
                Err(err) if err.code() == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.to_hresult() => Ok(()),
                other => other,
            },
        }
    }

    /// A tarefa, no formato do Agendador. Datas no horário local, sem fuso.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn task_xml(at: chrono::DateTime<chrono::Local>, exe: &str) -> String {
        let start = at.format("%Y-%m-%dT%H:%M:%S");
        let end = (at + chrono::Duration::from_std(super::LATE_LIMIT).unwrap_or_default()).format("%Y-%m-%dT%H:%M:%S");
        let exe = exe.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Abre a Islandbar na hora do alarme marcado nela, se ela estiver fechada. Some sozinha depois.</Description></RegistrationInfo>
  <Triggers><TimeTrigger><StartBoundary>{start}</StartBoundary><EndBoundary>{end}</EndBoundary><Enabled>true</Enabled></TimeTrigger></Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <DeleteExpiredTaskAfter>PT0S</DeleteExpiredTaskAfter>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>{arg}</Arguments></Exec></Actions>
</Task>"#,
            arg = ARG
        )
    }

    #[cfg(not(windows))]
    pub fn schedule(at: Option<chrono::DateTime<chrono::Local>>) {
        let _ = at;
    }
}

pub use wakeup::ARG as WAKE_ARG;

/// O bip-bip de despertador, gerado na hora (não precisa de arquivo).
mod sound {
    /// Um ciclo do som: 4 bipes curtos e uma pausa. Toca em repetição.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn wav() -> Vec<u8> {
        const RATE: u32 = 22_050;
        const FREQ: f32 = 2_000.0;
        let beep = RATE as usize * 9 / 100; // 90 ms
        let gap = RATE as usize * 7 / 100; // 70 ms
        let pause = RATE as usize * 45 / 100; // 450 ms
        let mut samples: Vec<i16> = Vec::new();
        for _ in 0..4 {
            for i in 0..beep {
                let t = i as f32 / RATE as f32;
                // Começo e fim suaves (sem estalo), um pouco de harmônico para
                // soar como despertador, e volume moderado.
                let edge = (i.min(beep - 1 - i) as f32 / (RATE as f32 * 0.004)).min(1.0);
                let tone = (t * FREQ * std::f32::consts::TAU).sin() * 0.8
                    + (t * FREQ * 2.0 * std::f32::consts::TAU).sin() * 0.2;
                samples.push((tone * edge * 0.45 * i16::MAX as f32) as i16);
            }
            samples.extend(std::iter::repeat_n(0, gap));
        }
        samples.extend(std::iter::repeat_n(0, pause));

        let data = (samples.len() * 2) as u32;
        let mut wav = Vec::with_capacity(44 + data as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // tamanho do bloco "fmt"
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&RATE.to_le_bytes());
        wav.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes por segundo
        wav.extend_from_slice(&2u16.to_le_bytes()); // bytes por amostra
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits por amostra
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        wav
    }

    #[cfg(windows)]
    pub fn play() {
        use std::sync::OnceLock;
        use windows::core::PCWSTR;
        use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_LOOP, SND_MEMORY, SND_NODEFAULT};
        // Precisa continuar existindo enquanto toca, então fica guardado.
        static WAV: OnceLock<Vec<u8>> = OnceLock::new();
        let wav = WAV.get_or_init(wav);
        let ok = unsafe {
            PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_LOOP | SND_NODEFAULT)
        };
        crate::perf::line(format!("alarm: ringing (sound started: {})", ok.as_bool()));
    }

    #[cfg(windows)]
    pub fn stop() {
        use windows::core::PCWSTR;
        use windows::Win32::Media::Audio::{PlaySoundW, SND_FLAGS};
        unsafe {
            let _ = PlaySoundW(PCWSTR::null(), None, SND_FLAGS(0));
        }
        crate::perf::line("alarm: sound stopped".to_string());
    }

    #[cfg(not(windows))]
    pub fn play() {
        crate::perf::line("alarm: ringing (sem som fora do Windows)".to_string());
    }

    #[cfg(not(windows))]
    pub fn stop() {
        crate::perf::line("alarm: sound stopped".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textos_do_tempo() {
        assert_eq!(countdown(Duration::from_secs(4499)), "1:14:59");
        assert_eq!(countdown(Duration::from_millis(59_500)), "1:00");
        assert_eq!(short(Duration::from_secs(4500)), "1h15");
        assert_eq!(short(Duration::from_secs(1490)), "25 min");
        assert_eq!(short(Duration::from_millis(44_200)), "45 s");
    }

    #[test]
    fn tarefa_do_windows() {
        use chrono::TimeZone;
        let at = chrono::Local.with_ymd_and_hms(2026, 11, 15, 22, 9, 59).unwrap();
        let xml = wakeup::task_xml(at, r"C:\Jogos & Apps\islandbar.exe");
        assert!(xml.contains("<StartBoundary>2026-11-15T22:09:59</StartBoundary>"));
        assert!(xml.contains("<EndBoundary>2026-11-15T22:19:59</EndBoundary>"));
        assert!(xml.contains(r"<Command>C:\Jogos &amp; Apps\islandbar.exe</Command>"));
        assert!(xml.contains("<Arguments>--alarm</Arguments>"));
    }

    #[test]
    fn setinhas_respeitam_limites() {
        let mut alarm = Alarm::new();
        alarm.change(-100);
        assert_eq!(alarm.chosen, 0);
        alarm.change(10_000);
        assert_eq!(alarm.chosen, MAX_MINUTES);
    }
}
