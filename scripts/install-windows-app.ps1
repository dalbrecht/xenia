$ErrorActionPreference = 'Stop'
$Source = Join-Path $PSScriptRoot '..\dist\windows-worker'
if (-not (Test-Path (Join-Path $Source 'XeniaWorker.exe'))) { throw 'Build the Windows worker first.' }
$Destination = Join-Path $env:LOCALAPPDATA 'Programs\XeniaWorker'
New-Item -ItemType Directory -Force $Destination | Out-Null
Copy-Item (Join-Path $Source '*') $Destination -Recurse -Force
$ShortcutPath = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Xenia Worker.lnk'
$Shell = New-Object -ComObject WScript.Shell
$Shortcut = $Shell.CreateShortcut($ShortcutPath)
$Shortcut.TargetPath = Join-Path $Destination 'XeniaWorker.exe'
$Shortcut.WorkingDirectory = $Destination
$Shortcut.Save()
Write-Host "Installed Xenia Worker in $Destination"
