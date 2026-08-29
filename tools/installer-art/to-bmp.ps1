# render.mjs が書き出した PNG を、24bit の BMP へ変換する。
#
# NSIS も WiX も **BMP しか受け付けない**。PNG を渡すと、絵が出ないのではなく
# 「壊れたインストーラー」になる（NSIS は組み立てのときに落ちる）。
#
#   pwsh -File tools/installer-art/to-bmp.ps1

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$dir = Join-Path $root "src-tauri/installer"
$manifest = Join-Path $dir "rendered.json"
if (-not (Test-Path $manifest)) {
    throw "先に node tools/installer-art/render.mjs を実行すること"
}

foreach ($item in (Get-Content $manifest -Raw | ConvertFrom-Json)) {
    $png = $item.png
    $bmp = [System.IO.Path]::ChangeExtension($png, ".bmp")
    $source = [System.Drawing.Image]::FromFile($png)
    try {
        if ($source.Width -ne $item.width -or $source.Height -ne $item.height) {
            throw "$($item.name): 寸法が違う ($($source.Width)x$($source.Height))"
        }
        # 透過を持ったまま BMP にすると、入れ物によっては黒く出る。紙の上へ
        # 焼き付けてから 24bit で書く。
        $flat = New-Object System.Drawing.Bitmap($item.width, $item.height, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        try {
            $graphics = [System.Drawing.Graphics]::FromImage($flat)
            try {
                $graphics.Clear([System.Drawing.Color]::White)
                $graphics.DrawImage($source, 0, 0, $item.width, $item.height)
            } finally { $graphics.Dispose() }
            $flat.Save($bmp, [System.Drawing.Imaging.ImageFormat]::Bmp)
        } finally { $flat.Dispose() }
    } finally { $source.Dispose() }
    Remove-Item $png
    Write-Host "$($item.name)  ->  $bmp"
}
Remove-Item $manifest
