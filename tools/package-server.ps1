# 単独サーバーの配布物を作り、空のフォルダーで動くことを確かめる。
#
# release.yml が使う。手元でも同じ確認ができる（要: Visual Studio の dumpbin）。
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$Executable = 'src-tauri/target/release/examples/piep-server.exe',
    [string]$OutputDir = 'target-server'
)
$ErrorActionPreference = 'Stop'

# 読み込む DLL の一覧。Windows に含まれないものが増えていたら、ここで止める。
# VC++ ランタイムは同梱する。
$allowed = '^(advapi32|bcrypt|bcryptprimitives|crypt32|d3d12|dbghelp|directml|dxgi|gdi32|iphlpapi|kernel32|msvcp140|msvcp140_1|ncrypt|ntdll|ole32|oleaut32|powrprof|psapi|rpcrt4|secur32|setupapi|shell32|shlwapi|user32|userenv|vcruntime140|vcruntime140_1|version|winhttp|ws2_32|api-ms-win-.*)\.dll$'
$companions = 'msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll'

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$dumpbin = & $vswhere -latest -products * -find '**/Hostx64/x64/dumpbin.exe' | Select-Object -First 1
if (-not $dumpbin) { throw 'dumpbin が見つかりません' }
$imports = & $dumpbin /nologo /dependents $Executable |
    ForEach-Object { $_.Trim() } |
    Where-Object { $_ -match '\.dll$' }
if (-not $imports) { throw '読み込む DLL を読み取れません' }
$unknown = $imports | Where-Object { $_ -notmatch $allowed }
if ($unknown) { throw "同梱していない DLL を読み込みます: $($unknown -join ', ')" }

New-Item -ItemType Directory -Force $OutputDir | Out-Null
$stage = Join-Path $OutputDir 'stage'
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory $stage | Out-Null
Copy-Item $Executable (Join-Path $stage 'piep-server.exe')
foreach ($name in $companions) {
    Copy-Item (Join-Path $env:SystemRoot "System32/$name") (Join-Path $stage $name)
}
# 添付 PDF の本文を読むのに要る。読み込みは実行時なので、上の DLL の検査では気づけない。
Copy-Item 'src-tauri/vendor/pdfium/pdfium.dll' (Join-Path $stage 'pdfium.dll')
$zip = Join-Path $OutputDir "piep-server-$Version-windows-x64.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip

# 空のフォルダーへ展開して、版と起動を確かめる。
$trial = Join-Path ([System.IO.Path]::GetTempPath()) ("piep-server-trial-" + [guid]::NewGuid())
Expand-Archive -Path $zip -DestinationPath $trial
$program = Join-Path $trial 'piep-server.exe'
$data = Join-Path $trial 'data'
try {
    $reported = (& $program --version).Trim()
    if ($reported -ne $Version) { throw "版が違います: $reported（期待は $Version）" }
    # 登録文字列を画面に出さない。
    & $program init --data $data | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'init に失敗しました' }
    $server = Start-Process -FilePath $program -ArgumentList 'serve', '--data', "`"$data`"" -PassThru -WindowStyle Hidden
    try {
        $hello = $null
        for ($attempt = 0; $attempt -lt 60 -and -not $hello; $attempt++) {
            Start-Sleep -Milliseconds 500
            try { $hello = Invoke-RestMethod -Uri 'http://127.0.0.1:38777/api/v1/hello' -TimeoutSec 2 } catch { }
        }
        if (-not $hello -or -not $hello.apiVersion) { throw 'hello に答えません' }
    } finally {
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
        $server.WaitForExit()
    }
} finally {
    Remove-Item -Recurse -Force $trial -ErrorAction SilentlyContinue
}
Remove-Item -Recurse -Force $stage
Write-Output $zip
