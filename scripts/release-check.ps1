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

$failures = @()
$summary = @()
foreach ($fps in 60, 120, 240) {
    $json = Join-Path ([IO.Path]::GetTempPath()) "release-check-$fps.json"
    Remove-Item $json -ErrorAction SilentlyContinue
    py tests/timecode_sync_test.py --generate --fps $fps --buffer 15 --monitor $Monitor --json $json
    $verdict = ./scripts/av-verdict.ps1 -Path $json
    if ($null -ne $verdict.Median) { $summary += "${fps}fps {0:+0;-0;0}ms" -f $verdict.Median }
    if ($verdict.Problems) { $failures += "$fps fps: $($verdict.Problems -join ', ')" }
}

$state = $failures ? 'failure' : 'success'
$description = $failures ? ($failures -join '; ') : "A/V sync median $($summary -join ', ')"
if ($description.Length -gt 140) { $description = $description.Substring(0, 137) + '...' }
$repo = gh repo view --json nameWithOwner --jq .nameWithOwner
gh api "repos/$repo/statuses/$sha" -f state=$state -f context=release/av-sync -f description=$description | Out-Null
"$state on $($sha.Substring(0, 7)): $description"
if ($failures) { exit 1 }
