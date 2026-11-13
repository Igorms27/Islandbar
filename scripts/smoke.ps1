# Teste visual no Windows do GitHub Actions: abre a ilha sobre um fundo
# colorido, simula o mouse e salva screenshots + estilos da janela em smoke/.
$ErrorActionPreference = 'Continue'
$out = "smoke"
New-Item -ItemType Directory -Force $out | Out-Null
$exeDir = Resolve-Path "target\release"
function Log($text) { $text | Tee-Object -Append "$out\log.txt" }

# A máquina do GitHub não tem placa de vídeo: usa o OpenGL por software (Mesa).
gh release download --repo pal1000/mesa-dist-win --pattern "mesa3d-*-release-msvc.7z" --dir mesa
7z x (Get-ChildItem mesa\*.7z).FullName -omesa\x -y | Out-Null
Copy-Item mesa\x\x64\opengl32.dll, mesa\x\x64\libgallium_wgl.dll $exeDir
Copy-Item mesa\x\x64\libglapi.dll $exeDir -ErrorAction SilentlyContinue

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
  $h = [W]::FindWindowW($null, "Islandbar")
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
$bgCode = 'Add-Type -AssemblyName System.Windows.Forms, System.Drawing; $f = New-Object Windows.Forms.Form; $f.BackColor = [Drawing.Color]::FromArgb(255,170,40); $f.FormBorderStyle = "None"; $f.WindowState = "Maximized"; $f.Text = "bg"; [Windows.Forms.Application]::Run($f)'
$bg = Start-Process powershell -PassThru -ArgumentList '-NoProfile', '-Command', $bgCode
Start-Sleep 4
Shot "0-background"

# Roda a ilha e fotografa cada etapa. $tag separa as duas rodadas.
function Run($tag) {
  [W]::SetCursorPos(900, 700) | Out-Null
  $app = Start-Process "$exeDir\islandbar.exe" -PassThru -ArgumentList '--autostart' -RedirectStandardError "$out\$tag-stderr.txt"
  Start-Sleep 8
  Log ("$tag app running: " + (-not $app.HasExited))

  $r = Styles "$tag start"
  Shot "$tag-1-collapsed"

  $cx = [int](($r.L + $r.R) / 2)
  [W]::SetCursorPos($cx, $r.T + 27) | Out-Null
  Start-Sleep -Milliseconds 700
  Shot "$tag-2-hover"
  Click $cx ($r.T + 27)
  Start-Sleep -Milliseconds 200
  Shot "$tag-3-opening"
  Start-Sleep -Milliseconds 1800
  Shot "$tag-4-expanded"
  Styles "$tag expanded" | Out-Null

  # Clica fora (o fundo ganha o foco): era aqui que aparecia a barra de título.
  Click 80 600
  Start-Sleep -Milliseconds 120
  Shot "$tag-5-clicked-away"
  Start-Sleep -Milliseconds 1800
  Shot "$tag-6-collapsed-again"
  Styles "$tag end" | Out-Null

  Stop-Process -Id $app.Id -ErrorAction SilentlyContinue
  Start-Sleep 1
  Get-Content "$out\$tag-stderr.txt" -ErrorAction SilentlyContinue | Select-Object -First 20
}

# Rodada "a": sem o gancho da janela, para confirmar que o defeito aparece
# neste Windows. Rodada "b": o programa como ele é.
$env:ISLANDBAR_DEBUG_NO_HOOK = "1"
Run "a-nohook"
Remove-Item Env:ISLANDBAR_DEBUG_NO_HOOK
Run "b-normal"

Stop-Process -Id $bg.Id -ErrorAction SilentlyContinue
exit 0
