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

/// Posição do cursor, para as mensagens de diagnóstico.
pub fn debug_cursor() -> Option<(i32, i32)> {
    #[cfg(windows)]
    return cursor_pos();
    #[cfg(not(windows))]
    None
}

/// Faz o Windows reenviar a posição do mouse para a janela que está embaixo
/// dele. Move o cursor 1 pixel e volta: recolocar no mesmo ponto nem sempre
/// conta como movimento para o Windows.
pub fn nudge_cursor() {
    #[cfg(windows)]
    if let Some((x, y)) = cursor_pos() {
        use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
        unsafe {
            let _ = SetCursorPos(x + 1, y);
            let _ = SetCursorPos(x, y);
        }
    }
}

#[cfg(windows)]
mod window {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION, WS_EX_APPWINDOW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_EX_WINDOWEDGE,
        WM_NCCALCSIZE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    };

    fn hwnd(frame: &eframe::Frame) -> Option<HWND> {
        let handle = frame.window_handle().ok()?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else { return None };
        Some(HWND(handle.hwnd.get() as *mut std::ffi::c_void))
    }

    static HOOKED: AtomicBool = AtomicBool::new(false);

    /// Intercepta uma única mensagem da janela: a que pergunta "quanto da
    /// janela é moldura?". A biblioteca de janelas responde reservando 1 pixel
    /// no topo (para uma sombra que não usamos), e esse pixel aparecia como uma
    /// linha branca. Aqui respondemos: nada é moldura, a janela inteira é nossa.
    unsafe extern "system" fn window_hook(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if message == WM_NCCALCSIZE && wparam.0 != 0 {
            return LRESULT(0);
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }

    pub fn sync(frame: &eframe::Frame, passthrough: bool) {
        let Some(hwnd) = hwnd(frame) else { return };
        unsafe {
            let mut frame_changed = false;
            if !HOOKED.swap(true, Ordering::Relaxed) {
                let _ = SetWindowSubclass(hwnd, Some(window_hook), 1, 0);
                frame_changed = true;
            }

            // Tudo que é de "janela normal": barra de título, botões de
            // fechar/maximizar/minimizar e borda de redimensionar. A biblioteca
            // de janelas (winit) mantém esses estilos escondidos em toda janela,
            // e os recoloca em alguns momentos (ex.: ao mostrar a janela). Com
            // fundo transparente, o Windows os desenhava por trás da ilha: daí
            // o retângulo escuro, o "x" no canto e a barra de título antiga.
            let frame_bits = (WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX).0;
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let wanted = (style & !frame_bits) | WS_POPUP.0;

            // TOOLWINDOW: não aparece no Alt+Tab. NOACTIVATE: clicar na ilha
            // não tira o foco do programa que você está usando.
            // LAYERED fica sempre ligado; com ele, é o TRANSPARENT que decide
            // se os cliques atravessam a janela. Ligar e desligar o LAYERED a
            // cada passada do mouse obrigava o Windows a recriar a janela por
            // dentro, e às vezes ela parava de receber o mouse.
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let mut wanted_ex = (ex_style & !(WS_EX_TRANSPARENT.0 | WS_EX_WINDOWEDGE.0 | WS_EX_APPWINDOW.0))
                | (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED).0;
            if passthrough {
                wanted_ex |= WS_EX_TRANSPARENT.0;
            }

            if ex_style != wanted_ex {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted_ex as i32 as isize);
            }
            if style != wanted {
                frame_changed = true;
                SetWindowLongPtrW(hwnd, GWL_STYLE, wanted as i32 as isize);

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
            }

            if frame_changed {
                // Avisa o Windows para recalcular a moldura.
                let flags = SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
                let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, flags);
            }
        }
    }
}

/// Mantém a janela como uma superfície transparente pura: sem barra de título,
/// sem borda, fora do Alt+Tab. Com `passthrough`, os cliques vão para o
/// programa que está embaixo. Chamada a cada frame; só mexe se algo mudou.
pub fn sync_window(frame: &eframe::Frame, passthrough: bool) {
    #[cfg(windows)]
    window::sync(frame, passthrough);
    #[cfg(not(windows))]
    let _ = (frame, passthrough);
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
