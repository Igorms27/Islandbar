Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$f = New-Object Windows.Forms.Form
$f.BackColor = [Drawing.Color]::FromArgb(255, 170, 40)
$f.FormBorderStyle = "None"
$f.WindowState = "Maximized"
$f.Text = "bg"
$f.Add_Shown({ $f.Activate() })
[Windows.Forms.Application]::Run($f)
