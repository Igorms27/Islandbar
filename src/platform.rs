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
