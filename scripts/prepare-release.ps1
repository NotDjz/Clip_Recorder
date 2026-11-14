#Requires -Version 7
# Prepares release <version>: bumps Cargo.toml and Cargo.lock, prepends its CHANGELOG.md section (git-cliff, from the
# last release to HEAD), commits on release/v<version> and opens the pull request. After it is merged:
#   pwsh scripts/release-check.ps1   (the A/V sync measurement, at the computer)
#   git tag v<version> && git push origin v<version>
# -WhatIf prints the section and changes nothing.
[CmdletBinding(SupportsShouldProcess)]
param([Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

if (git tag --list "v$Version") { throw "v$Version already exists." }

# The newest release by version (not the nearest tag: v1.x, the Python app, are in master's history too). When it is
# not in master's history, as v2.0.1 on the rust branch, start where that branch was merged (1812dfa).
$from = git tag --list 'v[0-9]*' --sort=-v:refname | Select-Object -First 1
git merge-base --is-ancestor $from HEAD 2>$null
if ($LASTEXITCODE) { $from = '1812dfa' }
# No --tag: with it git-cliff widens the range to the last tag it can see. The section is named afterwards.
$section = (git-cliff "$from..HEAD" --strip all) -join "`n"
if ($LASTEXITCODE) { throw 'git-cliff failed (cargo install git-cliff --version 2.14.2 --locked).' }
$section = $section -replace '^## \[Unreleased\]', "## [$Version] - $(Get-Date -Format yyyy-MM-dd)"
if ($section -notmatch '(?m)^### ') { throw "No releasable commit since $from." }
if (-not $PSCmdlet.ShouldProcess("v$Version", 'bump the version, prepend the changelog, commit and open the PR')) {
    $section
    return
}
if (git status --porcelain) { throw 'The working tree is not clean.' }
git fetch -q origin
if ((git rev-parse HEAD) -ne (git rev-parse origin/master)) { throw 'Run it on master, up to date with origin/master.' }

# Under the header (the first "## " line starts the newest release).
$changelog = Get-Content CHANGELOG.md -Raw
$at = $changelog.IndexOf("`n## ")
if ($at -lt 0) { throw 'CHANGELOG.md has no "## " release heading to insert above.' }
# The first `version =` only: [package] comes first, and a dependency table may carry its own.
$toml = Get-Content Cargo.toml -Raw
if ($toml -notmatch '\A\[package\]') { throw 'Cargo.toml no longer starts with [package].' }
Set-Content Cargo.toml ([regex]::new('(?m)^version = "[^"]+"').Replace($toml, "version = `"$Version`"", 1)) -NoNewline
cargo update -p clip-recorder --offline
if ($LASTEXITCODE) { exit $LASTEXITCODE }
Set-Content CHANGELOG.md ($changelog.Substring(0, $at + 1) + $section.Trim() + "`n`n" + $changelog.Substring($at + 1)) -NoNewline

git switch -c "release/v$Version"
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "build: release $Version" -m "Version bump and its CHANGELOG.md section, generated from the commits since $from."
git push -u origin "release/v$Version"
gh pr create --base master --head "release/v$Version" --title "build: release $Version" --body $section
