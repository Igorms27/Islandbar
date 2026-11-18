# Teste visual no Windows do GitHub Actions: abre a ilha sobre um fundo
# colorido, simula o mouse e salva screenshots + estilos da janela em smoke/.
# Aceita -Part 1 ou -Part 2 para dividir as rodadas entre duas máquinas em paralelo.
param([ValidateSet("all", "1", "2")][string]$Part = "all")
$run1 = ($Part -eq "all" -or $Part -eq "1")
$run2 = ($Part -eq "all" -or $Part -eq "2")

$ErrorActionPreference = 'Continue'
$out = "smoke"
New-Item -ItemType Directory -Force $out | Out-Null
$exeDir = Resolve-Path "target\release"
function Log($text) { $text | Tee-Object -Append "$out\log.txt" }

# A máquina do GitHub não tem placa de vídeo: usa o OpenGL por software (Mesa).
# (O pacote "mingw" é o que traz o OpenGL de desktop; guarda em mesa-dlls/ para o cache.)
$mesaCache = "mesa-dlls"
if (-not (Test-Path "$mesaCache\opengl32.dll")) {
  New-Item -ItemType Directory -Force $mesaCache | Out-Null
  gh release download --repo pal1000/mesa-dist-win --pattern "mesa3d-*-release-mingw.7z" --dir mesa
  $archive = (Get-ChildItem mesa\*.7z | Select-Object -First 1).FullName
  Log ("mesa: " + (Split-Path $archive -Leaf))
  7z x $archive -omesa\x -y | Out-Null
  Copy-Item mesa\x\x64\*.dll $mesaCache
} else {
  Log "mesa: cached"
}
Copy-Item "$mesaCache\*.dll" $exeDir
Log ("dlls next to exe: " + ((Get-ChildItem $exeDir\*.dll).Name -join ", "))

Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class W {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtrW(IntPtr h, int i);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  public struct RECT { public int L, T, R, B; }
}
"@

function WaitWindow($title, $timeoutMs = 10000) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
    $h = [W]::FindWindowW([NullString]::Value, $title)
    if ($h -ne [IntPtr]::Zero) { return $h }
    Start-Sleep -Milliseconds 100
  }
  [IntPtr]::Zero
}
function WaitLog($file, $pattern, $count = 1, $skip = 0, $timeoutMs = 15000) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
    if (Test-Path $file) {
      $hits = @(Get-Content $file -ErrorAction SilentlyContinue | Select-Object -Skip $skip | Select-String $pattern)
      if ($hits.Count -ge $count) { return $true }
    }
    Start-Sleep -Milliseconds 200
  }
  $false
}
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
# Arrasta com o botão apertado, de ($x1, $y) até ($x2, $y).
function Drag($x1, $y, $x2) {
  [W]::SetCursorPos($x1, $y) | Out-Null
  Start-Sleep -Milliseconds 500
  [W]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
  for ($i = 1; $i -le 12; $i++) {
    [W]::SetCursorPos([int]($x1 + ($x2 - $x1) * $i / 12), $y) | Out-Null
    Start-Sleep -Milliseconds 25
  }
  Start-Sleep -Milliseconds 60
  [W]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
}
# Cor de um ponto da tela.
function Pixel($x, $y) {
  $bmp = New-Object Drawing.Bitmap 1, 1
  $g = [Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
  $c = $bmp.GetPixel(0, 0)
  $g.Dispose(); $bmp.Dispose()
  $c
}
# Pontos da janela da ilha onde a barra de título antiga apareceria (o topo
# da janela, fora da ilha). Fora da ilha a janela é transparente: o ponto tem
# que ter a mesma cor de quando a ilha acabou de abrir.
function TitlePoints($r) { @(@(($r.L + 30), ($r.T + 10)), @(($r.L + 160), ($r.T + 20)), @(($r.R - 30), ($r.T + 10))) }
function TitleBase($r) { TitlePoints $r | ForEach-Object { Pixel $_[0] $_[1] } }
function TitleCheck($label, $r, $base) {
  $now = TitlePoints $r | ForEach-Object { Pixel $_[0] $_[1] }
  $bar = $false
  for ($i = 0; $i -lt $now.Count; $i++) {
    $d = [math]::Abs($now[$i].R - $base[$i].R) + [math]::Abs($now[$i].G - $base[$i].G) + [math]::Abs($now[$i].B - $base[$i].B)
    if ($d -gt 40) { $bar = $true }
  }
  Log ("{0}: barra de titulo: {1} (agora {2}; antes {3})" -f $label, $(if ($bar) { "SIM" } else { "nao" }), (($now | ForEach-Object { "{0},{1},{2}" -f $_.R, $_.G, $_.B }) -join " "), (($base | ForEach-Object { "{0},{1},{2}" -f $_.R, $_.G, $_.B }) -join " "))
}
function Click($x, $y) {
  [W]::SetCursorPos($x, $y) | Out-Null
  Start-Sleep -Milliseconds 500
  [W]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 60
  [W]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
}

Log ("screen: " + [Windows.Forms.Screen]::PrimaryScreen.Bounds)
(New-Object -ComObject Shell.Application).MinimizeAll()
Start-Sleep -Milliseconds 600

function StartIsland($tag, $welcome = $false, $settleMs = 1200) {
  [W]::SetCursorPos(900, 700) | Out-Null
  $env:ISLANDBAR_DEBUG_SYSMON = "1"
  $env:ISLANDBAR_DEBUG_GAME = "1"
  $env:ISLANDBAR_DEBUG_UI = "1"
  $env:ISLANDBAR_DEBUG_PERF = "1"
  # Sem placa de som na máquina do GitHub: botões de saída/microfone de mentira.
  $env:ISLANDBAR_DEBUG_DEVICES = "1"
  # Com --autostart (como no login do Windows) o cartão inicial não aparece.
  $params = @{ FilePath = "$exeDir\islandbar.exe"; PassThru = $true; RedirectStandardError = "$out\$tag-stderr.txt" }
  if (-not $welcome) { $params.ArgumentList = '--autostart' }
  $app = Start-Process @params
  WaitWindow "Islandbar" 10000 | Out-Null
  WaitLog "$out\$tag-stderr.txt" '^System \{' 1 0 8000 | Out-Null
  if ($settleMs -gt 0) { Start-Sleep -Milliseconds $settleMs }
  Log ("$tag app running: " + (-not $app.HasExited) + $(if ($app.HasExited) { " (exit code 0x{0:X8})" -f $app.ExitCode }))
  $app
}
function StopIsland($tag, $app) {
  Stop-Process -Id $app.Id -ErrorAction SilentlyContinue
  $app.WaitForExit(3000) | Out-Null
  Start-Sleep -Milliseconds 200
  Log ("$tag last readings: " + (Get-Content "$out\$tag-stderr.txt" -ErrorAction SilentlyContinue | Select-String 'System' | Select-Object -Last 8 | Out-String))
}

if ($run1) {
# Fundo bem colorido, para qualquer véu/retângulo da janela aparecer.
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
WaitWindow "bg" 8000 | Out-Null
Start-Sleep -Milliseconds 500
Shot "0-background"

# ---- Rodada 1: pílula, painel, clicar fora --------------------------------
$app = StartIsland "island"
$r = Styles "island start"
$base = TitleBase $r
Shot "island-1-collapsed"
# Parada, sem ninguém mexer: é aqui que se mede o consumo em repouso.
# (Mais de 30 s, para pegar também a devolução da memória parada.)
Start-Sleep 34
Log ("perf idle:`n" + (Get-Content "$out\island-stderr.txt" | Select-String '^perf' | Out-String))

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
# Clica no segundo botão de saída de som: o destaque deve ir para ele na hora.
Click ($cx + 19) ($r.T + 110)
Start-Sleep -Milliseconds 300
Shot "island-4b-device-clicked"
# Arrasta duas páginas: Som -> Alarme -> Clima (a previsão de verdade, pela internet).
Drag ($cx + 120) ($r.T + 110) ($cx - 120)
Start-Sleep 1
Drag ($cx + 175) ($r.T + 144) ($cx - 65)
Start-Sleep 1
Shot "island-4c-weather"
Log ("weather lines: " + ((Get-Content "$out\island-stderr.txt" | Select-String '^weather') -join " | "))

# Clica fora (o fundo ganha o foco): era aqui que aparecia a barra de título.
Click 80 600
Start-Sleep -Milliseconds 120
Shot "island-5-clicked-away"
Start-Sleep -Milliseconds 1800
Shot "island-6-collapsed-again"
Styles "island end" | Out-Null
TitleCheck "depois de clicar fora" $r $base

# Ctrl com o mouse em cima: a ilha sai da frente e o clique passa para trás.
[W]::SetCursorPos($cx, $pillY) | Out-Null
Start-Sleep -Milliseconds 700
[W]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)      # Ctrl apertado
Start-Sleep -Milliseconds 300
[W]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)      # Ctrl solto
Start-Sleep -Milliseconds 600
Shot "ghost-1-hidden"
Click $cx $pillY
Start-Sleep -Milliseconds 900
Shot "ghost-2-clicked"
Log ("ghost: ui after click: " + ((Get-Content "$out\island-stderr.txt" | Select-String '^ui:' | Select-Object -Last 3) -join " | "))
[W]::SetCursorPos($cx, 500) | Out-Null
Start-Sleep -Milliseconds 1200
Shot "ghost-3-back"
TitleCheck "depois do Ctrl" $r $base
Start-Sleep 3
TitleCheck "depois do Ctrl (3 s)" $r $base

# Tecla de volume: a ilha a pega antes do Windows (para a barrinha de volume
# do Windows não aparecer). Sem placa de som aqui, ela deixa a tecla passar.
[W]::keybd_event(0xAF, 0, 0, [UIntPtr]::Zero)
[W]::keybd_event(0xAF, 0, 2, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 500
Log ("volume keys: " + ((Get-Content "$out\island-stderr.txt" | Select-String '^volume keys') -join " | "))

# ---- Rodada 2: modo jogo ---------------------------------------------------
# Um "jogo falso" em tela cheia (examples/fake_game.rs) entrega quadros pelo
# DirectX; a ilha deve abrir a pílula do modo jogo com o FPS.
Stop-Process -Id $bg.Id -ErrorAction SilentlyContinue
Start-Sleep 1
# O jogo se marca como "sempre no topo", como muitos jogos em tela cheia: a
# ilha tem que continuar aparecendo por cima dele.
$game = Start-Process "$exeDir\examples\fake_game.exe" -PassThru -ArgumentList '--topmost' -RedirectStandardError "$out\game-stderr.txt"
Start-Sleep 2
Click 512 500   # garante que o jogo está em primeiro plano
Start-Sleep 7
Log ("fake game running: " + (-not $game.HasExited))
Shot "game-1-playing"
# Um ponto dentro da pílula, abaixo do texto: escuro = a ilha está na frente do
# jogo; azul = o jogo cobriu a ilha.
$bmp = New-Object Drawing.Bitmap "$out\game-1-playing.png"
$p = $bmp.GetPixel($cx, $r.T + 40)
$bmp.Dispose()
Log ("island visible over topmost game: {0} (pixel {1},{2},{3})" -f ($p.B -lt 60), $p.R, $p.G, $p.B)
# Jogo rodando, ilha quieta: consumo no modo jogo.
$before = (Get-Content "$out\island-stderr.txt").Count
WaitLog "$out\island-stderr.txt" '^perf:' 2 $before 18000 | Out-Null
Log ("perf in game:`n" + (Get-Content "$out\island-stderr.txt" | Select-Object -Skip $before | Select-String '^perf' | Out-String))
# Clica na ilha várias vezes durante o jogo: o painel tem que abrir em todas.
$opened = 0
foreach ($try in 1..4) {
  [W]::SetCursorPos(900, 700) | Out-Null
  Start-Sleep -Milliseconds 1500
  $before = (Get-Content "$out\island-stderr.txt").Count
  Click $cx $pillY
  Start-Sleep -Milliseconds 1500
  if ($try -eq 1) { Shot "game-2-panel" }
  $lines = Get-Content "$out\island-stderr.txt" | Select-Object -Skip $before
  if ($lines | Select-String 'mode=Expanded') { $opened++ } else { Log "  try ${try}: panel did not open" }
}
Log "clicks during game that opened the panel: $opened of 4"
[W]::SetCursorPos(900, 700) | Out-Null
Start-Sleep 2
Log ("game detection: " + (Get-Content "$out\island-stderr.txt" | Select-String '^game:|^fps:' | Select-Object -Last 4 | Out-String))
Stop-Process -Id $game.Id -ErrorAction SilentlyContinue
Start-Sleep 3
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
Start-Sleep 2
Shot "denied-2-requested"
Log ("group after: " + ((Get-LocalGroupMember -SID 'S-1-5-32-559' -ErrorAction SilentlyContinue).Name -join ", "))
StopIsland "denied" $app
}

if ($run2) {
# ---- Rodada 4: alarme ------------------------------------------------------
# Painel: arrasta para a página do alarme, cancela o alarme de teste, marca
# outro pelo botão Iniciar. Depois uma ilha nova com um alarme curto: o cartão
# tem que aparecer tocando, e o botão Parar tem que fechá-lo.
$env:ISLANDBAR_DEBUG_ALARM = "600"
$app = StartIsland "alarm"
Remove-Item Env:ISLANDBAR_DEBUG_ALARM
$r = Styles "alarm start"
$base = TitleBase $r
$cx = [int](($r.L + $r.R) / 2)
Shot "alarm-1-pill"
Click $cx ($r.T + 27)
Start-Sleep 2
Drag ($cx + 120) ($r.T + 110) ($cx - 120)
Start-Sleep 1
Shot "alarm-2-page-counting"
Click ($cx + 62) ($r.T + 232)   # Cancelar
Start-Sleep 1
Shot "alarm-3-page-choose"
# Clica no número dos minutos e digita 42, Enter: deve começar a contar 42:00.
Click ($cx + 52) ($r.T + 144)
Start-Sleep -Milliseconds 700
[System.Windows.Forms.SendKeys]::SendWait("4")
Start-Sleep -Milliseconds 400
Shot "alarm-3b-typing"
[System.Windows.Forms.SendKeys]::SendWait("2{ENTER}")
Start-Sleep 1
Shot "alarm-4-page-started"
Styles "alarm after typing" | Out-Null
TitleCheck "depois de digitar" $r $base
Log ("keyboard lines: " + ((Get-Content "$out\alarm-stderr.txt" | Select-String '^keyboard') -join " | "))
Click 80 600                    # fecha o painel
Start-Sleep -Milliseconds 800
Shot "alarm-4b-clicked-away"
TitleCheck "depois de digitar e clicar fora" $r $base
Start-Sleep 11
Shot "alarm-5-pill-counting"
TitleCheck "depois de digitar e clicar fora (12 s)" $r $base
Log ("perf with alarm counting:`n" + (Get-Content "$out\alarm-stderr.txt" | Select-String '^perf' | Select-Object -Last 2 | Out-String))
StopIsland "alarm" $app

# O alarme de 42 min tem que voltar ao abrir a ilha de novo (fica no registro).
$saved = (Get-ItemProperty "HKCU:\Software\Islandbar" -Name Alarm -ErrorAction SilentlyContinue).Alarm
Log "alarm saved after closing: $saved"
$task = Get-ScheduledTask -TaskName "Islandbar - alarme" -ErrorAction SilentlyContinue
Log ("wake-up task: " + $(if ($task) { $task.Triggers[0].StartBoundary + " -> " + $task.Actions[0].Arguments } else { "none" }))
$app = StartIsland "alarm-again"
Shot "alarm-8-restored"
Log ("alarm restore: " + ((Get-Content "$out\alarm-again-stderr.txt" | Select-String '^alarm') -join " | "))
StopIsland "alarm-again" $app

$env:ISLANDBAR_DEBUG_ALARM = "1"
$app = StartIsland "ring" $false 0
Remove-Item Env:ISLANDBAR_DEBUG_ALARM
WaitLog "$out\ring-stderr.txt" '^alarm: ringing' 1 0 8000 | Out-Null
Start-Sleep -Milliseconds 1000
Shot "alarm-6-ringing"
Click ($cx + 126) ($r.T + 48)   # Parar
Start-Sleep 2
Shot "alarm-7-stopped"
Log ("alarm lines: " + (Get-Content "$out\ring-stderr.txt" | Select-String '^alarm' | Out-String))
StopIsland "ring" $app
# Depois de tocar, não fica nada guardado.
Log ("alarm saved after ringing: " + (Get-ItemProperty "HKCU:\Software\Islandbar" -Name Alarm -ErrorAction SilentlyContinue).Alarm)
Log ("wake-up task after ringing: " + $(if (Get-ScheduledTask -TaskName "Islandbar - alarme" -ErrorAction SilentlyContinue) { "still there" } else { "none" }))

# Alarme curto e a ilha fechada logo depois: o Windows tem que abri-la na
# hora, já tocando.
$env:ISLANDBAR_DEBUG_ALARM = "12"
$app = StartIsland "closed" $false 200
Remove-Item Env:ISLANDBAR_DEBUG_ALARM
WaitLog "$out\closed-stderr.txt" '^alarm: wake-up task set' 1 0 6000 | Out-Null
Log ("closed lines: " + ((Get-Content "$out\closed-stderr.txt" | Select-String '^alarm') -join " | "))
StopIsland "closed" $app
Log ("islands running after closing: " + @(Get-Process islandbar -ErrorAction SilentlyContinue).Count)
$wokeSw = [Diagnostics.Stopwatch]::StartNew()
while ($wokeSw.ElapsedMilliseconds -lt 25000) {
  if (@(Get-Process islandbar -ErrorAction SilentlyContinue).Count -ge 1) { break }
  Start-Sleep -Milliseconds 250
}
WaitWindow "Islandbar" 8000 | Out-Null
Start-Sleep -Milliseconds 1500
$woke = @(Get-Process islandbar -ErrorAction SilentlyContinue)
Log ("islands running at alarm time: " + $woke.Count + " (expected 1, opened by Windows)")
Shot "alarm-9-woke-ringing"
$woke | Stop-Process -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
Log ("wake-up task after waking: " + $(if (Get-ScheduledTask -TaskName "Islandbar - alarme" -ErrorAction SilentlyContinue) { "still there" } else { "none" }))

# ---- Rodada 5: cartão inicial ----------------------------------------------
# Abre como quem clica no .exe: aparece o cartão. Escolhe "Placa de vídeo",
# liga "Temp" na ilha, escolhe o pet Fantasma e clica em Iniciar. A escolha tem
# que ficar no registro.
Remove-Item "HKCU:\Software\Islandbar" -Recurse -ErrorAction SilentlyContinue
$app = StartIsland "welcome" $true
$r = Styles "welcome start"
$cx = [int](($r.L + $r.R) / 2)
Shot "welcome-1-card"
Click ($cx + 90) ($r.T + 222)    # Temperatura: Placa de vídeo
Start-Sleep -Milliseconds 400
Click ($cx + 104) ($r.T + 128)   # Mostrar na ilha: Temp
Start-Sleep -Milliseconds 400
Click ($cx + 28) ($r.T + 162)    # Pet: Fantasma
Start-Sleep -Milliseconds 600
Shot "welcome-2-chosen"
Click ($cx + 103) ($r.T + 346)   # Pedidos do iFood: Instalar extensão
Start-Sleep 2
$extDir = "$env:LOCALAPPDATA\Islandbar\extensao-ifood"
Log ("extension folder: " + ((Get-ChildItem $extDir -ErrorAction SilentlyContinue | ForEach-Object Name) -join ", ") + " (expected background.js, bridge.js, manifest.json, page.js)")
# Fecha a janela do Explorador que o botão abriu (senão cobre as próximas fotos).
(New-Object -ComObject Shell.Application).Windows() | Where-Object { $_.LocationURL -like '*extensao-ifood*' } | ForEach-Object { $_.Quit() }
Start-Sleep 1
Shot "welcome-2b-extension"
Click ($cx + 126) ($r.T + 410)   # Iniciar
Start-Sleep 2
Shot "welcome-3-pill"
$saved = (Get-ItemProperty "HKCU:\Software\Islandbar" -Name Stats -ErrorAction SilentlyContinue).Stats
Log "settings saved: $saved (expected temp=gpu;show=cpu,gpu,ram,temp;pet=ghost;weather=on)"
Log ("welcome lines: " + ((Get-Content "$out\welcome-stderr.txt" | Select-String '^(store|temp sources)' | Select-Object -Last 4) -join " | "))
StopIsland "welcome" $app
# Aberta de novo pelo login: a escolha continua valendo.
$app = StartIsland "welcome2"
Shot "welcome-4-pet-pill"
# Parada com o pet: ele faz gestos de tempos em tempos (custo em repouso).
WaitLog "$out\welcome2-stderr.txt" '^perf:' 3 0 20000 | Out-Null
Log ("after restart: " + ((Get-Content "$out\welcome2-stderr.txt" | Select-String '^temp sources' | Select-Object -Last 1) -join ""))
Log ("perf idle with pet:`n" + (Get-Content "$out\welcome2-stderr.txt" | Select-String '^perf' | Select-Object -Last 4 | Out-String))
StopIsland "welcome2" $app

# ---- Rodada 6: pedido do iFood ---------------------------------------------
# Faz o papel da extensão do navegador: manda o andamento do pedido para a
# ilha (127.0.0.1:47615). A pílula do pedido aparece à esquerda da ilha.
$app = StartIsland "ifood"
WaitLog "$out\ifood-stderr.txt" '^ifood: listening' 1 0 5000 | Out-Null
function PostOrder($body, $origin = "chrome-extension://smoketest") {
  [IO.File]::WriteAllText("$out\order.txt", $body)
  $curlArgs = @('-s', '-o', 'NUL', '-w', '%{http_code}', '--data-binary', "@$out\order.txt", 'http://127.0.0.1:47615/ifood')
  if ($origin) { $curlArgs = @('-H', "Origin: $origin") + $curlArgs }
  & curl.exe @curlArgs
}
$order = "order=0a1b2c3d-0000-4000-8000-0000000000aa"
Log ("order preparing: " + (PostOrder "$order`nstage=preparing`nstore=Lanche Bom`neta=20:10-20:25`ncode=`nchat=0"))
Start-Sleep 1
Shot "ifood-1-preparing"
Start-Sleep 1
Log ("order shipping: " + (PostOrder "$order`nstage=shipping`nstore=Lanche Bom`neta=20:10-20:25`ncode=4821`nchat=0"))
Start-Sleep 1
# Com tudo: entregador, número e os pontos do mapa (Praça da Sé, em São Paulo).
$full = "$order`nstage=shipping`nstore=Lanche Bom`nnumber=4242`neta=20:10-20:25`ncode=4821`nchat=1`ndriver=João S.`nvehicle=moto`nfrom=-23.55030,-46.63390`nto=-23.56100,-46.65600`ncourier=-23.55500,-46.64300"
Log ("new message: " + (PostOrder $full))
Start-Sleep 1
Shot "ifood-2-shipping-message"
$r = Styles "ifood"
$cx = [int](($r.L + $r.R) / 2)
$pillY = $r.T + 27
# Ctrl com o mouse em cima da pílula do pedido: as duas pílulas saem da frente.
Start-Sleep 1
[W]::SetCursorPos($cx - 205, $pillY) | Out-Null
Start-Sleep -Milliseconds 700
[W]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 300
[W]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 600
Shot "ifood-2b-ctrl-hidden"
[W]::SetCursorPos(900, 700) | Out-Null
Start-Sleep 2
# Clique na pílula do pedido: abre a janela do pedido, com o mapa.
Click ($cx - 205) $pillY
Start-Sleep 5
Shot "ifood-2c-card"
Log ("map lines: " + ((Get-Content "$out\ifood-stderr.txt" | Select-String '^map' | Select-Object -First 3) -join " | "))
[W]::SetCursorPos(900, 700) | Out-Null
Start-Sleep 2
Shot "ifood-2d-closed"
Log ("from a website (expected 403): " + (PostOrder "order=evil`nstage=preparing" "https://example.com"))
Log ("without origin (expected 403): " + (PostOrder "order=evil`nstage=preparing" ""))
# Parada com a pílula do pedido: o custo tem que voltar ao de repouso.
$beforeOrder = (Get-Content "$out\ifood-stderr.txt").Count
WaitLog "$out\ifood-stderr.txt" '^perf:' 3 $beforeOrder 18000 | Out-Null
Log ("perf idle with order:`n" + (Get-Content "$out\ifood-stderr.txt" | Select-Object -Skip $beforeOrder | Select-String '^perf' | Select-Object -Last 2 | Out-String))
Log ("tab closed: " + (PostOrder "gone=0a1b2c3d-0000-4000-8000-0000000000aa"))
Start-Sleep 2
Shot "ifood-3-gone"
# Login do iFood vencido (a extensão manda a cada minuto): a pílula vira o aviso.
Log ("login expired: " + (PostOrder "login=expired"))
Start-Sleep 2
Shot "ifood-3b-login"
Log ("login ok: " + (PostOrder "login=ok"))
Start-Sleep 2
Shot "ifood-3c-login-ok"
Log ("ifood lines: " + ((Get-Content "$out\ifood-stderr.txt" | Select-String '^ifood' | Select-Object -Last 6) -join " | "))
StopIsland "ifood" $app
# A extensão (aqui, o curl fazendo o papel dela) falou com a ilha: fica anotado
# no registro, e o cartão inicial passa a mostrar "Instalada".
Log ("extension seen (registry): " + (Get-ItemProperty "HKCU:\Software\Islandbar" -Name Extension -ErrorAction SilentlyContinue).Extension)
$app = StartIsland "ifood-welcome" $true
Shot "ifood-4-welcome-installed"
StopIsland "ifood-welcome" $app

# ---- Rodada 7: clima, botão "Usar localização do Windows" ------------------
# A previsão real já foi conferida na Rodada 1; aqui usa ISLANDBAR_DEBUG_WEATHER=internet
# para não depender da cota da API externa após abrir a ilha várias vezes seguidas.
# Clicar no botão abre a janela do Windows perguntando; Esc fecha sem permitir.
$env:ISLANDBAR_DEBUG_WEATHER = "internet"
$app = StartIsland "location"
Remove-Item Env:ISLANDBAR_DEBUG_WEATHER
$r = Styles "location"
$cx = [int](($r.L + $r.R) / 2)
Click $cx ($r.T + 27)
Start-Sleep 2
Drag ($cx + 120) ($r.T + 110) ($cx - 120)
Start-Sleep 1
Drag ($cx + 175) ($r.T + 144) ($cx - 65)
Start-Sleep 1
Shot "location-1-button"
Click $cx ($r.T + 308)          # Usar localização do Windows
Start-Sleep 3
Shot "location-2-asked"
[System.Windows.Forms.SendKeys]::SendWait("{ESC}")
Start-Sleep 2
[W]::SetCursorPos($cx, ($r.T + 200)) | Out-Null
Start-Sleep 1
Shot "location-3-answered"
Log ("location lines: " + ((Get-Content "$out\location-stderr.txt" | Select-String '^weather') -join " | "))
StopIsland "location" $app

# ---- Rodada 8: bateria dos periféricos -------------------------------------
# Simula fone sem fio (78%), mouse (64%), teclado carregando (92%) e controle
# 8BitDo com bateria fraca (18%): primeiro aparece o aviso na pílula fechada,
# depois o painel mostra o % no botão de som e a faixinha dos periféricos.
$env:ISLANDBAR_DEBUG_BATTERY = "fake"
$app = StartIsland "battery" $false 1800
Remove-Item Env:ISLANDBAR_DEBUG_BATTERY
$r = Styles "battery"
$cx = [int](($r.L + $r.R) / 2)
Shot "battery-1-low-pill"
Click $cx ($r.T + 27)
Start-Sleep -Milliseconds 1500
Shot "battery-2-panel"
StopIsland "battery" $app
}

exit 0
