$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

# Cargo.toml is the single source of the version.
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value

# FFmpeg ships in the MSI, so it is pinned: this exact build, checked by hash before it is used.
# Bump the URL and the hash together, after checking the zip against GitHub's published digest.
$ffmpegUrl = 'https://github.com/GyanD/codexffmpeg/releases/download/8.1/ffmpeg-8.1-full_build.zip'
$ffmpegSha256 = 'D1E2A156261ECC675081943197A85F08F2868784A0AF499171EDE89353EDAD31'
$ffmpeg = Join-Path $PSScriptRoot 'target\ffmpeg\ffmpeg.exe'

function Test-Pinned($path) { (Test-Path $path) -and (Get-FileHash $path -Algorithm SHA256).Hash -eq $ffmpegSha256 }

if (-not (Test-Pinned $ffmpeg)) {
    New-Item -ItemType Directory -Force (Split-Path $ffmpeg) | Out-Null
    # An unverified binary must never stay where the next build could bundle it.
    Remove-Item $ffmpeg -Force -ErrorAction SilentlyContinue
    $zip = Join-Path ([IO.Path]::GetTempPath()) "ffmpeg-8.1-full_build-$PID.zip"
    try {
        Write-Host "Downloading $ffmpegUrl (~250 MB)..."
        Invoke-WebRequest $ffmpegUrl -OutFile $zip -UseBasicParsing
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $archive = [IO.Compression.ZipFile]::OpenRead($zip)
        try {
            $entry = $archive.Entries | Where-Object { $_.Name -eq 'ffmpeg.exe' } | Select-Object -First 1
            if (-not $entry) { throw 'No ffmpeg.exe in the downloaded archive.' }
            $part = "$ffmpeg.part"
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $part, $true)
        } finally { $archive.Dispose() }
        if (-not (Test-Pinned $part)) { Remove-Item $part -Force; throw 'The downloaded ffmpeg.exe does not match the pinned SHA-256. Refusing to use it.' }
        Move-Item $part $ffmpeg -Force
    } finally { Remove-Item $zip -Force -ErrorAction SilentlyContinue }
}

cargo build --release --locked
if ($LASTEXITCODE) { exit $LASTEXITCODE }
wix build installer\main.wxs -arch x64 -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext `
    -d Version=$version -d FFmpeg=$ffmpeg -o target\clip-recorder-$version-x64.msi
exit $LASTEXITCODE
