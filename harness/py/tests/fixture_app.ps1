# WinForms fixture for desktop_test. Modes: good | broken | hang
param([string]$Mode = "good", [string]$StateFile = "$env:TEMP\kritic-desktop-fixture.json",
      [Parameter(ValueFromRemainingArguments = $true)] $Rest)
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.Application]::EnableVisualStyles()

$form = New-Object System.Windows.Forms.Form
$form.Text = "Kritic Fixture ($Mode)"
$form.Size = New-Object System.Drawing.Size(420, 360)
$form.StartPosition = "CenterScreen"

$status = New-Object System.Windows.Forms.Label
$status.Text = "Status: idle"; $status.Location = "20,15"; $status.AutoSize = $true; $status.Name = "status"
$form.Controls.Add($status)

$name = New-Object System.Windows.Forms.TextBox
$name.Location = "20,45"; $name.Width = 200; $name.Name = "nameBox"; $name.AccessibleName = "Display name"
$form.Controls.Add($name)

$dark = New-Object System.Windows.Forms.CheckBox
$dark.Text = "Dark mode"; $dark.Location = "20,80"; $dark.Name = "darkBox"
$form.Controls.Add($dark)

$size = New-Object System.Windows.Forms.ComboBox
$size.Location = "20,115"; $size.Name = "sizeBox"; $size.AccessibleName = "Size"; $size.DropDownStyle = "DropDownList"
[void]$size.Items.AddRange(@("Small", "Medium", "Large")); $size.SelectedIndex = 0
$form.Controls.Add($size)

function Add-Button($text, $x, $y, $handler) {
    $b = New-Object System.Windows.Forms.Button
    $b.Text = $text; $b.Location = "$x,$y"; $b.Width = 110
    if ($handler) { $b.Add_Click($handler) }
    $form.Controls.Add($b)
}

# persistence (good mode only)
if ($Mode -eq "good" -and (Test-Path $StateFile)) {
    $s = Get-Content $StateFile -Raw | ConvertFrom-Json
    $name.Text = $s.name; $dark.Checked = [bool]$s.dark
}
$save = { if ($Mode -eq "good") { @{ name = $name.Text; dark = $dark.Checked } | ConvertTo-Json | Set-Content $StateFile } }
$name.Add_TextChanged($save); $dark.Add_CheckedChanged($save)

Add-Button "Save" 250 45 { $status.Text = "Status: saved" }

# modal settings dialog (multi-step flow target)
Add-Button "Settings..." 250 250 {
    $dlg = New-Object System.Windows.Forms.Form
    $dlg.Text = "Settings"; $dlg.Size = New-Object System.Drawing.Size(260, 160)
    $nick = New-Object System.Windows.Forms.TextBox
    $nick.Location = "15,15"; $nick.Width = 200; $nick.Name = "nickBox"; $nick.AccessibleName = "Nickname"
    $ok = New-Object System.Windows.Forms.Button
    $ok.Text = "OK"; $ok.Location = "15,50"
    $ok.Add_Click({ $status.Text = "Status: nick=" + $nick.Text; $dlg.Close() })
    $dlg.Controls.Add($nick); $dlg.Controls.Add($ok)
    [void]$dlg.ShowDialog($form)
}

if ($Mode -eq "broken") {
    Add-Button "Load report" 250 80 {
        [System.Windows.Forms.MessageBox]::Show("Unhandled exception: Object reference not set to an instance of an object.", "Application Error") | Out-Null
    }
    Add-Button "Does nothing" 250 115 $null
    Add-Button "Log warning" 250 150 { [Console]::Error.WriteLine("ERROR: widget cache failed to load") ; $status.Text = "Status: logged" }
    Add-Button "Delete all data" 250 185 { $form.Controls.Clear() }
    Add-Button "Zz crash" 250 220 { [Environment]::Exit(3) }
}
if ($Mode -eq "unitysim") {
    # stands in for a Unity player: honors -logFile, logs an exception, then dies with code 5
    $log = $null
    for ($i = 0; $i -lt $Rest.Count; $i++) { if ($Rest[$i] -eq "-logFile") { $log = $Rest[$i + 1] } }
    if ($log) { "Initialize engine version: 6000.6.2f1`nNullReferenceException: Object reference not set to an instance of an object`n  at Enemy.Update ()" | Set-Content $log }
    $t = New-Object System.Windows.Forms.Timer; $t.Interval = 3000
    $t.Add_Tick({ [Environment]::Exit(5) }); $t.Start()
}
if ($Mode -eq "hang") {
    # a real UI hang: busy loop on the UI thread (Start-Sleep pumps COM/sent messages, so it isn't one)
    Add-Button "Freeze" 250 80 { $end = (Get-Date).AddSeconds(12); while ((Get-Date) -lt $end) { } }
}

[void]$form.ShowDialog()
