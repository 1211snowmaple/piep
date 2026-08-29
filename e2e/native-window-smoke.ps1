param(
  [Parameter(Mandatory = $true)][string]$Executable,
  [string]$ArtifactDirectory = "test-results/native"
)

$ErrorActionPreference = "Stop"
$resolvedExecutable = (Resolve-Path -LiteralPath $Executable).Path
$resolvedWorkspace = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if (-not $resolvedExecutable.StartsWith($resolvedWorkspace, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Executable must stay inside the workspace"
}

New-Item -ItemType Directory -Force -Path $ArtifactDirectory | Out-Null
$resolvedArtifacts = (Resolve-Path -LiteralPath $ArtifactDirectory).Path
# A smoke test must never open or migrate the developer/runner's real library.
# Tauri derives its application directories from these Windows locations.
$env:APPDATA = Join-Path $resolvedArtifacts "appdata"
$env:LOCALAPPDATA = Join-Path $resolvedArtifacts "localappdata"
New-Item -ItemType Directory -Force -Path $env:APPDATA, $env:LOCALAPPDATA | Out-Null
$process = Start-Process -FilePath $resolvedExecutable -PassThru -WindowStyle Normal
try {
  Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class PiepNativeWindow {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  public delegate bool EnumWindowProc(IntPtr hwnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hwnd, EnumWindowProc callback, IntPtr lParam);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hwnd, System.Text.StringBuilder text, int maxCount);
}
"@
  # **取っ手ができたことと、大きさが決まったことは別。** 取っ手が出た瞬間に
  # 読むと、まだ形の決まっていない窓が 16x16 で返る。実際にそれで、同じ
  # コミットが Quality では通りリリースでだけ落ちた。大きさが決まるまで待つ。
  # 窓が出ないまま時間切れになれば、それはそれで失敗として残る。
  $deadline = [DateTime]::UtcNow.AddSeconds(45)
  $rect = New-Object PiepNativeWindow+RECT
  $width = 0
  $height = 0
  do {
    Start-Sleep -Milliseconds 250
    $process.Refresh()
    if ($process.HasExited) { throw "piep exited before opening a window" }
    if ($process.MainWindowHandle -eq 0) { continue }
    if (-not [PiepNativeWindow]::GetWindowRect($process.MainWindowHandle, [ref]$rect)) { continue }
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
  } until (($width -ge 900 -and $height -ge 600) -or [DateTime]::UtcNow -gt $deadline)
  if ($process.MainWindowHandle -eq 0) { throw "piep did not create a main window" }
  if ($width -lt 900 -or $height -lt 600) { throw "Initial window is smaller than 900x600: ${width}x${height}" }

  [PiepNativeWindow]::SetForegroundWindow($process.MainWindowHandle) | Out-Null
  Add-Type -AssemblyName System.Windows.Forms
  [System.Windows.Forms.SendKeys]::SendWait("^+s")
  Start-Sleep -Seconds 3

  $classes = [System.Collections.Generic.List[string]]::new()
  $callback = [PiepNativeWindow+EnumWindowProc]{ param($hwnd, $lParam)
    $name = New-Object System.Text.StringBuilder 256
    [void][PiepNativeWindow]::GetClassName($hwnd, $name, $name.Capacity)
    $classes.Add($name.ToString())
    return $true
  }
  [PiepNativeWindow]::EnumChildWindows($process.MainWindowHandle, $callback, [IntPtr]::Zero) | Out-Null
  if (-not ($classes | Where-Object { $_ -match "Chrome_WidgetWin|WebView" })) { throw "No WebView2 child surface was found" }

  [pscustomobject]@{ Width = $width; Height = $height; ChildWindowClasses = $classes } |
    ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $ArtifactDirectory "native-window.json") -Encoding utf8
}
finally {
  if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force }
}
