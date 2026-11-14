#Requires -Version 7
# The installer's whole life on a disposable Windows machine (a CI runner, or Windows Sandbox through sandbox.ps1):
# install, start, save a clip with the real hotkey, switch "Start with Windows" off, upgrade with the app running,
# second launch, uninstall while running. Every check is recorded in <Out>\lifecycle.json; the exit code is the
# number of failed checks. It installs per machine and changes HKCU: never run it on a computer you use.
# -Phase runs one part (CI runs each as its own step, so a runner that dies mid-test still shows which part killed
# it); the checks carry over in <Out>\state.json, and only the "report" phase (or "all") exits with the count.
param(
    [Parameter(Mandatory)][string]$Old,
    [Parameter(Mandatory)][string]$New,
    [string]$Out = (Join-Path ([IO.Path]::GetTempPath()) 'msi-lifecycle'),
    [ValidateSet('all', 'install', 'start', 'upgrade', 'relaunch', 'uninstall', 'report')][string]$Phase = 'all'
)
$ErrorActionPreference = 'Continue'
New-Item -ItemType Directory -Force $Out | Out-Null
$state = Join-Path $Out 'state.json'
$checks = [ordered]@{}
$clip = $null
if ($Phase -notin 'all', 'install' -and (Test-Path $state)) {
    $saved = Get-Content $state -Raw | ConvertFrom-Json -AsHashtable
    $checks = $saved.checks
    $clip = $saved.clip
}
function Runs($part) { $Phase -in 'all', $part }
$dir = "$env:ProgramFiles\Clip Recorder"
$exe = "$dir\clip-recorder.exe"
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$approved = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
$clips = "$env:USERPROFILE\Videos\ClipRecorder"
Add-Type @"
using System; using System.Runtime.InteropServices; using System.Text;
public static class K {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  public static void Press(params byte[] keys) {
    foreach (var k in keys) keybd_event(k, 0, 0, UIntPtr.Zero);
    Array.Reverse(keys); foreach (var k in keys) keybd_event(k, 0, 2, UIntPtr.Zero);
  }
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public static bool Visible(string title) {
    bool found = false;
    EnumWindows((h, l) => { var s = new StringBuilder(128); GetWindowText(h, s, 128);
      if (s.ToString() == title && IsWindowVisible(h)) { found = true; return false; } return true; }, IntPtr.Zero);
    return found;
  }
}
"@

# Each check is printed as it happens: a step that hangs then shows where.
function Check($name, $ok, $detail = '') {
    $checks[$name] = [ordered]@{ ok = [bool]$ok; detail = "$detail" }
    '{0}  {1}  {2}' -f ($ok ? 'ok  ' : 'FAIL'), $name, $detail | Write-Host
}
# A check whose expression throws (indexing a missing value...) would be skipped and never counted: it fails instead.
trap { Check "threw: $($_.InvocationInfo.Line.Trim())" $false $_; continue }
function Step($what) { "[$(Get-Date -Format HH:mm:ss)] $what" | Write-Host }
function Msi($verb, $msi, $log) {
    # Wait on msiexec itself only: -Wait would also wait for an app Restart Manager relaunches.
    # /norestart: under /qn a file in use may reboot the machine. Without the reboot it exits 3010, which the exit-0
    # checks report.
    Step "msiexec $verb $(Split-Path $msi -Leaf)"
    $p = Start-Process msiexec -ArgumentList $verb, "`"$msi`"", '/qn', '/norestart', '/l*v', "`"$Out\$log`"" -PassThru
    if (-not $p.WaitForExit(300000)) {
        $p.Kill($true)
        return 'timed out after 5 min'
    }
    $p.ExitCode
}
function Count($name) { @(Get-Process $name -ErrorAction SilentlyContinue).Count }
# Not Start-Process: under PowerShell 7 in Windows Sandbox it started nothing and returned no process.
function Launch { [Diagnostics.Process]::Start([Diagnostics.ProcessStartInfo]@{ FileName = $exe; UseShellExecute = $false }) }
function RunValue { (Get-ItemProperty $run -ErrorAction SilentlyContinue).'Clip Recorder' }

if (Runs install) {
$code = Msi '/i' $Old 'install.log'
Check 'install exits 0' ($code -eq 0) $code
Check 'app and ffmpeg installed' ((Test-Path $exe) -and (Test-Path "$dir\ffmpeg.exe"))
Check 'starts with Windows (HKCU Run)' ((RunValue) -eq "`"$exe`"") (RunValue)

}

if (Runs start) {
Step 'launch the app'
$app = Launch
Start-Sleep -Seconds 12
Check 'app running' ((Count clip-recorder) -eq 1) ($app.HasExited ? ('exited with 0x{0:X8}' -f $app.ExitCode) : '')

Step 'press the hotkey'
# A clip through the real hotkey: whatever the machine's GPU, the probed pipeline must record.
[K]::Press(0x11, 0x12, 0x52)
foreach ($i in 1..30) {
    Start-Sleep -Seconds 1
    $found = Get-ChildItem $clips -Filter *.mp4 -ErrorAction SilentlyContinue | Where-Object Length -gt 0 | Select-Object -First 1
    if ($found) { $clip = $found.FullName; break }
}
Check 'hotkey saves a clip' ($null -ne $clip) ($found ? "$($found.Name) $($found.Length)" : 'none')

# The user switches startup off, as Settings does.
New-Item $approved -Force | Out-Null
Set-ItemProperty $approved 'Clip Recorder' ([byte[]](3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)) -Type Binary

}

if (Runs upgrade) {
$code = Msi '/i' $New 'upgrade.log'
Check 'upgrade with the app running exits 0 (no reboot)' ($code -eq 0) $code
Start-Sleep -Seconds 10
Check 'never two instances after the upgrade' ((Count clip-recorder) -le 1) (Count clip-recorder)
Check 'Run value kept by the upgrade' ((RunValue) -eq "`"$exe`"") (RunValue)
Check 'start-up "off" survives the upgrade' (((Get-ItemProperty $approved -ErrorAction SilentlyContinue).'Clip Recorder')[0] -eq 3)

}

if (Runs relaunch) {
if ((Count clip-recorder) -eq 0) { $null = Launch; Start-Sleep -Seconds 8 }
Step 'second launch'
$null = Launch
$settings = $false
foreach ($i in 1..10) { Start-Sleep -Seconds 1; if ([K]::Visible('Clip Recorder Settings')) { $settings = $true; break } }
Check 'second launch opens Settings' $settings
Check 'second launch leaves one instance' ((Count clip-recorder) -eq 1) (Count clip-recorder)

}

if (Runs uninstall) {
$code = Msi '/x' $New 'uninstall.log'
Check 'uninstall while running exits 0' ($code -eq 0) $code
Start-Sleep -Seconds 3
Check 'nothing left running' (((Count clip-recorder) + (Count ffmpeg)) -eq 0)
Check 'Program Files folder removed' (-not (Test-Path $dir))
Check 'Run value removed' ($null -eq (RunValue))
Check 'clips kept' ($clip -and (Test-Path $clip))
}

@{ checks = $checks; clip = $clip } | ConvertTo-Json -Depth 4 | Set-Content $state -Encoding utf8
if (-not (Runs report)) { exit 0 }
$failed = @($checks.Keys | Where-Object { -not $checks[$_].ok })
[ordered]@{
    checks = $checks
    failed = $failed
    log    = @(Get-Content "$env:APPDATA\ClipRecorder\clip_recorder.log" -ErrorAction SilentlyContinue)
} | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $Out 'lifecycle.json') -Encoding utf8
"$($failed.Count) failed: $($failed -join '; ')" | Write-Host
exit $failed.Count
