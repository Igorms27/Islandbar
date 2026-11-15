//! Atualização automática.
//!
//! Cada versão publicada no GitHub tem um número (o "build"). A cada poucos
//! minutos a ilha pergunta qual é o mais novo; se for maior que o dela, baixa o
//! novo executável, troca o arquivo e reinicia sozinha.
//!
//! A pergunta é baixar um arquivo de texto de poucos bytes (`version.txt`):
//! custa uns milissegundos de CPU e quase nada de internet, então dá para
//! fazer com frequência sem pesar no computador.

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Número desta versão. Só existe nos executáveis gerados pelo GitHub; uma
/// compilação feita na sua máquina não se atualiza sozinha.
pub const BUILD: Option<&str> = option_env!("ISLANDBAR_BUILD");

/// Argumento usado ao reabrir a ilha depois de atualizar.
pub const UPDATED_ARG: &str = "--updated";

const DOWNLOADS: &str = "https://github.com/Igorms27/Islandbar/releases/latest/download";
/// De quanto em quanto tempo procura uma versão nova.
const CHECK_EVERY: Duration = Duration::from_secs(2 * 60);
/// Depois de uma falha (sem internet, por exemplo).
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

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
        let agent = http();
        loop {
            let result = install_if_newer(&agent, current);
            // A conexão segura carrega bibliotecas do Windows na memória;
            // devolve essa memória logo em vez de esperar a ilha ser usada.
            crate::platform::trim_memory();
            match result {
                Ok(true) => {
                    READY.store(true, Ordering::Relaxed);
                    return;
                }
                Ok(false) => std::thread::sleep(CHECK_EVERY),
                // Sem internet, por exemplo: tenta de novo daqui a pouco.
                Err(err) => {
                    eprintln!("islandbar: falha ao atualizar: {err}");
                    std::thread::sleep(RETRY_AFTER);
                }
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
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build(),
        )
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .new_agent()
}

fn latest_build(agent: &ureq::Agent) -> Result<u64> {
    let text = agent.get(format!("{DOWNLOADS}/version.txt")).call()?.body_mut().read_to_string()?;
    Ok(text.trim().parse()?)
}

fn install_if_newer(agent: &ureq::Agent, current: u64) -> Result<bool> {
    if latest_build(agent)? <= current {
        return Ok(false);
    }

    let bytes = agent
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
