//! Atualização automática.
//!
//! Cada versão publicada no GitHub tem um número (o "build"). De tempos em
//! tempos a ilha pergunta qual é o mais novo; se for maior que o dela, baixa o
//! novo executável, troca o arquivo e reinicia sozinha.

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Número desta versão. Só existe nos executáveis gerados pelo GitHub; uma
/// compilação feita na sua máquina não se atualiza sozinha.
pub const BUILD: Option<&str> = option_env!("ISLANDBAR_BUILD");

/// Argumento usado ao reabrir a ilha depois de atualizar.
pub const UPDATED_ARG: &str = "--updated";

const DOWNLOADS: &str = "https://github.com/Igorms27/Islandbar/releases/latest/download";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

static READY: AtomicBool = AtomicBool::new(false);

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn start() {
    // Sobra da atualização anterior.
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }

    let Some(current) = BUILD.and_then(|b| b.parse::<u64>().ok()) else { return };
    let _ = std::thread::Builder::new().name("islandbar-updater".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(15));
        loop {
            match install_if_newer(current) {
                Ok(true) => {
                    READY.store(true, Ordering::Relaxed);
                    return;
                }
                Ok(false) => std::thread::sleep(CHECK_EVERY),
                // Sem internet, por exemplo: tenta de novo daqui a pouco.
                Err(_) => std::thread::sleep(RETRY_AFTER),
            }
        }
    });
}

/// Já existe uma versão nova instalada, esperando a ilha reiniciar?
pub fn ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// Abre a versão nova. Ela mesma pede para esta fechar.
pub fn restart() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg(UPDATED_ARG).spawn();
    }
}

/// Cliente HTTP usando a criptografia (TLS) do próprio sistema operacional.
fn http() -> ureq::Agent {
    use ureq::tls::{TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .tls_config(TlsConfig::builder().provider(TlsProvider::NativeTls).build())
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .new_agent()
}

fn latest_build() -> Result<u64> {
    let text = http().get(format!("{DOWNLOADS}/version.txt")).call()?.body_mut().read_to_string()?;
    Ok(text.trim().parse()?)
}

fn install_if_newer(current: u64) -> Result<bool> {
    if latest_build()? <= current {
        return Ok(false);
    }

    let bytes = http()
        .get(format!("{DOWNLOADS}/islandbar.exe"))
        .call()?
        .body_mut()
        .with_config()
        .limit(200 * 1024 * 1024)
        .read_to_vec()?;
    // Confere se o que chegou é mesmo um executável do Windows.
    if bytes.len() < 500_000 || !bytes.starts_with(b"MZ") {
        return Err("download inválido".into());
    }

    // O Windows não deixa apagar um programa aberto, mas deixa renomear:
    // o atual vira ".old" e o novo assume o nome dele.
    let exe = std::env::current_exe()?;
    let new = exe.with_extension("exe.new");
    let old = exe.with_extension("exe.old");
    std::fs::write(&new, bytes)?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old)?;
    if let Err(err) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(err.into());
    }
    Ok(true)
}
