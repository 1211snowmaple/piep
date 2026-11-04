param([string]$ArtifactDirectory = 'test-results/installer-shortcuts')

$ErrorActionPreference = 'Stop'
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [IO.Path]::GetFullPath((Join-Path $workspace $ArtifactDirectory))
if (-not $output.StartsWith($workspace + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Installer test artifacts must stay inside the workspace'
}
New-Item -ItemType Directory -Force -Path $output | Out-Null
$template = Get-Content -LiteralPath (Join-Path $workspace 'src-tauri/installer/installer.nsi') -Raw
if ($template -notmatch '(?m)^!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED\s*$') {
    throw 'The desktop shortcut checkbox must default to unchecked'
}
$create = [regex]::Match($template, '(?ms)^Function CreateOrUpdateDesktopShortcut\r?\n.*?^FunctionEnd').Value
$automatic = [regex]::Match($template, '(?ms)^  ; Silent/passive installs.*?(?=^  !ifmacrodef NSIS_HOOK_POSTINSTALL)').Value
if (-not $create -or -not $automatic) { throw 'Could not extract the production shortcut code' }
# The same NSIS instructions run here, but the desktop variable points at a new
# workspace directory. No real desktop, app installation, or registry is changed.
$create = $create.Replace('$DESKTOP', '$FixtureDesktop')
$automatic = $automatic.Replace('$DESKTOP', '$FixtureDesktop')
$makensis = Join-Path $env:LOCALAPPDATA 'tauri/NSIS/makensis.exe'
$utils = Join-Path $workspace 'src-tauri/target/debug/nsis/x64/utils.nsh'
if (-not (Test-Path -LiteralPath $utils)) { throw 'Build the debug NSIS installer once before this test' }
$harness = @'
Unicode true
RequestExecutionLevel user
SilentInstall silent
OutFile "@@OUTPUT@@\shortcut-test.exe"
!include LogicLib.nsh
!include FileFunc.nsh
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "@@UTILS@@"
!define PRODUCTNAME "ShortcutAudit"
!define MAINBINARYNAME "audit-new"
!define BUNDLEID "piep.shortcut-audit"
Var FixtureDesktop
Var PassiveMode
Var UpdateMode
Var NoShortcutMode
Var WixMode
Var OldMainBinaryName
Var TestCase
Section
  ${GetOptions} $CMDLINE "/case=" $TestCase
  StrCpy $INSTDIR "@@OUTPUT@@\$TestCase"
  StrCpy $FixtureDesktop "$INSTDIR\desktop"
  CreateDirectory $FixtureDesktop
  StrCpy $PassiveMode 0
  StrCpy $UpdateMode 0
  StrCpy $NoShortcutMode 0
  StrCpy $WixMode 0
  StrCpy $OldMainBinaryName "audit-old.exe"
  ${If} $TestCase == "manual"
  ${OrIf} $TestCase == "passive"
  ${OrIf} $TestCase == "existing-passive"
  ${OrIf} $TestCase == "opt-in"
  ${OrIf} $TestCase == "no-shortcut"
    SetSilent normal
  ${EndIf}
  ${If} $TestCase == "passive"
  ${OrIf} $TestCase == "existing-passive"
    StrCpy $PassiveMode 1
  ${EndIf}
  ${If} $TestCase == "update"
    StrCpy $UpdateMode 1
  ${EndIf}
  ${If} $TestCase == "existing"
  ${OrIf} $TestCase == "existing-passive"
    StrCpy $UpdateMode 1
    CreateShortcut "$FixtureDesktop\${PRODUCTNAME}.lnk" "$INSTDIR\audit-old.exe"
  ${EndIf}
@@AUTOMATIC@@
  ${If} $TestCase == "opt-in"
    Call CreateOrUpdateDesktopShortcut
  ${EndIf}
  ${If} $TestCase == "no-shortcut"
    StrCpy $NoShortcutMode 1
    Call CreateOrUpdateDesktopShortcut
  ${EndIf}
  SetSilent silent
SectionEnd
@@CREATE@@
'@
$nsisOutput = $output.Replace('$', '$$').Replace('"', '$\"')
$nsisUtils = $utils.Replace('$', '$$').Replace('"', '$\"')
$harness = $harness.Replace('@@OUTPUT@@', $nsisOutput).Replace('@@UTILS@@', $nsisUtils).Replace('@@AUTOMATIC@@', $automatic).Replace('@@CREATE@@', $create)
$script = Join-Path $output 'shortcut-test.nsi'
$harness | Set-Content -LiteralPath $script -Encoding utf8NoBOM
& $makensis /V2 $script
if ($LASTEXITCODE -ne 0) { throw 'Could not compile the shortcut test harness' }
$results = foreach ($scenario in @('manual', 'silent', 'passive', 'update', 'opt-in', 'no-shortcut', 'existing', 'existing-passive')) {
    $shortcut = Join-Path $output "$scenario/desktop/ShortcutAudit.lnk"
    if (Test-Path -LiteralPath $shortcut) { Remove-Item -LiteralPath $shortcut }
    $process = Start-Process -FilePath (Join-Path $output 'shortcut-test.exe') -ArgumentList @('/S', "/case=$scenario") -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(20000)) { Stop-Process -Id $process.Id -Force; throw "NSIS test timed out: $scenario" }
    if ($process.ExitCode -ne 0) { throw "NSIS test failed: $scenario ($($process.ExitCode))" }
    $exists = Test-Path -LiteralPath $shortcut
    $expected = $scenario -in @('opt-in', 'existing', 'existing-passive')
    if ($exists -ne $expected) { throw "Wrong shortcut creation behavior: $scenario" }
    $target = if ($exists) { (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut).TargetPath } else { $null }
    if ($exists -and $target -ne (Join-Path $output "$scenario/audit-new.exe")) { throw "Wrong shortcut target: $scenario" }
    [pscustomobject]@{ Scenario = $scenario; Created = $exists; Target = $target; Passed = $true }
}
$results | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output 'results.json') -Encoding utf8NoBOM
$results | Format-Table -AutoSize
