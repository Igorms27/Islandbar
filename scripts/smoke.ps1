# Teste visual no Windows do GitHub Actions: abre a ilha sobre um fundo
# colorido, simula o mouse e salva screenshots + estilos da janela em smoke/.
$ErrorActionPreference = 'Continue'
$out = "smoke"
New-Item -ItemType Directory -Force $out | Out-Null
$exeDir = Resolve-Path "target\release"
function Log($text) { $text | Tee-Object -Append "$out\log.txt" }

# A máquina do GitHub não tem placa de vídeo: usa o OpenGL por software (Mesa).
# (O pacote "mingw" é o que traz o OpenGL de desktop.)
gh release download --repo pal1000/mesa-dist-win --pattern "mesa3d-*-release-mingw.7z" --dir mesa
$archive = (Get-ChildItem mesa\*.7z | Select-Object -First 1).FullName
Log ("mesa: " + (Split-Path $archive -Leaf))
7z x $archive -omesa\x -y | Out-Null
Copy-Item mesa\x\x64\*.dll $exeDir
Log ("dlls next to exe: " + ((Get-ChildItem $exeDir\*.dll).Name -join ", "))

Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class W {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtrW(IntPtr h, int i);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  public struct RECT { public int L, T, R, B; }
}
"@

function Shot($name) {
  $b = [Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bmp = New-Object Drawing.Bitmap $b.Width, $b.Height
  $g = [Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen(0, 0, 0, 0, $bmp.Size)
  $bmp.Save("$out\$name.png")
  $g.Dispose(); $bmp.Dispose()
}
function Styles($label) {
  $h = [W]::FindWindowW([NullString]::Value, "Islandbar")
  $r = New-Object W+RECT
  $c = New-Object W+RECT
  [W]::GetWindowRect($h, [ref]$r) | Out-Null
  [W]::GetClientRect($h, [ref]$c) | Out-Null
  $style = [int64][W]::GetWindowLongPtrW($h, -16) -band 0xFFFFFFFFL
  $ex = [int64][W]::GetWindowLongPtrW($h, -20) -band 0xFFFFFFFFL
  Log ("{0}: hwnd={1} style=0x{2:X8} exstyle=0x{3:X8} rect={4},{5},{6},{7} client={8}x{9} foreground={10}" -f $label, $h, $style, $ex, $r.L, $r.T, $r.R, $r.B, $c.R, $c.B, [W]::GetForegroundWindow())
  $r
}
function Click($x, $y) {
  [W]::SetCursorPos($x, $y) | Out-Null
  Start-Sleep -Milliseconds 500
  [W]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 60
  [W]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
}

Log ("screen: " + [Windows.Forms.Screen]::PrimaryScreen.Bounds)

# Fundo bem colorido, para qualquer véu/retângulo da janela aparecer.
(New-Object -ComObject Shell.Application).MinimizeAll()
Start-Sleep 1
@'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$f = New-Object Windows.Forms.Form
$f.BackColor = [Drawing.Color]::FromArgb(255, 170, 40)
$f.FormBorderStyle = "None"
$f.WindowState = "Maximized"
$f.Text = "bg"
$f.Add_Shown({ $f.Activate() })
[Windows.Forms.Application]::Run($f)
'@ | Set-Content "$out\bg.ps1"
$bg = Start-Process powershell -PassThru -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "$out\bg.ps1"
Start-Sleep 4
Shot "0-background"

function StartIsland($tag) {
  [W]::SetCursorPos(900, 700) | Out-Null
  $env:ISLANDBAR_DEBUG_SYSMON = "1"
  $env:ISLANDBAR_DEBUG_GAME = "1"
  $env:ISLANDBAR_DEBUG_UI = "1"
  $app = Start-Process "$exeDir\islandbar.exe" -PassThru -ArgumentList '--autostart' -RedirectStandardError "$out\$tag-stderr.txt"
  Start-Sleep 8
  Log ("$tag app running: " + (-not $app.HasExited) + $(if ($app.HasExited) { " (exit code 0x{0:X8})" -f $app.ExitCode }))
  $app
}
function StopIsland($tag, $app) {
  Stop-Process -Id $app.Id -ErrorAction SilentlyContinue
  Start-Sleep 1
  Log ("$tag last readings: " + (Get-Content "$out\$tag-stderr.txt" -ErrorAction SilentlyContinue | Select-String 'System' | Select-Object -Last 2 | Out-String))
}

# ---- Rodada 1: pílula, painel, clicar fora --------------------------------
$app = StartIsland "island"
$r = Styles "island start"
Shot "island-1-collapsed"

$cx = [int](($r.L + $r.R) / 2)
$pillY = $r.T + 27
[W]::SetCursorPos($cx, $pillY) | Out-Null
Start-Sleep -Milliseconds 700
Shot "island-2-hover"
Click $cx $pillY
Start-Sleep -Milliseconds 200
Shot "island-3-opening"
Start-Sleep -Milliseconds 1800
Shot "island-4-expanded"
Styles "island expanded" | Out-Null

# Clica fora (o fundo ganha o foco): era aqui que aparecia a barra de título.
Click 80 600
Start-Sleep -Milliseconds 120
Shot "island-5-clicked-away"
Start-Sleep -Milliseconds 1800
Shot "island-6-collapsed-again"
Styles "island end" | Out-Null

# ---- Rodada 2: modo jogo ---------------------------------------------------
# Um "jogo falso" em tela cheia (examples/fake_game.rs) entrega quadros pelo
# DirectX; a ilha deve abrir a pílula do modo jogo com o FPS.
Stop-Process -Id $bg.Id -ErrorAction SilentlyContinue
Start-Sleep 1
$game = Start-Process "$exeDir\examples\fake_game.exe" -PassThru -RedirectStandardError "$out\game-stderr.txt"
Start-Sleep 2
Click 512 500   # garante que o jogo está em primeiro plano
Start-Sleep 7
Log ("fake game running: " + (-not $game.HasExited))
Shot "game-1-playing"
"--- click on pill during game ---" | Add-Content "$out\marker.txt"
$before = (Get-Content "$out\island-stderr.txt").Count
Click $cx $pillY
Start-Sleep 2
Shot "game-2-panel"
Log ("ui during game click: " + (Get-Content "$out\island-stderr.txt" | Select-Object -Skip $before | Select-String '^ui:' | Select-Object -First 3 -Last 12 | Out-String))
[W]::SetCursorPos(900, 700) | Out-Null
Start-Sleep 2
Log ("game detection: " + (Get-Content "$out\island-stderr.txt" | Select-String '^game:|^fps:' | Select-Object -Last 12 | Out-String))
Stop-Process -Id $game.Id -ErrorAction SilentlyContinue
Start-Sleep 4
Shot "game-3-after"
StopIsland "island" $app
Log ("fake game stderr: " + (Get-Content "$out\game-stderr.txt" -ErrorAction SilentlyContinue | Out-String))

# ---- Rodada 3: pedir a permissão de FPS ------------------------------------
# Finge que falta a permissão, clica em "Ativar" e confere se o usuário entrou
# no grupo "Usuários de log de desempenho" (S-1-5-32-559).
Log ("group before: " + ((Get-LocalGroupMember -SID 'S-1-5-32-559' -ErrorAction SilentlyContinue).Name -join ", "))
$env:ISLANDBAR_DEBUG_FPS_DENIED = "1"
$app = StartIsland "denied"
Remove-Item Env:ISLANDBAR_DEBUG_FPS_DENIED
Click $cx $pillY
Start-Sleep 2
Shot "denied-1-panel"
# Botão "Ativar": à direita, na linha logo abaixo dos mostradores.
Click ($cx + 156) ($r.T + 262)
Start-Sleep 5
Shot "denied-2-requested"
Log ("group after: " + ((Get-LocalGroupMember -SID 'S-1-5-32-559' -ErrorAction SilentlyContinue).Name -join ", "))
StopIsland "denied" $app

exit 0
