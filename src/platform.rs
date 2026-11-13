//! Pequenas chamadas diretas ao sistema operacional.

/// Posição do mouse na tela, em pixels físicos.
///
/// Precisamos disso porque a janela fica "transparente a cliques" quando o
/// mouse está fora da ilha, e nesse modo ela não recebe eventos de mouse.
#[cfg(windows)]
pub fn cursor_pos() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point).ok()? };
    Some((point.x, point.y))
}

/// Recorta a janela para ela existir só no retângulo arredondado dado (em
/// pixels, relativo à janela). Fora dele não há nada: nem imagem, nem borda,
/// nem área clicável.
#[cfg(windows)]
pub fn clip_window(frame: &eframe::Frame, rect: [i32; 4], corner_diameter: i32) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};

    let Ok(handle) = frame.window_handle() else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
    let hwnd = HWND(handle.hwnd.get() as *mut std::ffi::c_void);
    let [left, top, right, bottom] = rect;

    unsafe {
        // Sem a borda e os cantos que o Windows 11 desenha em volta das janelas.
        let no_rounding = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &no_rounding as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&no_rounding) as u32,
        );
        let no_border: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &no_border as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&no_border) as u32,
        );

        // O +1 é porque a região não inclui a última linha/coluna.
        let region = CreateRoundRectRgn(left, top, right + 1, bottom + 1, corner_diameter, corner_diameter);
        // Depois desta chamada o Windows passa a ser o dono da região.
        // `false`: não pede para o Windows repintar a área (isso causava
        // piscadas); o próximo frame da ilha já cobre tudo.
        SetWindowRgn(hwnd, Some(region), false);
    }
}

#[cfg(windows)]
mod autostart {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    // Programas listados aqui abrem quando o usuário entra no Windows.
    const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    const VALUE: PCWSTR = w!("Islandbar");

    pub fn enabled() -> bool {
        unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE, RRF_RT_REG_SZ, None, None, None) }.is_ok()
    }

    pub fn set(enabled: bool) {
        unsafe {
            if !enabled {
                let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE);
                return;
            }
            let Ok(exe) = std::env::current_exe() else { return };
            let command = format!("\"{}\" {}", exe.display(), super::AUTOSTART_ARG);
            let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE,
                REG_SZ.0,
                Some(wide.as_ptr() as *const std::ffi::c_void),
                (wide.len() * 2) as u32,
            );
        }
    }
}

/// Argumento que o Windows passa ao abrir a ilha sozinho no login. Com ele,
/// a tela de boas-vindas não aparece.
pub const AUTOSTART_ARG: &str = "--autostart";

/// A ilha está configurada para abrir junto com o Windows?
pub fn autostart_enabled() -> bool {
    #[cfg(windows)]
    return autostart::enabled();
    #[cfg(not(windows))]
    false
}

pub fn set_autostart(enabled: bool) {
    #[cfg(windows)]
    autostart::set(enabled);
    #[cfg(not(windows))]
    let _ = enabled;
}

#[cfg(not(windows))]
pub fn clip_window(_frame: &eframe::Frame, _rect: [i32; 4], _corner_diameter: i32) {}

#[cfg(windows)]
mod instance {
    use std::sync::atomic::{AtomicIsize, Ordering};
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        CreateEventW, CreateMutexW, ResetEvent, SetEvent, WaitForSingleObject,
    };

    /// Evento "pode fechar", compartilhado entre todas as ilhas abertas.
    static QUIT_EVENT: AtomicIsize = AtomicIsize::new(0);

    pub fn take_over() {
        unsafe {
            let Ok(quit) = CreateEventW(None, true, false, w!("Local\\IslandbarQuit")) else { return };
            let mutex = CreateMutexW(None, false, w!("Local\\IslandbarInstance"));
            let already_running = GetLastError() == ERROR_ALREADY_EXISTS;
            if let Ok(mutex) = mutex {
                if already_running {
                    // Pede para a ilha antiga fechar e espera ela sair.
                    let _ = SetEvent(quit);
                    WaitForSingleObject(mutex, 5000);
                    let _ = ResetEvent(quit);
                } else {
                    WaitForSingleObject(mutex, 0);
                }
                // O mutex fica aberto de propósito até o programa terminar.
            }
            QUIT_EVENT.store(quit.0 as isize, Ordering::Relaxed);
        }
    }

    pub fn quit_requested() -> bool {
        let quit = QUIT_EVENT.load(Ordering::Relaxed);
        quit != 0 && unsafe { WaitForSingleObject(HANDLE(quit as *mut std::ffi::c_void), 0) } == WAIT_OBJECT_0
    }
}

/// Garante uma ilha só: se já existe uma aberta, ela é fechada e esta assume.
pub fn take_over_single_instance() {
    #[cfg(windows)]
    instance::take_over();
}

/// Outra ilha (mais nova) abriu e pediu para esta fechar?
pub fn quit_requested() -> bool {
    #[cfg(windows)]
    return instance::quit_requested();
    #[cfg(not(windows))]
    false
}
