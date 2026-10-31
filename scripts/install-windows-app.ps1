$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows-install.ps1')
Install-XeniaWorkerPackage `
    -Source (Join-Path $PSScriptRoot '..\dist\windows-worker') `
    -Destination (Join-Path $env:LOCALAPPDATA 'Programs\XeniaWorker') `
    -ShortcutPath (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Xenia Worker.lnk') `
    -LockPath (Join-Path $env:LOCALAPPDATA 'XeniaWorker\desktop.lock')
