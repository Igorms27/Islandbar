//! "Jogo falso" usado só pelo teste visual (scripts/smoke.ps1).
//!
//! Abre uma janela sem borda do tamanho da tela e fica entregando quadros
//! pelo DirectX 11, como um jogo em tela cheia faria. Serve para conferir no
//! Windows do GitHub se a ilha entra no modo jogo e mede o FPS. Fecha sozinho
//! depois de alguns segundos.

// Sem janela de console, como um jogo de verdade.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use std::time::{Duration, Instant};
    use windows::core::w;
    use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDeviceAndSwapChain, ID3D11Texture2D, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::{
        DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetSystemMetrics, PeekMessageW, PostQuitMessage,
        RegisterClassW, SetForegroundWindow, TranslateMessage, MSG, PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN,
        WINDOW_EX_STYLE, WM_DESTROY, WM_QUIT, WNDCLASSW, WS_POPUP, WS_VISIBLE,
    };

    unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if message == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        DefWindowProcW(window, message, wparam, lparam)
    }

    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("IslandbarFakeGame");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        });
        let (width, height) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Jogo Falso"),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = SetForegroundWindow(window);

        let description = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC {
                Width: width as u32,
                Height: height as u32,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ..Default::default()
            },
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            OutputWindow: window,
            Windowed: true.into(),
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            Flags: 0,
        };
        let (mut swap_chain, mut device, mut context) = (None, None, None);
        // WARP = DirectX por software: funciona sem placa de vídeo.
        D3D11CreateDeviceAndSwapChain(
            None,
            D3D_DRIVER_TYPE_WARP,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&description),
            Some(&mut swap_chain),
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let (swap_chain, device, context) = (swap_chain.unwrap(), device.unwrap(), context.unwrap());
        let back_buffer: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
        let mut target = None;
        device.CreateRenderTargetView(&back_buffer, None, Some(&mut target))?;
        let target = target.unwrap();

        let start = Instant::now();
        let mut message = MSG::default();
        'frames: while start.elapsed() < Duration::from_secs(30) {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_QUIT {
                    break 'frames;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let pulse = (start.elapsed().as_secs_f32() * 2.0).sin() * 0.5 + 0.5;
            context.ClearRenderTargetView(&target, &[0.08, 0.15 + 0.1 * pulse, 0.45, 1.0]);
            swap_chain.Present(1, DXGI_PRESENT(0)).ok()?;
            // Limita o ritmo caso o Present não espere pela tela.
            std::thread::sleep(Duration::from_millis(8));
        }
    }
    Ok(())
}
