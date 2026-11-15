//! Teclas de volume do teclado.
//!
//! Quando você aperta uma tecla de volume, o Windows muda o volume e mostra a
//! barrinha dele na tela. A ilha já mostra o próprio aviso, então ficavam dois.
//! Aqui a ilha "pega" essas teclas antes do Windows (um gancho de teclado, o
//! mesmo recurso que programas como o AutoHotkey usam) e muda o volume ela
//! mesma. A tecla não chega ao Windows, e a barrinha dele não aparece.
//!
//! O gancho é chamado a cada tecla apertada no computador, então ele só compara
//! um número e devolve na hora; o trabalho de mudar o volume é mandado para o
//! backend. Se a ilha fechar, o Windows volta a cuidar das teclas sozinho.

use super::{Command, VolumeKey};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_SYSKEYDOWN,
};

/// Para onde as teclas vão (o backend).
static SENDER: OnceLock<Mutex<Sender<Command>>> = OnceLock::new();
/// O backend tem um dispositivo de som aberto. Sem ele a ilha não conseguiria
/// mudar o volume, então as teclas seguem para o Windows como sempre.
static AUDIO_READY: AtomicBool = AtomicBool::new(false);
static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn set_audio_ready(ready: bool) {
    AUDIO_READY.store(ready, Ordering::Relaxed);
}

/// Instala o gancho numa thread própria (ele precisa de uma fila de mensagens).
pub fn start(tx: Sender<Command>) {
    let _ = SENDER.set(Mutex::new(tx));
    DEBUG.store(std::env::var_os("ISLANDBAR_DEBUG_UI").is_some(), Ordering::Relaxed);
    let thread = std::thread::Builder::new().name("islandbar-keys".into()).stack_size(128 * 1024);
    let _ = thread.spawn(|| unsafe {
        let module = GetModuleHandleW(None).ok().map(Into::into);
        match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), module, 0) {
            Ok(_) => crate::perf::line("volume keys: hook installed".to_string()),
            Err(err) => {
                crate::perf::line(format!("volume keys: hook failed: {err}"));
                return;
            }
        }
        // O Windows chama o gancho por esta fila; ela fica parada esperando.
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {}
    });
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let key = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let volume_key = match key.vkCode as u16 {
            vk if vk == VK_VOLUME_UP.0 => Some(VolumeKey::Up),
            vk if vk == VK_VOLUME_DOWN.0 => Some(VolumeKey::Down),
            vk if vk == VK_VOLUME_MUTE.0 => Some(VolumeKey::Mute),
            _ => None,
        };
        if let Some(volume_key) = volume_key {
            if AUDIO_READY.load(Ordering::Relaxed) {
                // Apertar (e repetir segurando) muda o volume; soltar não faz nada.
                let pressed = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
                if pressed {
                    if let Some(Ok(tx)) = SENDER.get().map(Mutex::lock) {
                        let _ = tx.send(Command::VolumeKey(volume_key));
                    }
                }
                // A tecla para aqui: o Windows não a vê, e não mostra a barrinha dele.
                return LRESULT(1);
            }
            if DEBUG.load(Ordering::Relaxed) {
                crate::perf::line(format!("volume keys: {volume_key:?} passed to Windows (no audio device)"));
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}
