# Disposable filesystem tests. COM is mocked; native Windows also checks real .lnk creation.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '../scripts/windows-install.ps1')
$root = Join-Path ([IO.Path]::GetTempPath()) ('xenia-install-test-' + [Guid]::NewGuid().ToString('N'))
$source = Join-Path $root 'package'; $destination = Join-Path $root 'Programs/XeniaWorker'
$shortcut = Join-Path $root 'Start Menu/Xenia Worker.lnk'; $lockPath = Join-Path $root 'data/desktop.lock'
$script:scenario = ''; $script:checks = 0
function Assert-True($Condition, $Message) { if (-not $Condition) { throw $Message } }
$realGuard = ${function:Assert-XeniaWorkerStopped}
$realShortcut = ${function:Write-XeniaWorkerShortcut}
function Assert-XeniaWorkerStopped {
    $script:checks++
    if ($script:scenario -eq 'running' -or ($script:scenario -eq 'late-start' -and $script:checks -gt 1)) { throw 'Synthetic running worker' }
    if ($script:scenario -eq 'process-error') { throw 'Synthetic process enumeration failure' }
}
function Write-XeniaWorkerShortcut([string]$Path, [string]$Destination) {
    if ($script:scenario -eq 'shortcut-error') { throw 'Synthetic shortcut failure' }
    Set-Content -LiteralPath $Path -Value ('new shortcut ' + $Destination)

}
function Copy-Item {
    [CmdletBinding()] param([string]$LiteralPath, [string]$Destination, [switch]$Recurse, [switch]$Force)
    if ($script:scenario -eq 'copy-error' -and (Split-Path -Leaf $Destination) -like '.xenia-stage-*') { throw 'Synthetic copy failure' }
    Microsoft.PowerShell.Management\Copy-Item -LiteralPath $LiteralPath -Destination $Destination -Recurse:$Recurse -Force:$Force
}
function Get-FileHash {
    [CmdletBinding()] param([string]$LiteralPath, [string]$Algorithm)
    $hash = Microsoft.PowerShell.Utility\Get-FileHash -LiteralPath $LiteralPath -Algorithm $Algorithm
    if ($script:scenario -eq 'verify-error' -and $LiteralPath -like '*.xenia-stage-*') { $hash.Hash = 'synthetic mismatch' }
    return $hash
}
function Move-Item {
    [CmdletBinding()] param([string]$LiteralPath, [string]$Destination)
    if (($script:scenario -in @('publish-error','restore-error')) -and (Split-Path -Leaf $LiteralPath) -like '.xenia-stage-*' -and $Destination -eq $script:destination) { throw 'Synthetic publish failure' }
    if ($script:scenario -eq 'shortcut-publish-error' -and $Destination -eq $script:shortcut) { throw 'Synthetic shortcut publish failure' }
    if ($script:scenario -eq 'restore-error' -and (Split-Path -Leaf $LiteralPath) -like '.xenia-previous-*') { throw 'Synthetic restore failure' }
    Microsoft.PowerShell.Management\Move-Item -LiteralPath $LiteralPath -Destination $Destination
}
function Install-Test {
    Install-XeniaWorkerPackage -Source $source -Destination $destination -ShortcutPath $shortcut -LockPath $lockPath
}
try {
    New-Item -ItemType Directory -Force $source,$destination,(Split-Path -Parent $shortcut),(Split-Path -Parent $lockPath) | Out-Null
    foreach ($name in @('XeniaWorker.exe','XeniaWorker.dll','XeniaWorker.runtimeconfig.json','xenia-engine.exe','LICENSE')) { Set-Content (Join-Path $source $name) ('synthetic ' + $name) }
    $nested = Join-Path $source 'runtimes/win-x64/native'
    New-Item -ItemType Directory -Force $nested | Out-Null
    Set-Content (Join-Path $nested 'support library.dll') 'synthetic nested runtime'
    Set-Content (Join-Path $destination 'previous') 'old application'
    Set-Content $shortcut 'old shortcut'
    foreach ($case in @('running','late-start','process-error','copy-error','verify-error','shortcut-error','publish-error')) {
        $script:scenario = $case; $script:checks = 0; $failed = $false
        try { Install-Test } catch { $failed = $true }
        Assert-True $failed "Expected $case to fail"
        Assert-True ((Get-Content (Join-Path $destination 'previous')) -eq 'old application') "$case destroyed the previous application"
        Assert-True ((Get-Content $shortcut) -eq 'old shortcut') "$case changed the previous shortcut"
        Assert-True (@(Get-ChildItem -Force (Split-Path -Parent $destination) -Filter '.xenia-stage-*').Count -eq 0) "$case left staging files"
    }
    # Simulate a failure after the application directory has been published.
    $script:scenario = 'shortcut-publish-error'; $script:checks = 0; $failed = $false
    try { Install-Test } catch { $failed = $true }
    Assert-True $failed 'Expected shortcut publication to fail'
    Assert-True ((Get-Content (Join-Path $destination 'previous')) -eq 'old application') 'Shortcut failure did not restore the application'
    # The same injected failure blocks shortcut restoration; its backup must survive.
    $shortcutBackups = @(Get-ChildItem -Force (Split-Path -Parent $shortcut) -Filter '.xenia-previous-*')
    Assert-True ($shortcutBackups.Count -eq 1) 'Shortcut rollback backup was lost'
    Copy-Item -LiteralPath $shortcutBackups[0].FullName -Destination $shortcut
    Remove-Item -LiteralPath $shortcutBackups[0].FullName -Force
    # Exclusive lock is shared with the WPF app and held through publication.
    $script:scenario = ''; $handle = [IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None'); $failed = $false
    try { try { Install-Test } catch { $failed = $true } } finally { $handle.Dispose() }
    Assert-True $failed 'Installer ignored the desktop lock'
    Assert-True (Test-Path (Join-Path $destination 'previous')) 'Locked installation changed files'
    # Missing engine must be rejected before changing an existing installation.
    Remove-Item (Join-Path $source 'xenia-engine.exe'); $failed = $false
    try { Install-Test } catch { $failed = $true }
    Assert-True $failed 'Incomplete package was accepted'
    Set-Content (Join-Path $source 'xenia-engine.exe') 'synthetic engine'
    $script:scenario = 'restore-error'; $script:checks = 0; $message = ''
    try { Install-Test } catch { $message = $_.ToString() }
    Assert-True ($message -like '*previous files remain*') 'Failed rollback did not report recovery location'
    $backups = @(Get-ChildItem -Force (Split-Path -Parent $destination) -Directory -Filter '.xenia-previous-*')
    Assert-True ($backups.Count -eq 1 -and (Test-Path (Join-Path $backups[0].FullName 'previous'))) 'Failed rollback lost previous files'
    Microsoft.PowerShell.Management\Move-Item -LiteralPath $backups[0].FullName -Destination $destination
    $script:scenario = ''; $script:checks = 0
    Install-Test
    Assert-True (Test-Path (Join-Path $destination 'xenia-engine.exe')) 'Successful install missing engine'
    Assert-True ((Get-Content (Join-Path $destination 'runtimes/win-x64/native/support library.dll')) -eq 'synthetic nested runtime') 'Nested runtime not copied'
    Assert-True (-not (Test-Path (Join-Path $destination 'previous'))) 'Upgrade retained stale files in the app directory'
    Assert-True ((Get-Content $shortcut) -like 'new shortcut *') 'Shortcut not updated'
    Assert-True (@(Get-ChildItem -Force (Split-Path -Parent $destination) -Directory -Filter '.xenia-previous-*').Count -eq 1) 'Previous version not retained after success'
    # First installation must also work.
    Install-XeniaWorkerPackage $source (Join-Path $root 'First/app') (Join-Path $root 'First/menu/app.lnk') $lockPath
    function Get-Process {
        [CmdletBinding()] param()
        if ($script:scenario -eq 'enumeration-error') { throw 'Synthetic enumeration failure' }
        if ($script:scenario -eq 'legacy-worker') { return [PSCustomObject]@{ProcessName='xenia-engine'} }
        return [PSCustomObject]@{ProcessName='unrelated'}
    }
    foreach ($guardCase in @('legacy-worker','enumeration-error')) {
        $script:scenario = $guardCase; $failed = $false
        try { & $realGuard } catch { $failed = $true }
        Assert-True $failed "Real process guard ignored $guardCase"
    }
    $script:scenario = ''; & $realGuard
    if ($env:OS -eq 'Windows_NT') {
        $realPath = Join-Path $root 'native.lnk'
        & $realShortcut $realPath $destination
        $shell = New-Object -ComObject WScript.Shell
        $native = $shell.CreateShortcut($realPath)
        Assert-True ($native.TargetPath -eq (Join-Path $destination 'XeniaWorker.exe')) 'Native shortcut target is incorrect'
    }
    Write-Host 'Windows installer staging, running-app and lock guards, incomplete packages, publication failures, rollback and retained backups: PASS'
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force
}
