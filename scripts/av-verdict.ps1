#Requires -Version 7
# The verdict on one `timecode_sync_test.py --json` result, for scripts/release-check.ps1: { Problems; Median }, no
# problems when it passes. tests/source.rs runs it on fixture results.
param([Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'

# Thresholds: the measured release build sat at median 0 ms (60/120/240 fps), with a test resolution of 10 to 20 ms.
$limits = @{ median = 30; max = 60; durationDiff = 50; lost = 1.5 }
$r = (Test-Path $Path) ?(Get-Content $Path -Raw | ConvertFrom-Json) : $null
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
}
[pscustomobject]@{ Problems = $problems; Median = $r.median_offset_ms }
