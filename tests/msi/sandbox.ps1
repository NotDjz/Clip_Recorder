#Requires -Version 7
# Runs lifecycle.ps1 in Windows Sandbox (networking off, a fresh Windows each time), for a manual check on a computer
# that is used: the script itself installs per machine and changes HKCU, so it must never run on the host.
# The host's PowerShell 7 folder is mapped read-only (Sandbox only has Windows PowerShell 5).
param(
    [Parameter(Mandatory)][string]$Old,
    [Parameter(Mandatory)][string]$New,
    [int]$TimeoutMinutes = 15
)
$ErrorActionPreference = 'Stop'
$work = Join-Path ([IO.Path]::GetTempPath()) "msi-sandbox-$PID"
$in, $out = (Join-Path $work 'in'), (Join-Path $work 'out')
New-Item -ItemType Directory -Force $in, $out | Out-Null
Copy-Item $Old (Join-Path $in 'old.msi')
Copy-Item $New (Join-Path $in 'new.msi')
Copy-Item (Join-Path $PSScriptRoot 'lifecycle.ps1') $in
$pwshDir = Split-Path (Get-Command pwsh).Source
Set-Content (Join-Path $in 'run.cmd') @"
"C:\pwsh\pwsh.exe" -NoProfile -ExecutionPolicy Bypass -File C:\in\lifecycle.ps1 -Old C:\in\old.msi -New C:\in\new.msi -Out C:\out > C:\out\console.txt 2>&1
echo %ERRORLEVEL% > C:\out\exit.txt
"@ -Encoding ascii
$wsb = Join-Path $work 'lifecycle.wsb'
Set-Content $wsb @"
<Configuration>
  <Networking>Disable</Networking>
  <MappedFolders>
    <MappedFolder><HostFolder>$in</HostFolder><SandboxFolder>C:\in</SandboxFolder><ReadOnly>true</ReadOnly></MappedFolder>
    <MappedFolder><HostFolder>$pwshDir</HostFolder><SandboxFolder>C:\pwsh</SandboxFolder><ReadOnly>true</ReadOnly></MappedFolder>
    <MappedFolder><HostFolder>$out</HostFolder><SandboxFolder>C:\out</SandboxFolder><ReadOnly>false</ReadOnly></MappedFolder>
  </MappedFolders>
  <LogonCommand><Command>C:\in\run.cmd</Command></LogonCommand>
</Configuration>
"@
$running = { @((wsb list --raw | ConvertFrom-Json).WindowsSandboxEnvironments | ForEach-Object Id) }
$before = & $running
if ($before.Count) { throw 'A Windows Sandbox is already running: only one can run at a time. Close it first.' }
Start-Process $wsb
$exitFile = Join-Path $out 'exit.txt'
$deadline = (Get-Date).AddMinutes($TimeoutMinutes)
while (-not (Test-Path $exitFile) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 5 }
foreach ($id in & $running) { wsb stop --id $id | Out-Null }
if (-not (Test-Path $exitFile)) { throw "No result after $TimeoutMinutes minutes (see $out)" }
$result = Get-Content (Join-Path $out 'lifecycle.json') -Raw | ConvertFrom-Json
foreach ($p in $result.checks.PSObject.Properties) { '{0}  {1}  {2}' -f ($p.Value.ok ? 'ok  ' : 'FAIL'), $p.Name, $p.Value.detail }
"Results in $out"
exit [int](Get-Content $exitFile).Trim()
