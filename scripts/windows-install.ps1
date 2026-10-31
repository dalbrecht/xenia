# Shared installer operations. Keep compatible with Windows PowerShell 5.1.
function Assert-XeniaWorkerStopped {
    $running = @(Get-Process -ErrorAction Stop | Where-Object { $_.ProcessName -in @('XeniaWorker', 'xenia-engine') })
    if ($running.Count) { throw 'Stop active work and quit Xenia Worker before installing or updating.' }
}
function Write-XeniaWorkerShortcut([string]$Path, [string]$Destination) {
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($Path)
    $shortcut.TargetPath = Join-Path $Destination 'XeniaWorker.exe'
    $shortcut.WorkingDirectory = $Destination
    $shortcut.Save()
}
function Install-XeniaWorkerPackage([string]$Source, [string]$Destination, [string]$ShortcutPath, [string]$LockPath) {
    $ErrorActionPreference = 'Stop'
    $sourceRoot = (Resolve-Path -LiteralPath $Source).ProviderPath.TrimEnd([char[]]'\/')
    foreach ($name in @('XeniaWorker.exe', 'XeniaWorker.dll', 'XeniaWorker.runtimeconfig.json', 'xenia-engine.exe', 'LICENSE')) {
        if (-not (Test-Path -LiteralPath (Join-Path $sourceRoot $name) -PathType Leaf)) { throw "Incomplete package: missing $name. Build the Windows worker first." }
    }
    $entries = @(Get-ChildItem -LiteralPath $sourceRoot -Recurse -Force)
    if (@($entries | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }).Count) { throw 'Package contains links or reparse points; refusing installation.' }
    $files = @($entries | Where-Object { -not $_.PSIsContainer })
    $suffix = [Guid]::NewGuid().ToString('N')
    $parent = Split-Path -Parent $Destination
    $stage = Join-Path $parent ".xenia-stage-$suffix"
    $backup = Join-Path $parent ".xenia-previous-$suffix"
    $shortcutStage = Join-Path (Split-Path -Parent $ShortcutPath) ".xenia-stage-$suffix.lnk"
    $shortcutBackup = Join-Path (Split-Path -Parent $ShortcutPath) ".xenia-previous-$suffix.lnk"
    $oldMoved = $false; $published = $false; $oldShortcutMoved = $false; $shortcutPublished = $false
    New-Item -ItemType Directory -Force (Split-Path -Parent $LockPath) | Out-Null
    try { $lock = [IO.File]::Open($LockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None) }
    catch { throw 'Xenia Worker or another installer is open. Stop active work and quit the app before updating.' }
    try {
        Assert-XeniaWorkerStopped
        New-Item -ItemType Directory -Force $parent | Out-Null
        New-Item -ItemType Directory $stage | Out-Null
        Get-ChildItem -LiteralPath $sourceRoot -Force | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $stage -Recurse -Force }
        foreach ($file in $files) {
            $relative = $file.FullName.Substring($sourceRoot.Length + 1)
            $copied = Join-Path $stage $relative
            if ((Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath $copied -Algorithm SHA256).Hash) { throw "Staged file verification failed: $relative" }
        }
        New-Item -ItemType Directory -Force (Split-Path -Parent $ShortcutPath) | Out-Null
        Write-XeniaWorkerShortcut $shortcutStage $Destination
        if (-not (Test-Path -LiteralPath $shortcutStage -PathType Leaf)) { throw 'Could not stage Start-menu shortcut.' }
        Assert-XeniaWorkerStopped
        if (Test-Path -LiteralPath $Destination) { Move-Item -LiteralPath $Destination -Destination $backup; $oldMoved = $true }
        Move-Item -LiteralPath $stage -Destination $Destination; $published = $true
        if (Test-Path -LiteralPath $ShortcutPath) { Move-Item -LiteralPath $ShortcutPath -Destination $shortcutBackup; $oldShortcutMoved = $true }
        Move-Item -LiteralPath $shortcutStage -Destination $ShortcutPath; $shortcutPublished = $true
        Write-Host "Installed Xenia Worker in $Destination"
        if ($oldMoved) { Write-Host "Previous application retained in $backup" }
        if ($oldShortcutMoved) { Write-Host "Previous shortcut retained in $shortcutBackup" }
    } catch {
        $originalError = $_
        $recoveryErrors = @()
        try {
            if ($shortcutPublished) { Remove-Item -LiteralPath $ShortcutPath -Force }
            if ($oldShortcutMoved) { Move-Item -LiteralPath $shortcutBackup -Destination $ShortcutPath }
        } catch { $recoveryErrors += "Shortcut recovery failed; inspect $shortcutBackup" }
        try {
            if ($published) { Remove-Item -LiteralPath $Destination -Recurse -Force }
            if ($oldMoved) { Move-Item -LiteralPath $backup -Destination $Destination }
        } catch { $recoveryErrors += "Application recovery failed; previous files remain in $backup" }
        if ($recoveryErrors.Count) { throw ($originalError.ToString() + '. ' + ($recoveryErrors -join '. ')) }
        throw $originalError
    } finally {
        # Never remove a retained previous installation on an error path.
        if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue }
        if (Test-Path -LiteralPath $shortcutStage) { Remove-Item -LiteralPath $shortcutStage -Force -ErrorAction SilentlyContinue }
        $lock.Dispose()
    }
}
