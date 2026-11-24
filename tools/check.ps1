<#
.SYNOPSIS
piep の検査の入口。手元でも CI でも、これ一つを呼ぶ。

.DESCRIPTION
段（Stage）ごとに検査を流す。何も指定しなければ frontend・rust・docs（push の前に流す分）。

  frontend  型検査・lint・単体試験（画面側）
  rust      Rust の試験（nextest）・clippy・rustfmt・EPUB の適合（EPUBCheck）
  docs      画面と Rust の境界の突き合わせ・方針書の数・文書サイトの組み立て
  visual    画面の組み立てと Playwright の視覚の試験
  android   携帯の画面側の組み立てと試験、携帯の Rust の試験・clippy・rustfmt
  apk       携帯の APK を鍵なしで組めること（重い）

CI の各ジョブは同じ段をこのスクリプトで呼ぶ。手順を workflow に直接書かない。
依存のインストール（npm ci など）はしない。手元では入っている前提、CI は前の手順で入れる。

.EXAMPLE
pwsh tools/check.ps1                 # push の前
pwsh tools/check.ps1 -Stage visual   # 画面を変えたとき
pwsh tools/check.ps1 -Stage frontend,rust,visual,android
#>
[CmdletBinding()]
param(
    [string[]]$Stage = @('frontend', 'rust', 'docs')
)
$ErrorActionPreference = 'Stop'
# pwsh tools/check.ps1 -Stage a,b と外から呼ぶと、コンマ区切りが1つの文字列で届く。
$Stage = @($Stage | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$known = @('frontend', 'rust', 'docs', 'visual', 'android', 'apk')
$unknown = @($Stage | Where-Object { $_ -notin $known })
if ($unknown.Count -gt 0) { throw "知らない段です: $($unknown -join ', ')（使える段: $($known -join ', ')）" }
# 日本語の Windows では既定が CP932 で、ログに残すと段の名前が化ける。
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Step([string]$name, [scriptblock]$body) {
    Write-Host "::group::$name"
    $started = Get-Date
    try {
        & $body
        if ($LASTEXITCODE -and $LASTEXITCODE -ne 0) { throw "$name が失敗しました（終了コード $LASTEXITCODE）" }
    } finally {
        Write-Host "::endgroup::"
    }
    Write-Host ('{0}: {1:N0} 秒' -f $name, ((Get-Date) - $started).TotalSeconds)
}

function Invoke-Native([string]$command, [string[]]$arguments) {
    & $command @arguments
    if ($LASTEXITCODE -ne 0) { throw "$command $($arguments -join ' ') が失敗しました（終了コード $LASTEXITCODE）" }
}

$manifest = 'src-tauri/Cargo.toml'

if ('frontend' -in $Stage) {
    Step '型検査' { Invoke-Native npm @('run', 'check') }
    Step 'lint' { Invoke-Native npm @('run', 'lint') }
    Step '画面の単体試験' { Invoke-Native npm @('test') }
}

if ('rust' -in $Stage) {
    if (-not (Get-Command cargo-nextest -ErrorAction SilentlyContinue)) {
        throw 'cargo-nextest がありません。cargo install cargo-nextest --locked で入れてください'
    }
    # 試験はテストごとに別のプロセスで走らせる。Tantivy は1つの索引に IndexWriter を
    # 1つしか許さず、同じプロセスで索引つきの試験を並べると Windows では書き込みが
    # "Access is denied" で落ちる。プロセスを分ければ、1プロセスに1つの棚という実際の
    # 使われ方のまま並べられる。一時フォルダーは試験ごとに一意の名前にしてある。
    Step 'Rust の試験' { Invoke-Native cargo @('nextest', 'run', '--manifest-path', $manifest, '--profile', 'ci') }
    Step 'clippy' { Invoke-Native cargo @('clippy', '--manifest-path', $manifest, '--all-targets', '--', '-D', 'warnings') }
    Step 'rustfmt' { Invoke-Native cargo @('fmt', '--manifest-path', $manifest, '--all', '--', '--check') }

    # 適合の根拠は EPUBCheck に置いている（docs/policy/02-principles.md）。内蔵の検証器は
    # 自分で書いた規則しか知らないので、外の権威の代わりにならない。原稿は利用者の
    # ライブラリを要さない。例は試験の手順で組み上がっているので、そのまま動かす。
    Step 'EPUB の適合' {
        $target = (cargo metadata --manifest-path $manifest --format-version 1 --no-deps | ConvertFrom-Json).target_directory
        $example = Join-Path $target 'debug/examples/epub_conformance.exe'
        if (-not (Test-Path -LiteralPath $example)) { throw "$example がありません" }
        $books = Join-Path ([IO.Path]::GetTempPath()) "piep-epub-conformance-$([guid]::NewGuid())"
        try {
            Invoke-Native $example @($books)
            $version = '5.3.0'
            # 手元で「致命的0・エラー0・警告0」を確認した配布物そのもの。版と指紋の両方を固定する。
            $expected = '6C07E68584B2E2CE2F89FE06E1246DFEAD3EB36B46B340E7D93524F29DCFF6C5'
            if (-not (Get-Command java -ErrorAction SilentlyContinue)) { throw 'java が見つかりません' }
            $cache = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) "piep-check/epubcheck-$version"
            $jar = Join-Path $cache "epubcheck-$version/epubcheck.jar"
            if (-not (Test-Path -LiteralPath $jar)) {
                New-Item -ItemType Directory -Force $cache | Out-Null
                $zip = Join-Path $cache 'epubcheck.zip'
                Invoke-WebRequest -Uri "https://github.com/w3c/epubcheck/releases/download/v$version/epubcheck-$version.zip" -OutFile $zip
                $actual = (Get-FileHash -Algorithm SHA256 $zip).Hash
                if ($actual -ne $expected) { Remove-Item -Force $zip; throw "EPUBCheck の指紋が違います: $actual" }
                Expand-Archive -Path $zip -DestinationPath $cache -Force
                Remove-Item -Force $zip
            }
            $epubs = @(Get-ChildItem $books -Filter *.epub)
            if ($epubs.Count -eq 0) { throw '検証する EPUB がありません' }
            $failed = @()
            foreach ($book in $epubs) {
                # 警告も落とす。文言はランナーの地域設定に左右させない。
                & java -jar $jar --locale en --failonwarnings $book.FullName
                if ($LASTEXITCODE -ne 0) { $failed += $book.Name }
            }
            if ($failed.Count -gt 0) { throw "EPUBCheck が問題を報告しました: $($failed -join ', ')" }
            $global:LASTEXITCODE = 0
        } finally {
            Remove-Item -Recurse -Force -LiteralPath $books -ErrorAction SilentlyContinue
        }
    }
}

if ('docs' -in $Stage) {
    # invoke の名前・イベント・スキーマの食い違いと、手で書いた数の古さを捕まえる。
    # 実行するまで気づかない境界のずれは、ここでしか見つからない。
    foreach ($tool in 'tools/ipc-extract/Cargo.toml', 'tools/api-types/Cargo.toml') {
        Step "試験 $tool" { Invoke-Native cargo @('test', '--manifest-path', $tool) }
        Step "clippy $tool" { Invoke-Native cargo @('clippy', '--manifest-path', $tool, '--all-targets', '--', '-D', 'warnings') }
        Step "rustfmt $tool" { Invoke-Native cargo @('fmt', '--manifest-path', $tool, '--all', '--', '--check') }
    }
    Step '携帯向け API の試験' { Invoke-Native cargo @('test', '--manifest-path', 'crates/piep-mobile-api/Cargo.toml') }
    Step 'API の型の突き合わせ' { Invoke-Native cargo @('run', '--manifest-path', 'tools/api-types/Cargo.toml', '--', '--repo', '.', '--check') }
    if (-not (Test-Path -LiteralPath 'docs-tools/node_modules')) {
        throw 'docs-tools の依存がありません。npm --prefix docs-tools ci で入れてください'
    }
    Push-Location docs-tools
    try {
        # docs/reference/ の生成物は追跡していない。手元に前回の写しが残っていると
        # 方針書の検査がそれを「古い」と言うので、先に今のソースから作り直す。
        Step '境界の突き合わせ' { Invoke-Native npm @('run', 'contract') }
        Step '方針書の検査' { Invoke-Native npm @('run', 'docs:check') }
        Step '文書サイトの組み立て' { Invoke-Native npm @('run', 'site:build') }
    } finally { Pop-Location }
}

if ('visual' -in $Stage) {
    # 組み立ての失敗を組み立ての失敗として読めるよう、Playwright の中ではなく先に組む。
    Step '画面の組み立て' { Invoke-Native npm @('run', 'build') }
    Step '視覚の試験' { Invoke-Native npm @('run', 'test:visual') }
}

if ('android' -in $Stage) {
    Push-Location android
    try {
        Step '携帯の画面の組み立て' { Invoke-Native npm @('run', 'build') }
        Step '携帯の画面の試験' { Invoke-Native npm @('test') }
        # 携帯の Rust（端末の記録・接続・本文の組み立て）の試験は、手元の PC の上で動く。
        # 版は android/rust-toolchain.toml が決めるので、android の中から呼ぶ。
        $mobile = 'src-tauri/Cargo.toml'
        Step '携帯の Rust の試験' { Invoke-Native cargo @('test', '--manifest-path', $mobile) }
        Step '携帯の clippy' { Invoke-Native cargo @('clippy', '--manifest-path', $mobile, '--all-targets', '--', '-D', 'warnings') }
        Step '携帯の rustfmt' { Invoke-Native cargo @('fmt', '--manifest-path', $mobile, '--all', '--', '--check') }
    } finally { Pop-Location }
}

if ('apk' -in $Stage) {
    Push-Location android
    try {
        $version = (Get-Content -LiteralPath src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
        Step '携帯の APK（鍵なし）' { ./tools/release.ps1 -Version $version -Unsigned -OutputDir (Join-Path ([IO.Path]::GetTempPath()) 'piep-apk-check') }
    } finally { Pop-Location }
}

Write-Host "検査はすべて通りました: $($Stage -join ', ')"
