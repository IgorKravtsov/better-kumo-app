#requires -Version 7
param([Parameter(Mandatory)][string]$Setup)
$ErrorActionPreference = 'Stop'

$app = Join-Path $env:LOCALAPPDATA 'Programs\Better Kumo'
$exe = Join-Path $app 'Better Kumo.exe'
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

$log = Join-Path ($env:RUNNER_TEMP ?? $env:TEMP) 'setup.log'

function Install {
    # Not -Wait: that also waits for the Better Kumo the update starts again.
    $p = Start-Process $Setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/TASKS=startup', "/LOG=`"$log`"" -PassThru
    $null = $p.Handle
    $p.WaitForExit()
    if ($p.ExitCode -ne 0) { Get-Content $log; throw "setup exited with $($p.ExitCode)" }
}
function Wait-Until([scriptblock]$Condition, [string]$What) {
    for ($i = 0; $i -lt 60; $i++) { if (& $Condition) { return }; Start-Sleep -Milliseconds 500 }
    throw "timed out: $What"
}

Install
foreach ($f in 'Better Kumo.exe', 'kumo-mcp.exe', 'unins000.exe') {
    if (-not (Test-Path (Join-Path $app $f))) { throw "missing $f" }
}
$value = (Get-ItemProperty $run).'better-kumo'
if ($value -ne "`"$exe`" ") { throw "unexpected Run value: [$value]" }
if (-not (Test-Path (Join-Path ([Environment]::GetFolderPath('Programs')) 'Better Kumo.lnk'))) { throw 'no Start menu shortcut' }
if (Select-String -Path $exe, (Join-Path $app 'kumo-mcp.exe') -Pattern 'VCRUNTIME140' -SimpleMatch -Quiet) { throw 'linked to the VC++ runtime DLL' }
& (Join-Path $app 'kumo-mcp.exe') --version
if ($LASTEXITCODE -ne 0) { throw "kumo-mcp --version exited with $LASTEXITCODE" }
Write-Host "Installed to $app, login entry: $value"

Start-Process $exe
Start-Sleep -Seconds 5
if (-not (Get-Process 'Better Kumo' -ErrorAction SilentlyContinue)) { throw 'Better Kumo did not keep running' }

# Turned off in the app's menu: an update must not turn it back on.
Remove-ItemProperty $run -Name 'better-kumo'
Install
Wait-Until { Get-Process 'Better Kumo' -ErrorAction SilentlyContinue } 'the update starts the app again'
Start-Sleep -Seconds 5
if (-not (Get-Process 'Better Kumo' -ErrorAction SilentlyContinue)) { throw 'Better Kumo did not keep running after the update' }
if ($null -ne (Get-ItemProperty $run).'better-kumo') { throw 'the update turned Start at login back on' }
Write-Host 'Updated over the running app'

Set-ItemProperty $run -Name 'better-kumo' -Value "`"$exe`" "

New-Item -ItemType Directory (Join-Path $app '.better-kumo-update-smoke\old') | Out-Null
$p = Start-Process (Join-Path $app 'unins000.exe') -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "uninstaller exited with $($p.ExitCode)" }
Wait-Until { -not (Test-Path $app) } 'the uninstaller removes the app folder'
if ($null -ne (Get-ItemProperty $run).'better-kumo') { throw 'login entry left behind' }
Write-Host 'Uninstalled cleanly'
