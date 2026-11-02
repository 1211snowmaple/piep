# 署名済み ARM64 APK と更新の案内を作り、中身を検査する。
#
# 手元でも CI でも同じ手順で動かす。公開は -Publish を付けたときだけ行う。
# 署名鍵は src-tauri/gen/android/keystore.properties から読む。Git には入れない。
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$OutputDir = 'release-output',
    [switch]$Publish
)
$ErrorActionPreference = 'Stop'
$repository = '1211snowmaple/piep-android'
$applicationId = 'io.github.u1211snowmaple.piep.android'

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw '版は X.Y.Z の形にしてください' }

# 道具を動かす前に版を照合する。
$package = (Get-Content -LiteralPath package.json -Raw | ConvertFrom-Json).version
$tauri = (Get-Content -LiteralPath src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
$cargo = [regex]::Match((Get-Content -LiteralPath src-tauri/Cargo.toml -Raw), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if ($package -ne $Version -or $tauri -ne $Version -or $cargo -ne $Version) {
    throw "版がそろっていません: package.json $package / tauri.conf.json $tauri / Cargo.toml $cargo"
}
if (-not (Test-Path -LiteralPath src-tauri/gen/android/keystore.properties)) {
    throw 'keystore.properties がありません。RELEASING.md を見てください'
}
if ($env:PIEP_ANDROID_TEST_UPDATE_SOURCE) { throw '試験用の更新先が設定されたまま配布物を作ろうとしています' }

$env:RUSTFLAGS = "--remap-path-prefix=$env:USERPROFILE=/build"
npm ci
if ($LASTEXITCODE -ne 0) { throw 'npm ci に失敗しました' }
npx tauri android build --apk --target aarch64 --ci
if ($LASTEXITCODE -ne 0) { throw 'APK を組めませんでした' }

$built = 'src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk'
if (-not (Test-Path -LiteralPath $built)) { throw "$built がありません" }

$sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } else { Join-Path $env:LOCALAPPDATA 'Android/Sdk' }
$buildTools = Get-ChildItem (Join-Path $sdk 'build-tools') | Sort-Object { [version]($_.Name -replace '[^\d.].*$', '') } | Select-Object -Last 1
$aapt = Join-Path $buildTools.FullName 'aapt.exe'
$apksigner = Join-Path $buildTools.FullName 'apksigner.bat'

$badging = (& $aapt dump badging $built | Select-Object -First 1)
$match = [regex]::Match($badging, "name='([^']+)' versionCode='(\d+)' versionName='([^']+)'")
if (-not $match.Success) { throw 'APK の情報を読み取れません' }
if ($match.Groups[1].Value -ne $applicationId) { throw "アプリ ID が違います: $($match.Groups[1].Value)" }
if ($match.Groups[3].Value -ne $Version) { throw "APK の版が違います: $($match.Groups[3].Value)" }
$versionCode = [long]$match.Groups[2].Value

& $apksigner verify $built
if ($LASTEXITCODE -ne 0) { throw 'APK の署名を検証できません' }
$certificate = & $apksigner verify --print-certs $built | Select-String 'SHA-256' | Select-Object -First 1

# ビルド端末のユーザー名やホームディレクトリを配布物に埋め込まない。
Add-Type -AssemblyName System.IO.Compression.FileSystem
$profilePath = Split-Path -NoQualifier $env:USERPROFILE
$needles = @($profilePath, ($profilePath -replace '\\', '/'))
$archive = [IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath $built).Path)
try {
    foreach ($entry in $archive.Entries) {
        $stream = $entry.Open()
        try {
            $buffer = New-Object IO.MemoryStream
            $stream.CopyTo($buffer)
        } finally { $stream.Dispose() }
        $text = [Text.Encoding]::Latin1.GetString($buffer.ToArray())
        foreach ($needle in $needles) {
            if ($text.Contains($needle, [StringComparison]::OrdinalIgnoreCase)) {
                throw "$($entry.FullName) にユーザープロファイルのパスが含まれています"
            }
        }
    }
} finally { $archive.Dispose() }

New-Item -ItemType Directory -Force $OutputDir | Out-Null
$name = "piep-android-v$Version-arm64.apk"
$apk = Join-Path $OutputDir $name
Copy-Item -LiteralPath $built -Destination $apk -Force
$manifest = Join-Path $OutputDir 'latest-android.json'
[ordered]@{
    version = $Version
    versionCode = $versionCode
    url = "https://github.com/$repository/releases/download/v$Version/$name"
    sha256 = (Get-FileHash -LiteralPath $apk -Algorithm SHA256).Hash.ToLowerInvariant()
    size = (Get-Item -LiteralPath $apk).Length
} | ConvertTo-Json | Set-Content -LiteralPath $manifest -Encoding utf8NoBOM

Write-Output "署名: $certificate"
Write-Output "APK: $apk"
Write-Output "案内: $manifest"

if ($Publish) {
    gh release create "v$Version" $apk $manifest --repo $repository --title "piep Android v$Version" --notes "ARM64 向けの APK です。すでに入れている場合は、アプリの「設定 → アプリの更新」から更新できます。"
    if ($LASTEXITCODE -ne 0) { throw '公開できませんでした' }
}
