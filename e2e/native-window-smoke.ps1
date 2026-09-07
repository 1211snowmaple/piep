param(
  [Parameter(Mandatory = $true)][string]$Executable,
  [string]$ArtifactDirectory = "test-results/native"
)

$ErrorActionPreference = "Stop"
$resolvedExecutable = (Resolve-Path -LiteralPath $Executable).Path
$resolvedWorkspace = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if (-not $resolvedExecutable.StartsWith($resolvedWorkspace + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Executable must stay inside the workspace"
}

New-Item -ItemType Directory -Force -Path $ArtifactDirectory | Out-Null
$resolvedArtifacts = (Resolve-Path -LiteralPath $ArtifactDirectory).Path
# A smoke test must never open or migrate the developer/runner's real library.
# Windows Known Folders ignore APPDATA/LOCALAPPDATA overrides. The app changes
# its Tauri identifier before initializing plugins, the library, or WebView2.
$previousRunId = $env:PIEP_SMOKE_TEST_ID
$env:PIEP_SMOKE_TEST_ID = [guid]::NewGuid().ToString('N')
$identifier = "com.hiron.piep.smoke.$($env:PIEP_SMOKE_TEST_ID)"
$appData = Join-Path ([Environment]::GetFolderPath('ApplicationData')) $identifier
$localAppData = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) $identifier
if ((Test-Path -LiteralPath $appData) -or (Test-Path -LiteralPath $localAppData)) {
  throw "Smoke-test directories must be new"
}
$process = $null
try {
  $process = Start-Process -FilePath $resolvedExecutable -PassThru -WindowStyle Hidden
  Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class PiepNativeWindow {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
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

  # Do not send global keystrokes: focus can belong to the user's normal app.
  $database = Join-Path $appData 'piep.db'
  while (-not (Test-Path -LiteralPath $database)) {
    if ($process.HasExited -or [DateTime]::UtcNow -gt $deadline) {
      throw "Smoke test did not initialize its isolated library"
    }
    Start-Sleep -Milliseconds 250
    $process.Refresh()
  }

  $classes = [System.Collections.Generic.List[string]]::new()
  $callback = [PiepNativeWindow+EnumWindowProc]{ param($hwnd, $lParam)
    $name = New-Object System.Text.StringBuilder 256
    [void][PiepNativeWindow]::GetClassName($hwnd, $name, $name.Capacity)
    $classes.Add($name.ToString())
    return $true
  }
  [PiepNativeWindow]::EnumChildWindows($process.MainWindowHandle, $callback, [IntPtr]::Zero) | Out-Null
  if (-not ($classes | Where-Object { $_ -match "Chrome_WidgetWin|WebView" })) { throw "No WebView2 child surface was found" }

  [pscustomobject]@{ Width = $width; Height = $height; ChildWindowClasses = $classes; Identifier = $identifier; AppData = $appData; LocalAppData = $localAppData } |
    ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $ArtifactDirectory "native-window.json") -Encoding utf8
}
finally {
  if ($null -ne $process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Force }
  # The library must be released before its files can be removed.
  if ($null -ne $process) { [void]$process.WaitForExit(10000) }
  # Keep the log: the run's own directories are about to go, and a failure here
  # is usually explained by what the app wrote on the way up.
  $log = Join-Path $appData "logs"
  if (Test-Path -LiteralPath $log) {
    Copy-Item -LiteralPath $log -Destination (Join-Path $ArtifactDirectory "logs") -Recurse -Force -ErrorAction SilentlyContinue
  }
  # Each run invents a fresh identifier, so these directories belong to this run
  # alone. Leaving them behind puts a throwaway library beside the real one in
  # the developer's AppData, once per run.
  # WebView2 keeps its own processes a moment longer than the host, and holds
  # EBWebView open while they go. Retry rather than leave the directory behind.
  foreach ($directory in @($appData, $localAppData)) {
    for ($attempt = 0; $attempt -lt 20 -and $directory -and (Test-Path -LiteralPath $directory); $attempt++) {
      Remove-Item -LiteralPath $directory -Recurse -Force -ErrorAction SilentlyContinue
      if (Test-Path -LiteralPath $directory) { Start-Sleep -Milliseconds 500 }
    }
    if ($directory -and (Test-Path -LiteralPath $directory)) {
      Write-Warning "Could not remove the smoke-test directory: $directory"
    }
  }
  $env:PIEP_SMOKE_TEST_ID = $previousRunId
}
