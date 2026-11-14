#Requires -Version 7
# Before tagging a release: measures A/V sync on the release build at 60, 120 and 240 fps, with a real screen and real
# sound (what CI cannot do: its runners have no audio device), and records the verdict on the commit as the status
# `release/av-sync`, without which .github/workflows/release.yml refuses to build.
# At the computer: the second screen on, nothing else playing sound, no other Clip Recorder running. About 3 minutes.
param([int]$Monitor = 1)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

if (git status --porcelain) { throw 'The working tree is not clean.' }
git fetch -q origin
$sha = git rev-parse HEAD
if ($sha -ne (git rev-parse origin/master)) { throw 'Run it on master, up to date with origin/master: releases are tagged there.' }
$others = Get-Process ClipRecorder, clip-recorder -ErrorAction SilentlyContinue
if ($others) {
    throw "Close Clip Recorder first ($(@($others.Name | Sort-Object -Unique) -join ', ')): a second capture of the screen skews the result by about 50 ms."
}
py -I -c 'import pyaudiowpatch' 2>$null
if ($LASTEXITCODE) { throw 'The harness needs pyaudiowpatch: py -m pip install -r tests/requirements.txt' }

./build-msi.ps1 -FFmpegOnly
cargo build --release --locked
if ($LASTEXITCODE) { exit $LASTEXITCODE }

# Thresholds: the measured release build sat at median 0 ms (60/120/240 fps), with a test resolution of 10 to 20 ms.
$limits = @{ median = 30; max = 60; durationDiff = 50; lost = 1.5 }
$failures = @()
$summary = @()
foreach ($fps in 60, 120, 240) {
    $json = Join-Path ([IO.Path]::GetTempPath()) "release-check-$fps.json"
    Remove-Item $json -ErrorAction SilentlyContinue
    py tests/timecode_sync_test.py --generate --fps $fps --buffer 15 --monitor $Monitor --json $json
    $r = Test-Path $json ? (Get-Content $json -Raw | ConvertFrom-Json) : $null
    $problems = @()
    if (-not $r) { $problems += 'no result' } else {
        if ($r.rc -ne 0) { $problems += "harness rc $($r.rc)" }
        if (-not $r.decode_clean) { $problems += 'decode errors' }
        if ($r.discontinuities -ne 0) { $problems += "$($r.discontinuities) discontinuities" }
        if ($r.beeps_missing -or $r.unexpected_sound) { $problems += 'sound where silence was scheduled, or the reverse' }
        if ([math]::Abs($r.median_offset_ms) -gt $limits.median) { $problems += "median $($r.median_offset_ms) ms" }
        if ($r.max_abs_offset_ms -gt $limits.max) { $problems += "max $($r.max_abs_offset_ms) ms" }
        if ($r.duration_diff_ms -gt $limits.durationDiff) { $problems += "audio/video lengths differ by $($r.duration_diff_ms) ms" }
        if ($null -ne $r.lost_before_press_s -and $r.lost_before_press_s -gt $limits.lost) { $problems += "lost $($r.lost_before_press_s) s before the press" }
        $summary += "${fps}fps {0:+0;-0;0}ms" -f $r.median_offset_ms
    }
    if ($problems) { $failures += "$fps fps: $($problems -join ', ')" }
}

$state = $failures ? 'failure' : 'success'
$description = $failures ? ($failures -join '; ') : "A/V sync median $($summary -join ', ')"
if ($description.Length -gt 140) { $description = $description.Substring(0, 137) + '...' }
$repo = gh repo view --json nameWithOwner --jq .nameWithOwner
gh api "repos/$repo/statuses/$sha" -f state=$state -f context=release/av-sync -f description=$description | Out-Null
"$state on $($sha.Substring(0, 7)): $description"
if ($failures) { exit 1 }
