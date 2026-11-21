# リリースで Android の APK を組み直すか、前の APK をそのまま載せ直すかを決める。
#
# 版の番号はデスクトップ版と同じ一本の流れに乗せるが、携帯に関わるものが変わって
# いないリリースでは携帯の版を上げない。上げると、中身が同じでも携帯に更新の通知が
# 出るからである。決め方は版のファイルに書いた番号だけで、ここはそれを確かめる。
#
#   android/src-tauri/tauri.conf.json の版がタグと同じ → 組み直す
#   それ以外 → その版の APK が piep の公開済みのリリースにあり、その版のタグから
#              携帯に関わるファイルが一つも変わっていないこと。満たせば載せ直す
#
# 出力は GitHub Actions の出力の形（build=true|false / version=タグの X.Y.Z / carry=載せ直す X.Y.Z）。
param(
    [Parameter(Mandatory)][string]$Tag,
    [string]$Repository = '1211snowmaple/piep'
)
$ErrorActionPreference = 'Stop'

if ($Tag -notmatch '^v(\d+\.\d+\.\d+)$') { throw 'タグは vX.Y.Z の形にしてください' }
$version = $Matches[1]

# 携帯の APK の中身を決めるもの。説明の文書は含めない。
$paths = @('android', 'shared', 'crates', ':(exclude)android/*.md', ':(exclude)android/**/*.md')

$android = (Get-Content -LiteralPath android/src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
$package = (Get-Content -LiteralPath android/package.json -Raw | ConvertFrom-Json).version
$cargo = [regex]::Match((Get-Content -LiteralPath android/src-tauri/Cargo.toml -Raw), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if ($android -ne $package -or $android -ne $cargo) {
    throw "携帯の版がそろっていません: tauri.conf.json $android / package.json $package / Cargo.toml $cargo"
}

if ($android -eq $version) {
    Write-Output 'build=true'
    Write-Output "version=$version"
    Write-Output 'carry='
    exit 0
}

$bump = "携帯の版（android/ の package.json・tauri.conf.json・Cargo.toml）を $version に上げてください"

$work = Join-Path ([IO.Path]::GetTempPath()) "piep-android-plan-$([guid]::NewGuid())"
New-Item -ItemType Directory -Force $work | Out-Null
try {
    gh release download "v$android" --repo $Repository --pattern latest-android.json --dir $work 2>$null
    $manifestPath = Join-Path $work 'latest-android.json'
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $manifestPath)) {
        throw "携帯の版 $android の APK が $Repository のリリースにありません。$bump"
    }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.version -ne $android) {
        throw "v$android に載っている携帯の案内は $($manifest.version) を指しています。$bump"
    }
} finally {
    Remove-Item -Recurse -Force -LiteralPath $work -ErrorAction SilentlyContinue
}

git rev-parse --verify --quiet "refs/tags/v$android" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "タグ v$android がありません（履歴を全部取ってきていない可能性があります）" }
$changed = @(git diff --name-only "v$android" HEAD -- @paths)
if ($LASTEXITCODE -ne 0) { throw '差分を取れませんでした' }
if ($changed.Count -gt 0) {
    $list = ($changed | Select-Object -First 10) -join ', '
    throw "v$android から携帯に関わるファイルが $($changed.Count) 個変わっています（$list）。$bump"
}

Write-Output 'build=false'
Write-Output "version=$version"
Write-Output "carry=$android"
