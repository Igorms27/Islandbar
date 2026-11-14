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

/// Área da ilha na tela (esquerda, topo, direita, base), em pixels físicos.
/// A interface publica a cada quadro; quem confere o mouse só lê.
#[cfg(windows)]
static HOVER_AREA: [std::sync::atomic::AtomicI32; 4] = [const { std::sync::atomic::AtomicI32::new(0) }; 4];

#[cfg(windows)]
pub fn set_hover_area(left: f32, top: f32, right: f32, bottom: f32) {
    use std::sync::atomic::Ordering::Relaxed;
    for (slot, value) in HOVER_AREA.iter().zip([left.floor(), top.floor(), right.ceil(), bottom.ceil()]) {
        slot.store(value as i32, Relaxed);
    }
}

/// O mouse está sobre a área publicada em `set_hover_area`?
///
/// É uma consulta baratíssima, e pode ser feita de qualquer thread.
#[cfg(windows)]
pub fn cursor_over_island() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let Some((x, y)) = cursor_pos() else { return false };
    let [left, top, right, bottom] = [0, 1, 2, 3].map(|i| HOVER_AREA[i].load(Relaxed));
    left < right && left <= x && x <= right && top <= y && y <= bottom
}

/// Vigia o mouse e chama `wake` quando ele chega na ilha.
///
/// A janela não recebe o mouse enquanto os cliques a atravessam, então alguém
/// precisa conferir a posição dele de tempos em tempos. Antes era a própria
/// interface, redesenhando-se 20 vezes por segundo só para isso. Agora é uma
/// thread minúscula que só compara dois números, e a interface dorme até ser
/// chamada. Ela fica separada do backend de propósito: se uma consulta ao
/// Windows demorar, a ilha continua respondendo ao mouse na hora.
///
/// De passagem, avisa também quando outra ilha pediu para esta fechar.
pub fn watch_cursor(wake: impl Fn() + Send + 'static) {
    #[cfg(windows)]
    {
        let watcher = std::thread::Builder::new().name("islandbar-mouse".into()).stack_size(64 * 1024);
        let _ = watcher.spawn(move || {
            let mut was_over = false;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let over = cursor_over_island();
                if (over && !was_over) || quit_requested() {
                    wake();
                }
                was_over = over;
            }
        });
    }
    #[cfg(not(windows))]
    let _ = wake;
}

/// Devolve ao Windows a memória que o programa não está usando agora.
///
/// Abrir o programa e abrir o painel usam memória que depois fica parada
/// (código de inicialização, a checagem de atualização, etc.). Isto avisa o
/// Windows que ele pode reaproveitar essas páginas; o que voltar a ser usado
/// é trazido de volta sozinho.
pub fn trim_memory() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// Desloca o cursor `step` pixels na horizontal.
///
/// Serve para o Windows reenviar a posição do mouse à janela que está embaixo
/// dele. Precisa ser um deslocamento de verdade: a biblioteca de janelas
/// ignora um aviso de movimento com a mesma posição do anterior, mesmo que o
/// mouse tenha saído e voltado nesse meio tempo.
pub fn nudge_cursor(step: i32) {
    #[cfg(windows)]
    if let Some((x, y)) = cursor_pos() {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(x + step, y);
        }
    }
    #[cfg(not(windows))]
    let _ = step;
}

#[cfg(windows)]
mod window {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Mutex;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CLOAKED,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::System::SystemInformation::GetTickCount64;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindow, GetWindowLongPtrW, GetWindowRect, IsWindow, IsWindowVisible, SetWindowLongPtrW, SetWindowPos,
        GWL_EXSTYLE, GWL_STYLE, GW_HWNDPREV, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_EX_WINDOWEDGE, WM_NCCALCSIZE, WS_MAXIMIZEBOX,
        WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
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

    /// Intervalo mínimo entre duas conferências da ordem das janelas, em milissegundos.
    const Z_CHECK_EVERY: u64 = 400;
    static LAST_Z_CHECK: AtomicU64 = AtomicU64::new(0);
    /// Janelas grandes que continuam acima da ilha mesmo depois de ela subir.
    /// São as do próprio Windows (ex.: a troca de janelas do Win+Tab), que
    /// vivem numa camada que nenhum programa comum alcança. Não adianta
    /// insistir com elas.
    static UNBEATABLE: Mutex<Vec<isize>> = Mutex::new(Vec::new());

    /// A primeira janela grande e visível, acima da ilha, que cobre o lugar
    /// onde a ilha é desenhada.
    ///
    /// "Grande" = pelo menos metade da tela. Isso pega jogos em tela cheia
    /// (mesmo os que ficam alguns pixels maiores ou menores que o monitor) e
    /// deixa de fora menus, dicas e avisos, que são pequenos.
    unsafe fn large_window_above(hwnd: HWND, ignore: &[isize]) -> Option<HWND> {
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            return None;
        }
        let area = |rect: RECT| (rect.right - rect.left) as i64 * (rect.bottom - rect.top) as i64;
        let half_screen = area(info.rcMonitor) / 2;

        // Um ponto no meio da pílula: centro da janela, logo abaixo do topo.
        let mut own = RECT::default();
        GetWindowRect(hwnd, &mut own).ok()?;
        let (x, y) = ((own.left + own.right) / 2, own.top + (own.bottom - own.top) / 24);

        let mut current = hwnd;
        // O limite é só uma garantia contra uma lista que não termina.
        for _ in 0..256 {
            // "Anterior" na ordem das janelas = a que está logo acima.
            current = GetWindow(current, GW_HWNDPREV).ok().filter(|above| !above.is_invalid())?;
            if ignore.contains(&(current.0 as isize)) || !IsWindowVisible(current).as_bool() {
                continue;
            }
            let mut rect = RECT::default();
            if GetWindowRect(current, &mut rect).is_err() {
                continue;
            }
            let covers_island = rect.left <= x && x < rect.right && rect.top <= y && y < rect.bottom;
            if !covers_island || area(rect) < half_screen {
                continue;
            }
            // "Cloaked": janela que existe mas o Windows não está mostrando.
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(current, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut std::ffi::c_void, 4);
            if cloaked == 0 {
                return Some(current);
            }
        }
        None
    }

    /// Mantém a ilha na frente de jogos em tela cheia.
    ///
    /// A ilha é uma janela "sempre no topo". Só que muitos jogos, ao entrar em
    /// tela cheia, também se marcam assim, e entre duas janelas "sempre no
    /// topo" fica na frente a que subiu por último: o jogo cobria a ilha.
    /// Então, de tempos em tempos, a ilha confere se há uma janela grande
    /// acima dela e, se houver, volta para a frente (sem tirar o foco do jogo).
    ///
    /// A ilha só sobe quando está de fato coberta. Subir a cada quadro, sem
    /// olhar, também funcionaria para os jogos, mas colocaria a ilha na frente
    /// de menus, dicas e avisos do Windows que hoje aparecem por cima dela.
    unsafe fn stay_above_large_windows(hwnd: HWND) {
        let now = GetTickCount64();
        if now.wrapping_sub(LAST_Z_CHECK.load(Ordering::Relaxed)) < Z_CHECK_EVERY {
            return;
        }
        LAST_Z_CHECK.store(now, Ordering::Relaxed);

        let Ok(mut unbeatable) = UNBEATABLE.lock() else { return };
        unbeatable.retain(|window| IsWindow(Some(HWND(*window as *mut std::ffi::c_void))).as_bool());
        let Some(blocker) = large_window_above(hwnd, &unbeatable) else { return };

        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
        if DEBUG_Z.load(Ordering::Relaxed) {
            crate::perf::line(format!("window: raised above large window {:?}", blocker.0));
        }
        if large_window_above(hwnd, &unbeatable) == Some(blocker) && unbeatable.len() < 64 {
            unbeatable.push(blocker.0 as isize);
            if DEBUG_Z.load(Ordering::Relaxed) {
                crate::perf::line(format!("window: {:?} stays above; giving up on it", blocker.0));
            }
        }
    }

    /// ISLANDBAR_DEBUG_UI=1 também registra quando a ilha precisa voltar para a frente.
    static DEBUG_Z: AtomicBool = AtomicBool::new(false);

    pub fn sync(frame: &eframe::Frame, passthrough: bool) {
        let Some(hwnd) = hwnd(frame) else { return };
        unsafe {
            let mut frame_changed = false;
            if !HOOKED.swap(true, Ordering::Relaxed) {
                let _ = SetWindowSubclass(hwnd, Some(window_hook), 1, 0);
                frame_changed = true;
                DEBUG_Z.store(std::env::var_os("ISLANDBAR_DEBUG_UI").is_some(), Ordering::Relaxed);
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

            stay_above_large_windows(hwnd);
        }
    }
}

/// Mantém a janela como uma superfície transparente pura: sem barra de título,
/// sem borda, fora do Alt+Tab, e na frente de jogos em tela cheia. Com
/// `passthrough`, os cliques vão para o programa que está embaixo. Chamada a
/// cada frame; só mexe se algo mudou.
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
