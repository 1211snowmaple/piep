//! `piep-server install`。置き場を作り、起動時に動くよう登録して、動かす。
//!
//! 外へどう出すかは利用者が決める。特定の製品を検出も実行もしない。

use super::durable::{write_atomic, StagedWrite};
use super::standalone::{command_admin_pair, create_config, ServerPaths};
use super::store::ServerStore;
use super::update::{current_version, VERSION};
use super::watchdog::ReadyMark;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use uuid::Uuid;

const TASK_NAME: &str = "piep-server";
const DEFAULT_PORT_TEXT: &str = "38777";
const COMPANION_FILES: [&str; 5] = [
    "msvcp140.dll",
    "msvcp140_1.dll",
    "pdfium.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exposure {
    /// トンネルやリバースプロキシの裏。この機械の中だけで平文で待ち受ける。
    Proxy,
    /// この機械の接続口へ直接。自己署名の TLS で待ち受ける。
    Direct,
}

#[derive(Debug, Default)]
pub struct InstallOptions {
    pub data: Option<PathBuf>,
    pub url: Option<String>,
    pub expose: Option<Exposure>,
    /// 起動の仕組みを自分で用意する。タスクを登録せず、管理者権限も求めない。
    pub no_task: bool,
}

pub fn parse_exposure(text: &str) -> Result<Exposure, String> {
    match text {
        "proxy" => Ok(Exposure::Proxy),
        "direct" => Ok(Exposure::Direct),
        _ => Err("--expose は proxy か direct にしてください".into()),
    }
}

fn config_text(exposure: Exposure, url: &str) -> String {
    match exposure {
        Exposure::Proxy => format!(
            r#"# piep-server の設定
listen = "127.0.0.1:{DEFAULT_PORT_TEXT}"   # トンネルやリバースプロキシからここへ転送する
public_url = "{url}"
# admin_listen = "127.0.0.1:38778"   # 管理者を別の口に分ける。外へ公開するなら分ける

[update]
auto = true
window = "03:00-06:00"
"#
        ),
        Exposure::Direct => format!(
            r#"# piep-server の設定
listen = "0.0.0.0:{DEFAULT_PORT_TEXT}"
public_url = "{url}"
tls = "self-signed"
# admin_listen = "127.0.0.1:38778"   # 管理者を別の口に分ける。外へ公開するなら分ける

[update]
auto = true
window = "03:00-06:00"
"#
        ),
    }
}

/// タスク スケジューラの定義。`schtasks /SC ONSTART` の既定は 3 日で止まり、電池でも止まるので
/// XML で書く。
fn task_xml(data: &Path) -> String {
    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
    let command = escape(&data.join("piep-server.exe").display().to_string());
    let arguments = escape(&format!("serve --data \"{}\"", data.display()));
    let directory = escape(&data.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>piep のサーバー</Description>
  </RegistrationInfo>
  <Triggers>
    <BootTrigger>
      <Enabled>true</Enabled>
    </BootTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>S-1-5-18</UserId>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
      <WorkingDirectory>{directory}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn ask(question: &str, default: Option<&str>) -> Result<String, String> {
    match default {
        Some(default) => print!("{question} [{default}]: "),
        None => print!("{question}: "),
    }
    std::io::stdout()
        .flush()
        .map_err(|error| format!("画面に書けません: {error}"))?;
    let mut line = String::new();
    let read = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("入力を読めません: {error}"))?;
    if read == 0 {
        return Err("入力が終わりました。--data と --expose と --url を指定してください".into());
    }
    let answer = line.trim();
    if answer.is_empty() {
        default
            .map(str::to_string)
            .ok_or_else(|| "入力してください".to_string())
    } else {
        Ok(answer.to_string())
    }
}

fn default_data_dir() -> PathBuf {
    std::env::var_os("ProgramData")
        .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from)
        .join("piep-server")
}

fn host_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "このPCの名前".into())
}

fn normalized_url(url: &str) -> Result<String, String> {
    if !url.starts_with("https://") {
        return Err("URL は https:// で始めてください".into());
    }
    Ok(if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    })
}

/// 尋ねて足りない項目を埋める。設定がすでにあれば、出し方は尋ねない。
fn complete_options(
    mut options: InstallOptions,
) -> Result<(PathBuf, Option<Exposure>, Option<String>), String> {
    let data = match options.data.take() {
        Some(data) => data,
        None => PathBuf::from(ask(
            "置き場所（本棚が入るので、大きいドライブを選べます）",
            Some(&default_data_dir().display().to_string()),
        )?),
    };
    let paths = ServerPaths::new(&data)?;
    if paths.config().exists() {
        return Ok((paths.root, None, options.url));
    }
    let exposure = match options.expose {
        Some(exposure) => exposure,
        None => {
            println!();
            println!("携帯からの接続方法を選んでください。");
            println!(
                "  1. トンネルやリバースプロキシを経由する（公開 URL で、どこからでも使えます）"
            );
            println!("  2. この PC で直接待ち受ける（接続できる範囲は、この PC のネットワークによります）");
            match ask("番号", Some("1"))?.as_str() {
                "1" => Exposure::Proxy,
                "2" => Exposure::Direct,
                _ => return Err("1 か 2 を選んでください".into()),
            }
        }
    };
    let url = match options.url {
        Some(url) => url,
        None => match exposure {
            Exposure::Proxy => ask("公開 URL（https://...）", None)?,
            Exposure::Direct => ask(
                "この PC に接続する URL",
                Some(&format!("https://{}:{DEFAULT_PORT_TEXT}/", host_name())),
            )?,
        },
    };
    Ok((paths.root, Some(exposure), Some(normalized_url(&url)?)))
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

struct FileChange {
    target: PathBuf,
    replacement: Option<StagedWrite>,
}

impl FileChange {
    fn prepare(
        target: PathBuf,
        stage: impl FnOnce(&Path) -> Result<Option<StagedWrite>, String>,
    ) -> Result<Self, String> {
        file_exists(&target)?;
        let replacement = stage(&target)?;
        Ok(Self {
            target,
            replacement,
        })
    }
}

fn file_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.permissions().readonly() => Ok(true),
        Ok(_) => Err(format!(
            "{} は差し替えられるファイルではありません",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{} を調べられません: {error}", path.display())),
    }
}

/// 動いている実行物に触れず、差し替える内容を同じディレクトリへ書いておく。
fn prepare_programs(paths: &ServerPaths) -> Result<Vec<FileChange>, String> {
    check_install_transitions(paths)?;
    let executable =
        std::env::current_exe().map_err(|error| format!("自分の場所が分かりません: {error}"))?;
    let source = executable.parent().ok_or("自分の場所が分かりません")?;
    let version_dir = paths.version_dir(VERSION);
    fs::create_dir_all(&version_dir).map_err(|error| format!("置き場を作れません: {error}"))?;
    let mut changes = Vec::new();
    for path in [paths.pending_version(), paths.previous_version()] {
        changes.push(FileChange::prepare(path, |_| Ok(None))?);
    }
    for target in [&version_dir, &paths.root] {
        for path in stale_dlls(target, source)? {
            changes.push(FileChange::prepare(path, |_| Ok(None))?);
        }
        let destination = target.join("piep-server.exe");
        if !same_file(&executable, &destination) {
            changes.push(FileChange::prepare(destination, |path| {
                StagedWrite::copy_from(path, &executable).map(Some)
            })?);
        }
        for name in COMPANION_FILES {
            let from = source.join(name);
            let to = target.join(name);
            // 同梱されていない組み立てでは、機械に入っているものを使う。
            if from
                .try_exists()
                .map_err(|error| format!("{} を調べられません: {error}", from.display()))?
                && !same_file(&from, &to)
            {
                changes.push(FileChange::prepare(to, |path| {
                    StagedWrite::copy_from(path, &from).map(Some)
                })?);
            }
        }
    }
    changes.push(FileChange::prepare(paths.current_version(), |path| {
        StagedWrite::new(path, VERSION.as_bytes()).map(Some)
    })?);
    Ok(changes)
}

/// 再導入では、アクセス権を塞ぐ前からあった版の指示を信頼しない。
/// 動かすのは今回配置する版だけで、待機中や戻し中の実行ファイルを起動させない。
fn check_install_transitions(paths: &ServerPaths) -> Result<(), String> {
    // 保護された既存の置き場では、戻しの印はDB復旧に必要な状態である。
    // 再導入で消して部分的な復元を隠さず、見張り役による復旧を先に済ませる。
    for name in ["rollback-version", "requested-rollback-version"] {
        let path = paths.state().join(name);
        if path
            .try_exists()
            .map_err(|error| format!("{} を調べられません: {error}", path.display()))?
        {
            return Err(
                "前のバージョンへの復旧が残っています。serve で復旧を完了してから入れ直してください"
                    .into(),
            );
        }
    }
    Ok(())
}

/// DLL の残骸を昇格後の本体に読み込ませない。今回の配布元にある既知の DLL だけを残す。
fn stale_dlls(target: &Path, source: &Path) -> Result<Vec<PathBuf>, String> {
    let mut stale = Vec::new();
    for entry in fs::read_dir(target).map_err(|error| format!("実行物を調べられません: {error}"))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        {
            continue;
        }
        let name = entry.file_name();
        let supplied = COMPANION_FILES
            .iter()
            .any(|allowed| name.eq_ignore_ascii_case(allowed) && source.join(allowed).is_file());
        if !supplied {
            stale.push(path);
        }
    }
    Ok(stale)
}

/// 停止後に差し替えたファイルを、起動を確認するまで退避しておく。
#[derive(Default)]
struct PlacedFiles {
    backups: Vec<(PathBuf, Option<PathBuf>)>,
}

impl PlacedFiles {
    fn apply(&mut self, changes: Vec<FileChange>) -> Result<(), String> {
        for FileChange {
            target,
            replacement,
        } in changes
        {
            let backup = if file_exists(&target)? {
                let backup = target.with_file_name(format!(".{}.install-backup", Uuid::now_v7()));
                fs::rename(&target, &backup)
                    .map_err(|error| format!("{} を退避できません: {error}", target.display()))?;
                Some(backup)
            } else {
                None
            };
            self.backups.push((target, backup));
            if let Some(replacement) = replacement {
                replacement.commit()?;
            }
        }
        Ok(())
    }

    fn restore(self) -> Vec<String> {
        let mut errors = Vec::new();
        for (target, backup) in self.backups.into_iter().rev() {
            let result = match backup {
                Some(backup) => fs::rename(backup, &target),
                None => remove_if_exists(&target),
            };
            if let Err(error) = result {
                errors.push(format!("{} を元に戻せません: {error}", target.display()));
            }
        }
        errors
    }

    fn finish(self) {
        for (_, backup) in self.backups {
            if let Some(backup) = backup {
                let _ = fs::remove_file(backup);
            }
        }
    }
}

fn remove_if_exists(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn run_quiet(program: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("{program} を動かせません: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{program} が失敗しました（{}）", output.status))
    }
}

#[cfg(windows)]
fn is_elevated() -> bool {
    // SAFETY: 引数を取らず、自分のプロセスのトークンを見るだけである。
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}

/// 管理者権限で自分を開き直す。開いた窓で続きを行う。
#[cfg(windows)]
fn relaunch_elevated(args: &[String]) -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("自分の場所が分かりません: {error}"))?;
    let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let mut inner: Vec<String> = args
        .iter()
        .map(|argument| format!("\"{}\"", argument.replace('"', "")))
        .collect();
    if inner.is_empty() {
        inner.push("install".into());
    }
    let command = format!(
        "Start-Process -FilePath {} -ArgumentList {} -Verb RunAs",
        quote(&executable.display().to_string()),
        quote(&inner.join(" ")),
    );
    run_quiet(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &command],
    )
    .map_err(|_| "管理者権限を得られませんでした".to_string())
}

#[cfg(windows)]
fn stop_task() {
    // 動いていなければ失敗するが、それでよい。
    let _ = Command::new("schtasks")
        .args(["/End", "/TN", TASK_NAME])
        .output();
}

#[cfg(windows)]
fn start_task() -> Result<(), String> {
    run_quiet("schtasks", &["/Run", "/TN", TASK_NAME])
}

#[cfg(windows)]
struct TaskDefinition(PathBuf);

#[cfg(windows)]
impl TaskDefinition {
    fn prepare(paths: &ServerPaths, xml: &str) -> Result<Self, String> {
        let file = Self(paths.state().join(format!(".{}.xml", Uuid::now_v7())));
        // schtasks は UTF-16 の XML を読む。
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
        write_atomic(&file.0, &bytes)?;
        Ok(file)
    }

    fn create(&self) -> Result<(), String> {
        let file = self.0.display().to_string();
        run_quiet(
            "schtasks",
            &["/Create", "/TN", TASK_NAME, "/XML", &file, "/F"],
        )
    }
}

#[cfg(windows)]
impl Drop for TaskDefinition {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// 未登録と照会の失敗を区別し、既存登録があれば復元できるようにする。
#[cfg(windows)]
fn previous_task(paths: &ServerPaths) -> Result<Option<TaskDefinition>, String> {
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$scheduler = New-Object -ComObject 'Schedule.Service'
$scheduler.Connect()
foreach ($task in $scheduler.GetFolder('\').GetTasks(1)) {{
    if ($task.Name -eq '{TASK_NAME}') {{
        [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
        [Console]::Write($task.Xml)
        break
    }}
}}
"#
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|error| format!("以前のタスクを調べられません: {error}"))?;
    if !output.status.success() {
        return Err(format!("以前のタスクを調べられません（{}）", output.status));
    }
    if output.stdout.is_empty() {
        return Ok(None);
    }
    let xml = std::str::from_utf8(&output.stdout)
        .map_err(|error| format!("以前のタスクの定義を読めません: {error}"))?;
    TaskDefinition::prepare(paths, xml).map(Some)
}

/// 本体が `hello` を通すまで待つ。
async fn wait_for_ready(paths: &ServerPaths) -> Result<(), String> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(60) {
        if let Ok(bytes) = fs::read(paths.ready_mark()) {
            if let Ok(mark) = serde_json::from_slice::<ReadyMark>(&bytes) {
                if Some(mark.version) == current_version(paths) {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(format!(
        "サーバーが 60 秒以内に動きませんでした。{} を見てください",
        paths.watchdog_log().display()
    ))
}

fn pause_if_own_window(interactive: bool) {
    if interactive {
        print!("\nEnter で閉じます");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
    }
}

fn prepare_layout(
    root: &Path,
    exposure: Option<Exposure>,
    url: Option<&str>,
) -> Result<ServerPaths, String> {
    let paths = ServerPaths::new(root)?;
    fs::create_dir_all(paths.state()).map_err(|error| format!("置き場を作れません: {error}"))?;
    create_config(&paths, || {
        let exposure = exposure.ok_or("出し方が決まっていません")?;
        let url = url.ok_or("URL が決まっていません")?;
        Ok(config_text(exposure, url))
    })?;
    Ok(paths)
}

/// 置き場・設定・プログラムを整える。タスクには触らない。
pub fn lay_out(
    root: &Path,
    exposure: Option<Exposure>,
    url: Option<&str>,
) -> Result<ServerPaths, String> {
    let paths = prepare_layout(root, exposure, url)?;
    let changes = prepare_programs(&paths)?;
    let mut placed = PlacedFiles::default();
    if let Err(error) = placed.apply(changes) {
        let mut errors = vec![error];
        errors.extend(placed.restore());
        return Err(errors.join("\n"));
    }
    placed.finish();
    Ok(paths)
}

#[cfg(windows)]
async fn install_scheduled(
    paths: &ServerPaths,
    find_previous: impl FnOnce(&ServerPaths) -> Result<Option<TaskDefinition>, String>,
    mut stop: impl AsyncFnMut(),
) -> Result<(), String> {
    let mut changes = prepare_programs(paths)?;
    changes.push(FileChange::prepare(paths.ready_mark(), |_| Ok(None))?);
    let task = TaskDefinition::prepare(paths, &task_xml(&paths.root))?;
    let previous = find_previous(paths)?;
    if previous.is_some() {
        stop().await;
    }

    let mut placed = PlacedFiles::default();
    let mut registered = false;
    let result = async {
        // 準備中に見張り役が戻しを始めていたら、その印を保護したまま再起動する。
        check_install_transitions(paths)?;
        placed.apply(changes)?;
        task.create()?;
        registered = true;
        start_task()?;
        wait_for_ready(paths).await
    }
    .await;
    let Err(error) = result else {
        placed.finish();
        return Ok(());
    };

    if registered {
        stop().await;
    }
    let mut errors = vec![error];
    errors.extend(placed.restore());
    if let Some(previous) = previous {
        let restarted = (|| {
            if registered {
                previous.create()?;
            }
            start_task()
        })();
        match restarted {
            Ok(()) => errors.push("以前のサーバーの起動タスクを再実行しました".into()),
            Err(error) => errors.push(format!("以前のサーバーを再起動できません: {error}")),
        }
    } else if registered {
        if let Err(error) = run_quiet("schtasks", &["/Delete", "/TN", TASK_NAME, "/F"]) {
            errors.push(format!("新しい起動タスクを片付けられません: {error}"));
        }
    }
    Err(errors.join("\n"))
}

/// 入れ終わったときに見せるもの。
pub struct InstallDone {
    /// デスクトップ版に貼る、管理者の登録文字列。
    pub pairing: String,
    /// この機械のブラウザで開く、管理画面の一度きりのリンク。管理者のアカウントをここで作る。
    pub admin_link: Option<String>,
}

/// `piep-server install` の本体。登録文字列を返す。
#[cfg(windows)]
/// 入れ直しで管理者がすでにいれば、登録文字列は出さない。
async fn install_inner(options: InstallOptions) -> Result<Option<InstallDone>, String> {
    let no_task = options.no_task;
    let (root, exposure, url) = complete_options(options)?;
    validate_install_directory(&root)?;
    let paths = if no_task {
        lay_out(&root, exposure, url.as_deref())?
    } else {
        // 断るなら、動いているサーバーを止める前に断る。
        super::install_acl::secure_install_root(&root)?;
        let paths = prepare_layout(&root, exposure, url.as_deref())?;
        install_scheduled(&paths, previous_task, async || {
            stop_task();
            // 止まって実行ファイルが解放されるのを少し待つ。
            tokio::time::sleep(Duration::from_secs(2)).await;
        })
        .await?;
        paths
    };
    if no_task {
        println!();
        println!("置きました。次のコマンドを、お使いの仕組みで起動時に動かしてください。");
        println!(
            "  \"{}\" serve --data \"{}\"",
            paths.root.join("piep-server.exe").display(),
            paths.root.display()
        );
        println!("SYSTEM など管理者の権限で動かすときは、このフォルダーを管理者だけが書き換えられるようにしてください。");
    } else {
        println!();
        println!("動き始めました。この PC を起動すると、自動で動きます。");
    }
    if ServerStore::open(&paths.store())?.has_admin()? {
        return Ok(None);
    }
    let pairing = command_admin_pair(&paths.root, url, false).await?;
    // 起動の仕組みを自分で用意するときは、まだ動いていないのでリンクを出さない。
    let admin_link = if no_task {
        None
    } else {
        super::standalone::command_admin_setup(&paths.root, None).ok()
    };
    Ok(Some(InstallDone {
        pairing,
        admin_link,
    }))
}

/// 既存の無関係なフォルダー全体のアクセス権を変えない。
fn validate_install_directory(root: &Path) -> Result<(), String> {
    if root.parent().is_none() {
        return Err("ドライブ全体ではなく、サーバー専用のフォルダーを選んでください".into());
    }
    if root.exists() {
        let mut entries =
            fs::read_dir(root).map_err(|error| format!("置き場を調べられません: {error}"))?;
        let occupied = entries
            .next()
            .transpose()
            .map_err(|error| error.to_string())?
            .is_some();
        let paths = ServerPaths::new(root)?;
        let known = super::standalone::load_config(&paths).is_ok()
            && (is_server_store(&paths.store())
                || current_version(&paths).is_some_and(|version| {
                    paths
                        .version_dir(&version)
                        .join("piep-server.exe")
                        .is_file()
                }));
        if occupied && !known {
            return Err(
                "空のフォルダーか、すでに piep-server を置いたフォルダーを選んでください".into(),
            );
        }
    }
    Ok(())
}

fn is_server_store(path: &Path) -> bool {
    let Ok(connection) =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return false;
    };
    connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('devices', 'pairings', 'meta')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .is_ok_and(|count| count == 3)
}

/// 引数なしで開いたときも、ここへ来る。
#[cfg(windows)]
pub async fn command_install(
    options: InstallOptions,
    args: &[String],
    interactive: bool,
) -> Result<Option<InstallDone>, String> {
    if !options.no_task && !is_elevated() {
        relaunch_elevated(args)?;
        return Ok(None);
    }
    println!("piep-server {VERSION} を入れます。");
    match install_inner(options).await {
        Ok(Some(done)) => Ok(Some(done)),
        Ok(None) => {
            println!("管理者は登録済みなので、そのまま使えます。");
            pause_if_own_window(interactive);
            Ok(None)
        }
        Err(error) => {
            eprintln!("{error}");
            pause_if_own_window(interactive);
            Err(error)
        }
    }
}

#[cfg(windows)]
pub fn finish_install(interactive: bool) {
    pause_if_own_window(interactive);
}

#[cfg(windows)]
pub fn command_uninstall(args: &[String]) -> Result<bool, String> {
    if !is_elevated() {
        relaunch_elevated(args)?;
        return Ok(false);
    }
    stop_task();
    run_quiet("schtasks", &["/Delete", "/TN", TASK_NAME, "/F"])?;
    println!("起動時の登録を消して止めました。置き場所は消していません。");
    Ok(true)
}

#[cfg(not(windows))]
pub async fn command_install(
    _options: InstallOptions,
    _args: &[String],
    _interactive: bool,
) -> Result<Option<InstallDone>, String> {
    Err("install は Windows だけで動きます".into())
}

#[cfg(not(windows))]
pub fn finish_install(_interactive: bool) {}

#[cfg(not(windows))]
pub fn command_uninstall(_args: &[String]) -> Result<bool, String> {
    Err("uninstall は Windows だけで動きます".into())
}

#[cfg(test)]
mod tests {
    use super::super::standalone::ServerConfig;
    use super::*;

    #[test]
    fn generated_configs_are_valid() {
        for exposure in [Exposure::Proxy, Exposure::Direct] {
            let config: ServerConfig =
                toml::from_str(&config_text(exposure, "https://server.example/")).expect("config");
            config.validate().expect("valid");
            assert_eq!(
                config.public_url().as_deref(),
                Some("https://server.example/")
            );
        }
    }

    #[test]
    fn no_product_is_assumed_in_the_generated_config() {
        for exposure in [Exposure::Proxy, Exposure::Direct] {
            let text = config_text(exposure, "https://server.example/").to_lowercase();
            assert!(!text.contains("tailscale") && !text.contains("funnel"));
        }
    }

    #[test]
    fn urls_must_be_https_and_end_with_a_slash() {
        assert_eq!(
            normalized_url("https://a.example").as_deref(),
            Ok("https://a.example/")
        );
        assert_eq!(
            normalized_url("https://a.example/").as_deref(),
            Ok("https://a.example/")
        );
        assert!(normalized_url("http://a.example/").is_err());
        assert!(parse_exposure("funnel").is_err());
    }

    #[test]
    fn the_task_never_times_out_and_escapes_the_path() {
        let xml = task_xml(Path::new(r"D:\piep & co\server"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<UserId>S-1-5-18</UserId>"));
        assert!(xml.contains(r"D:\piep &amp; co\server\piep-server.exe"));
        assert!(xml.contains(r"serve --data &quot;D:\piep &amp; co\server&quot;"));
    }

    #[test]
    fn laying_out_twice_keeps_the_config() {
        let dir = crate::server::tests::TestRoot::created("install");
        let root = dir.0.join("server");
        let paths =
            lay_out(&root, Some(Exposure::Proxy), Some("https://a.example/")).expect("first");
        assert!(paths.config().exists());
        assert_eq!(current_version(&paths).as_deref(), Some(VERSION));
        assert!(paths.version_dir(VERSION).join("piep-server.exe").exists());
        fs::write(paths.config(), "listen = \"127.0.0.1:1234\"\n").expect("edit");
        lay_out(&root, None, None).expect("again");
        assert_eq!(
            fs::read_to_string(paths.config()).expect("config"),
            "listen = \"127.0.0.1:1234\"\n"
        );
    }
    #[test]
    fn installation_requires_a_dedicated_directory() {
        let dir = crate::server::tests::TestRoot::created("install_directory");
        assert!(validate_install_directory(&dir.0).is_ok());
        fs::write(dir.0.join("unrelated.txt"), b"keep").unwrap();
        assert!(validate_install_directory(&dir.0).is_err());
        fs::write(dir.0.join("server.toml"), b"").unwrap();
        assert!(validate_install_directory(&dir.0).is_err());
        let paths = ServerPaths::new(&dir.0).unwrap();
        drop(ServerStore::open(&paths.store()).unwrap());
        assert!(validate_install_directory(&dir.0).is_ok());
        assert_eq!(fs::read(dir.0.join("unrelated.txt")).unwrap(), b"keep");
        #[cfg(windows)]
        assert!(validate_install_directory(Path::new(r"C:\")).is_err());
    }

    #[test]
    fn reinstall_clears_old_instructions_after_recovery_and_removes_stale_dlls() {
        let dir = crate::server::tests::TestRoot::created("reinstall_contents");
        let paths = ServerPaths::new(&dir.0).unwrap();
        fs::create_dir_all(paths.state()).unwrap();
        write_atomic(&paths.current_version(), VERSION.as_bytes()).unwrap();
        for path in [paths.pending_version(), paths.previous_version()] {
            fs::write(path, b"9.9.9").unwrap();
        }
        for name in ["rollback-version", "requested-rollback-version"] {
            let marker = paths.state().join(name);
            fs::write(&marker, b"9.9.9").unwrap();
            assert!(prepare_programs(&paths).is_err());
            assert!(marker.exists());
            assert!(paths.pending_version().exists());
            assert!(paths.previous_version().exists());
            fs::remove_file(marker).unwrap();
        }
        let mut placed = PlacedFiles::default();
        placed.apply(prepare_programs(&paths).unwrap()).unwrap();
        placed.finish();
        assert!(current_version(&paths).is_some());
        for path in [paths.pending_version(), paths.previous_version()] {
            assert!(!path.exists());
        }

        let source = dir.0.join("source");
        let target = dir.0.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("pdfium.dll"), b"supplied").unwrap();
        fs::write(target.join("pdfium.dll"), b"old but will be replaced").unwrap();
        fs::write(target.join("vcruntime140.dll"), b"not supplied").unwrap();
        fs::write(target.join("unknown.DLL"), b"not in the package").unwrap();
        fs::write(target.join("notes.txt"), b"keep").unwrap();
        let changes = stale_dlls(&target, &source)
            .unwrap()
            .into_iter()
            .map(|path| FileChange::prepare(path, |_| Ok(None)).unwrap())
            .collect();
        let mut placed = PlacedFiles::default();
        placed.apply(changes).unwrap();
        placed.finish();
        assert!(target.join("pdfium.dll").is_file());
        assert!(!target.join("vcruntime140.dll").exists());
        assert!(!target.join("unknown.DLL").exists());
        assert_eq!(fs::read(target.join("notes.txt")).unwrap(), b"keep");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn refused_reinstalls_leave_the_old_task_running() {
        let dir = crate::server::tests::TestRoot::created("reinstall_refused");
        let paths = ServerPaths::new(&dir.0).unwrap();
        fs::create_dir_all(paths.state()).unwrap();
        let marker = paths.state().join("rollback-version");
        write_atomic(&marker, b"1.1.0").unwrap();
        write_atomic(&paths.pending_version(), b"1.1.0").unwrap();
        let existing_task =
            |paths: &ServerPaths| Ok(Some(TaskDefinition(paths.state().join("test-task.xml"))));
        let stop = async || panic!("準備に失敗したのにタスクを止めようとした");

        let error = install_scheduled(&paths, existing_task, stop)
            .await
            .unwrap_err();
        assert!(error.contains("前のバージョンへの復旧が残っています"));
        assert!(marker.exists());
        assert!(paths.pending_version().exists());

        fs::remove_file(marker).unwrap();
        fs::create_dir(paths.root.join("stale.dll")).unwrap();
        let error = install_scheduled(&paths, existing_task, stop)
            .await
            .unwrap_err();
        assert!(error.contains("stale.dll"));
        assert!(error.contains("差し替えられるファイルではありません"));
        assert!(paths.pending_version().exists());
    }

    #[test]
    fn a_failed_placement_restores_replaced_and_removed_files() {
        let dir = crate::server::tests::TestRoot::created("reinstall_restore");
        let removed = dir.0.join("previous-version");
        let replaced = dir.0.join("current-version");
        let added = dir.0.join("piep-server.exe");
        let blocked = dir.0.join("pending-version");
        write_atomic(&removed, b"1.0.0").unwrap();
        write_atomic(&replaced, b"1.1.0").unwrap();
        let changes = vec![
            FileChange::prepare(removed.clone(), |_| Ok(None)).unwrap(),
            FileChange::prepare(replaced.clone(), |path| {
                StagedWrite::new(path, b"1.2.0").map(Some)
            })
            .unwrap(),
            FileChange::prepare(added.clone(), |path| {
                StagedWrite::new(path, b"new").map(Some)
            })
            .unwrap(),
            FileChange::prepare(blocked.clone(), |_| Ok(None)).unwrap(),
        ];
        // 準備が済んだ後に、差し替え先を使えなくする。
        fs::create_dir(&blocked).unwrap();
        let mut placed = PlacedFiles::default();
        assert!(placed.apply(changes).is_err());
        assert_eq!(fs::read(&replaced).unwrap(), b"1.2.0");
        assert!(!removed.exists());
        assert!(added.exists());
        assert!(placed.restore().is_empty());
        assert_eq!(fs::read(removed).unwrap(), b"1.0.0");
        assert_eq!(fs::read(replaced).unwrap(), b"1.1.0");
        assert!(!added.exists());
        assert!(blocked.is_dir());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 3);
    }
}
