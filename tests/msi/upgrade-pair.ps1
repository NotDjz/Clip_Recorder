#Requires -Version 7
# From the MSI build-msi.ps1 just made, an upgrade pair: old.msi is that MSI, new.msi a second WiX build of the same
# version (a new package code, so installing it over old.msi is a major upgrade, as a release replacing the last one).
param([Parameter(Mandatory)][string]$Out)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent | Split-Path -Parent)
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
New-Item -ItemType Directory -Force $Out | Out-Null
Copy-Item "target\clip-recorder-$version-x64.msi" (Join-Path $Out 'old.msi')
wix build installer\main.wxs -arch x64 -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext `
    -d Version=$version -d FFmpeg=target\ffmpeg\ffmpeg.exe -o (Join-Path $Out 'new.msi')
exit $LASTEXITCODE
