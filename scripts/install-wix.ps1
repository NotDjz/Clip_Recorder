#Requires -Version 7
# Installs WiX 7.0.1 and its UI and Util extensions from nuget.org, each package checked against the SHA-512 that
# nuget.org publishes for it (its registration catalog) BEFORE anything from it runs. `dotnet tool install` and
# `wix extension add` would fetch whatever the feed serves at that version, unchecked.
# Puts a `wix` command on PATH ($env:GITHUB_PATH in Actions) and fills WiX's extension cache.
param([string]$ToolDir = (Join-Path ([IO.Path]::GetTempPath()) 'wix-7.0.1'))
$ErrorActionPreference = 'Stop'

$packages = [ordered]@{
    'wix'                    = 'nHKhfUNeDGQvK6bXhCSklSb810PMLQS9HgU9Hsri3RLWP6piAceVd0kON3Vmd/Wo199/VJjmS/mWAY6+F7N9cA=='
    'WixToolset.UI.wixext'   = 'trPpDmpbLB/IUmpt76tpsgB8+YTpmvAJx0ihxy9HU5192fhTxXrFOE1rnC3qqd/+L+N9DjUdf1deW+Ao3pEdBw=='
    'WixToolset.Util.wixext' = 'iIakOhTWOsKkiQUWLRyVEh/RpBJaIWjdmOKyAMdhT01vrZaEWojU4etk259XH5M5C19nvTQSmEwUl9TZWJgQPQ=='
}
$version = '7.0.1'

New-Item -ItemType Directory -Force $ToolDir | Out-Null
foreach ($id in $packages.Keys) {
    $lower = $id.ToLowerInvariant()
    $nupkg = Join-Path $ToolDir "$lower.$version.nupkg"
    Invoke-WebRequest "https://api.nuget.org/v3-flatcontainer/$lower/$version/$lower.$version.nupkg" -OutFile $nupkg -UseBasicParsing
    $actual = [Convert]::ToBase64String([Security.Cryptography.SHA512]::HashData([IO.File]::ReadAllBytes($nupkg)))
    if ($actual -ne $packages[$id]) {
        Remove-Item $nupkg -Force
        throw "$id $version does not match nuget.org's published SHA-512. Refusing to use it."
    }
    $target = if ($id -eq 'wix') { Join-Path $ToolDir 'wix' } else {
        Join-Path $env:USERPROFILE ".wix\extensions\$id\$version"
    }
    if (Test-Path $target) { Remove-Item $target -Recurse -Force }
    Expand-Archive $nupkg -DestinationPath $target
}

# A `wix` command running the verified tool.
$dll = Join-Path $ToolDir 'wix\tools\net8.0\any\wix.dll'
Set-Content (Join-Path $ToolDir 'wix.cmd') "@dotnet `"$dll`" %*" -Encoding ascii
if ($env:GITHUB_PATH) { $ToolDir >> $env:GITHUB_PATH }
$env:PATH = "$ToolDir;$env:PATH"
# WiX 7 needs its Open Source Maintenance Fee EULA accepted once per machine.
& (Join-Path $ToolDir 'wix.cmd') eula accept wix7
if ($LASTEXITCODE) { exit $LASTEXITCODE }
& (Join-Path $ToolDir 'wix.cmd') --version
