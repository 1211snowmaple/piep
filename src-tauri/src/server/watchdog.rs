//! 見張り役。本体を子として動かし、落ちたら立ち上げ直し、更新を当てて、だめなら戻す。
//!
//! 機能を持たないのは、自分自身はすぐには置き換えられないからである。

use super::durable::write_atomic;
use super::standalone::ServerPaths;
use super::update::{
    self, current_version, read_version_file, restore_databases, UpdateRecord, EXIT_APPLY_UPDATE,
    EXIT_RESTART, VERSION,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// 本体が起動して応答するまでの猶予。
const READY_LIMIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(250);
/// これだけ動いたら、立ち上げ直しの間隔を戻す。
const STABLE_AFTER: Duration = Duration::from_secs(10 * 60);
const BACKOFF: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(120),
];
const FAILURES_BEFORE_DOWN: u32 = 5;
const DOWN_RETRY: Duration = Duration::from_secs(60 * 60);
const DEFERRED_ROLLBACK_ENV: &str = "PIEP_SERVER_DEFERRED_ROLLBACK";

/// 本体が、起動して自分の口に `hello` を通したあとに書く。
#[derive(Debug, Serialize, Deserialize)]
pub struct ReadyMark {
    pub version: String,
    pub pid: u32,
}

/// 立ち上げ直すまでの間隔を決める。
#[derive(Debug, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// 本体が終わるたびに呼ぶ。次に試すまでの間隔を返す。
    pub fn after_exit(&mut self, ran_for: Duration) -> Duration {
        if ran_for >= STABLE_AFTER {
            self.failures = 0;
        }
        self.failures += 1;
        if self.failures > FAILURES_BEFORE_DOWN {
            return DOWN_RETRY;
        }
        let index = (self.failures as usize - 1).min(BACKOFF.len() - 1);
        BACKOFF[index]
    }

    pub fn is_down(&self) -> bool {
        self.failures >= FAILURES_BEFORE_DOWN
    }
}

fn note(paths: &ServerPaths, message: &str) {
    eprintln!("{message}");
    let line = format!("{} {message}\n", Utc::now().to_rfc3339());
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.watchdog_log())
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 戻しが済むまでは、本体を起動する処理へ進まない。
fn retry_rollback(
    paths: &ServerPaths,
    backoff: &mut Backoff,
    mut rollback: impl FnMut() -> Result<(), String>,
    mut sleep: impl FnMut(Duration),
) {
    while let Err(error) = rollback() {
        let wait = backoff.after_exit(Duration::ZERO);
        note(
            paths,
            &format!(
                "前の版に戻せません: {error}。{} 秒後に再試行します",
                wait.as_secs()
            ),
        );
        sleep(wait);
    }
}

/// 待機中の版へ切り替える。切り替えた版を返す。
fn switch_to_pending(paths: &ServerPaths) -> Result<String, String> {
    let pending =
        read_version_file(&paths.pending_version()).ok_or("更新先の版が書かれていません")?;
    if !paths.version_dir(&pending).join("piep-server.exe").exists() {
        return Err(format!("版 {pending} の実行ファイルがありません"));
    }
    if let Some(current) = current_version(paths) {
        // current を書いた直後に中断しても、戻す先を新しい版で上書きしない。
        if current != pending {
            write_atomic(&paths.previous_version(), current.as_bytes())?;
        }
    }
    write_atomic(&paths.current_version(), pending.as_bytes())?;
    // 起動確認が済むまで残す。見張り役が止まっても、次の起動で確認と戻しを続ける。
    Ok(pending)
}

/// 前の版と DB の写しへ戻し、戻す前の版を飛ばすよう記録する。
fn roll_back(paths: &ServerPaths, version: &str, failure: Option<&str>) -> Result<(), String> {
    let previous = read_version_file(&paths.previous_version()).ok_or("戻す版がありません")?;
    // DB を戻し始めたあとは、新しい版を再び起動してはいけない。current と
    // pending の間で止まっても、次の起動では切り替えより先に戻しを完了する。
    write_atomic(&rollback_mark(paths), version.as_bytes())?;
    restore_databases(paths)?;
    write_atomic(&paths.current_version(), previous.as_bytes())?;
    UpdateRecord::try_edit(paths, |record| {
        record.skip(version);
        if let Some(reason) = failure {
            record.note_failed_update(format!(
                "版 {version} に更新できなかったので、{previous} に戻しました: {reason}"
            ));
        } else {
            record.clear_transient_error();
        }
    })?;
    clear_pending(paths)?;
    fs::remove_file(rollback_mark(paths))
        .map_err(|error| format!("戻しの印を消せません: {error}"))?;
    Ok(())
}

fn rollback_mark(paths: &ServerPaths) -> std::path::PathBuf {
    paths.state().join("rollback-version")
}

fn resume_rollback(paths: &ServerPaths) -> Result<(), String> {
    let mark = rollback_mark(paths);
    if mark
        .try_exists()
        .map_err(|error| format!("戻しの印を確かめられません: {error}"))?
    {
        let version = read_version_file(&mark).ok_or("戻しの印を読めません")?;
        // 手で戻す依頼の印は、巻き戻しが済むまで残っている。
        let requested = requested_rollback_mark(paths)
            .try_exists()
            .map_err(|error| format!("頼まれた戻しの印を確かめられません: {error}"))?;
        let failure = (!requested).then_some("中断された旧版への戻しを再開しました");
        roll_back(paths, &version, failure)?;
    }
    Ok(())
}

fn requested_rollback_mark(paths: &ServerPaths) -> PathBuf {
    paths.state().join("requested-rollback-version")
}

/// 本体は開いているDBを置き換えず、終了後の見張り役へ戻しを頼む。
pub fn request_rollback(paths: &ServerPaths, version: &str) -> Result<(), String> {
    check_rollback_support()?;
    write_requested_rollback(paths, version)
}

pub fn check_rollback_support() -> Result<(), String> {
    check_rollback_capability(std::env::var(DEFERRED_ROLLBACK_ENV).ok().as_deref())
}

fn check_rollback_capability(value: Option<&str>) -> Result<(), String> {
    if value == Some("1") {
        Ok(())
    } else {
        Err("見張り役が以前のバージョンです。サーバーの起動タスクかPCを再起動してから、もう一度戻してください".into())
    }
}

fn write_requested_rollback(paths: &ServerPaths, version: &str) -> Result<(), String> {
    if read_version_file(&paths.previous_version()).as_deref() != Some(version)
        || !paths.version_dir(version).join("piep-server.exe").is_file()
    {
        return Err("戻すバージョンを確かめられません".into());
    }
    write_atomic(&requested_rollback_mark(paths), version.as_bytes())
}

fn resume_requested_rollback(paths: &ServerPaths) -> Result<(), String> {
    let mark = requested_rollback_mark(paths);
    if !mark
        .try_exists()
        .map_err(|error| format!("頼まれた戻しの印を確かめられません: {error}"))?
    {
        return Ok(());
    }
    let target = read_version_file(&mark).ok_or("頼まれた戻しのバージョンを読めません")?;
    let current = current_version(paths).ok_or("動いていたバージョンを読めません")?;
    if current != target {
        if read_version_file(&paths.previous_version()).as_deref() != Some(target.as_str()) {
            return Err("戻すバージョンが変わっています".into());
        }
        roll_back(paths, &current, None)?;
    }
    // currentを戻した直後に中断しても、済んだ戻しで旧版をskippedへ足さない。
    fs::remove_file(mark).map_err(|error| format!("頼まれた戻しの印を消せません: {error}"))
}

fn resume_rollbacks(paths: &ServerPaths, backoff: &mut Backoff, sleep: impl FnMut(Duration)) {
    retry_rollback(
        paths,
        backoff,
        || {
            // 依頼された戻しが途中で失敗した場合も、書かれた復旧の印から先に続ける。
            resume_rollback(paths)?;
            resume_requested_rollback(paths)
        },
        sleep,
    );
}

fn clear_pending(paths: &ServerPaths) -> Result<(), String> {
    match fs::remove_file(paths.pending_version()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("待機中の印を消せません: {error}")),
    }
}

fn resume_pending_update(
    paths: &ServerPaths,
    backoff: &mut Backoff,
) -> Result<Option<String>, String> {
    if !paths.pending_version().exists() {
        return Ok(None);
    }
    match switch_to_pending(paths) {
        Ok(version) => Ok(Some(version)),
        Err(error) => {
            note(paths, &format!("更新できません: {error}"));
            let pending = read_version_file(&paths.pending_version());
            // current を書いた後の中断で、選択済みの exe が壊れたり消えたりしても、
            // pending を消すだけで起動不能な current を残さない。
            if let Some(failed) =
                pending.filter(|version| Some(version) == current_version(paths).as_ref())
            {
                fail_update(paths, &failed, &error, backoff);
            } else {
                clear_pending(paths)?;
            }
            Ok(None)
        }
    }
}

/// 更新が定着したあとの片付け。直前の版だけ残す。
fn settle(paths: &ServerPaths, version: &str) {
    if let Err(error) = clear_pending(paths) {
        note(paths, &error);
    }
    let previous = read_version_file(&paths.previous_version());
    if let Ok(entries) = fs::read_dir(paths.versions()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name != version
                && Some(&name) != previous.as_ref()
                && update::parse_version(&name).is_some()
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    UpdateRecord::edit(paths, |record| {
        record.clear_error();
        record.last_applied_at = Some(Utc::now().to_rfc3339());
    });
}

/// 起動タスクが指す root の exe は固定の入口にする。Windows で実行中の入口を
/// 退避・差し替えず、起動するたびに current の版の見張り役へ委譲する。
fn bootstrap_target(paths: &ServerPaths, executable: &Path) -> Result<Option<PathBuf>, String> {
    let installed = paths.root.join("piep-server.exe");
    let is_bootstrap = fs::canonicalize(executable)
        .and_then(|executable| fs::canonicalize(installed).map(|installed| executable == installed))
        .map_err(|error| format!("起動入口を確かめられません: {error}"))?;
    if !is_bootstrap {
        return Ok(None);
    }
    let version = current_version(paths).ok_or("動かす版が書かれていません")?;
    let target = paths.version_dir(&version).join("piep-server.exe");
    if !target.is_file() {
        return Err(format!("版 {version} の実行ファイルがありません"));
    }
    Ok(Some(target))
}

/// 新版の見張り役そのものが起動できない場合も、固定入口の見張り役で pending の
/// 起動確認・巻き戻しを続ける。0 は利用者が止めた場合なので、そのまま終了する。
fn delegate_bootstrap(paths: &ServerPaths, executable: &Path) -> Option<i32> {
    // 戻し先の旧版は、この復旧用の印を知らないことがある。固定入口が先に
    // 復旧してから動かし、古い見張り役へ未完了の戻しを渡さない。
    if [rollback_mark(paths), requested_rollback_mark(paths)]
        .iter()
        .any(|mark| !matches!(mark.try_exists(), Ok(false)))
    {
        return None;
    }
    let target = match bootstrap_target(paths, executable) {
        Ok(Some(target)) => target,
        Ok(None) => return None,
        Err(error) => {
            note(paths, &format!("固定入口の見張り役で続けます: {error}"));
            return None;
        }
    };
    let delegated = (|| -> Result<i32, String> {
        let job = JobGuard::new();
        launch_failures_without_dialogs();
        let mut child = Command::new(&target)
            // 旧版の serve は root exe を差し替える。専用commandを知らない旧版なら
            // 非0で終わらせ、固定入口の安全な見張り役へ戻す。
            .arg("watchdog")
            .arg("--data")
            .arg(&paths.root)
            .spawn()
            .map_err(|error| format!("{} を起動できません: {error}", target.display()))?;
        job.adopt(&child);
        match child.wait() {
            Ok(status) => Ok(status.code().unwrap_or(1)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(format!("見張り役を待てません: {error}"))
            }
        }
    })();
    match delegated {
        Ok(0) => Some(0),
        result => {
            note(paths, &format!("固定入口の見張り役で続けます: {result:?}"));
            None
        }
    }
}

/// 起動できない exe で Windows に窓を出させず、失敗として返させる。
///
/// 中身が壊れた exe（PE でない物）を Windows は 16 ビットのプログラムとして扱い、
/// 既定では「サポートされていない 16 ビット アプリケーション」の窓を出して、
/// 閉じられるまで起動の呼び出しを返さない。見張り役は誰も見ていない所で動くので、
/// そこで止まると戻しも再起動も進まない。設定はプロセス全体に効き、子も受け継ぐ。
#[cfg(windows)]
pub(crate) fn launch_failures_without_dialogs() {
    use windows::Win32::System::Diagnostics::Debug::{
        GetErrorMode, SetErrorMode, SEM_FAILCRITICALERRORS, THREAD_ERROR_MODE,
    };
    // SAFETY: このプロセスのエラーの扱いの印を足すだけで、ほかの値は変えない。
    unsafe {
        SetErrorMode(THREAD_ERROR_MODE(GetErrorMode()) | SEM_FAILCRITICALERRORS);
    }
}

#[cfg(not(windows))]
pub(crate) fn launch_failures_without_dialogs() {}

/// 本体を子として動かし、見張り役が消えたら一緒に終了するようにする。
fn spawn_body(paths: &ServerPaths, version: &str, job: &JobGuard) -> Result<Child, String> {
    let _ = fs::remove_file(paths.ready_mark());
    let executable = paths.version_dir(version).join("piep-server.exe");
    launch_failures_without_dialogs();
    let child = body_command(paths, version)
        .spawn()
        .map_err(|error| format!("{} を起動できません: {error}", executable.display()))?;
    job.adopt(&child);
    Ok(child)
}

fn body_command(paths: &ServerPaths, version: &str) -> Command {
    let mut command = Command::new(paths.version_dir(version).join("piep-server.exe"));
    command
        .arg("run")
        .arg("--data")
        .arg(&paths.root)
        .env(DEFERRED_ROLLBACK_ENV, "1");
    command
}

/// 本体が準備できたか、終わったかを待つ。
fn wait_until_ready(
    paths: &ServerPaths,
    version: &str,
    child: &mut Child,
    limit: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("起動中に終わりました（{status}）"));
        }
        if let Some(mark) = read_ready_mark(paths) {
            if mark.pid == child.id() && mark.version == version {
                return Ok(());
            }
            if mark.pid == child.id() {
                return Err(format!("別の版 {} として起動しました", mark.version));
            }
        }
        if started.elapsed() >= limit {
            return Err(format!("{} 秒以内に応答しませんでした", limit.as_secs()));
        }
        std::thread::sleep(POLL);
    }
}

fn read_ready_mark(paths: &ServerPaths) -> Option<ReadyMark> {
    let bytes = fs::read(paths.ready_mark()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `piep-server serve` の本体。`install` の形で置かれているときだけ呼ばれる。
pub fn run(paths: &ServerPaths) -> Result<i32, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    if let Some(code) = delegate_bootstrap(paths, &executable) {
        return Ok(code);
    }
    run_watchdog(paths, std::thread::sleep)
}

fn run_watchdog(
    paths: &ServerPaths,
    mut rollback_sleep: impl FnMut(Duration),
) -> Result<i32, String> {
    // 同じ置き場を 2 つの見張り役で見ない。
    let lock = File::create(paths.watchdog_lock())
        .map_err(|error| format!("置き場を確保できません: {error}"))?;
    lock.try_lock()
        .map_err(|_| "この置き場ではすでに piep-server が動いています".to_string())?;
    let job = JobGuard::new();
    let mut backoff = Backoff::default();
    note(paths, &format!("見張り役を始めました（{VERSION}）"));

    resume_rollbacks(paths, &mut backoff, &mut rollback_sleep);
    // 更新の途中で機械が落ちたら、切り替える前の状態から続ける。
    let mut applying = resume_pending_update(paths, &mut backoff)?;

    loop {
        let version = current_version(paths).ok_or("動かす版が書かれていません")?;
        let started = Instant::now();
        let mut child = match spawn_body(paths, &version, &job) {
            Ok(child) => child,
            Err(error) => {
                note(paths, &error);
                if let Some(failed) = applying.take() {
                    fail_update(paths, &failed, &error, &mut backoff);
                    continue;
                }
                std::thread::sleep(backoff.after_exit(started.elapsed()));
                continue;
            }
        };
        if let Some(target) = applying.take() {
            match wait_until_ready(paths, &target, &mut child, READY_LIMIT) {
                Ok(()) => {
                    note(paths, &format!("版 {target} へ更新しました"));
                    settle(paths, &target);
                }
                Err(reason) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    fail_update(paths, &target, &reason, &mut backoff);
                    continue;
                }
            }
        }
        let status = child
            .wait()
            .map_err(|error| format!("本体を待てません: {error}"))?;
        match status.code() {
            Some(0) => {
                note(paths, "本体が止まったので、見張り役も止まります");
                return Ok(0);
            }
            Some(EXIT_RESTART) => {
                resume_rollbacks(paths, &mut backoff, &mut rollback_sleep);
                note(paths, "管理画面から頼まれたので立ち上げ直します");
                backoff = Backoff::default();
            }
            Some(EXIT_APPLY_UPDATE) => match switch_to_pending(paths) {
                Ok(next) => {
                    note(paths, &format!("版 {next} へ切り替えます"));
                    applying = Some(next);
                    backoff = Backoff::default();
                }
                Err(error) => note(paths, &format!("更新できません: {error}")),
            },
            code => {
                let wait = backoff.after_exit(started.elapsed());
                let message = format!(
                    "server_restarted: 本体が終わりました（{code:?}）。{} 秒後に立ち上げ直します",
                    wait.as_secs()
                );
                note(paths, &message);
                if backoff.is_down() {
                    note(paths, "server_down: 続けて起動に失敗しています");
                }
                std::thread::sleep(wait);
            }
        }
    }
}

fn fail_update(paths: &ServerPaths, failed: &str, reason: &str, backoff: &mut Backoff) {
    note(paths, &format!("update_failed: 版 {failed}: {reason}"));
    // 印を書くところで失敗しても、失敗した版と理由を保持したまま戻しをやり直す。
    retry_rollback(
        paths,
        backoff,
        || roll_back(paths, failed, Some(reason)),
        std::thread::sleep,
    );
}

/// 見張り役が強制終了されても本体を残さない。残ると置き場の鍵を持ったままになり、
/// 次の起動が「すでに動いています」で止まる。
#[cfg(windows)]
struct JobGuard(Option<windows::Win32::Foundation::HANDLE>);

#[cfg(windows)]
impl JobGuard {
    fn new() -> Self {
        use windows::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: 名前のないジョブを作り、自分で確保した構造体を渡すだけである。
        unsafe {
            let Ok(job) = CreateJobObjectW(None, None) else {
                return Self(None);
            };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            Self(set.is_ok().then_some(job))
        }
    }

    fn adopt(&self, child: &Child) {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        if let Some(job) = self.0 {
            // SAFETY: どちらのハンドルも、この呼び出しの間は有効である。
            let assigned =
                unsafe { AssignProcessToJobObject(job, HANDLE(child.as_raw_handle().cast())) };
            if let Err(error) = assigned {
                log::warn!("本体をジョブに入れられません: {error}");
            }
        }
    }
}

#[cfg(not(windows))]
struct JobGuard;

#[cfg(not(windows))]
impl JobGuard {
    fn new() -> Self {
        Self
    }
    fn adopt(&self, _child: &Child) {}
}

/// 本体が準備できたことを書く。
pub fn write_ready_mark(paths: &ServerPaths) -> Result<(), String> {
    let mark = ReadyMark {
        version: VERSION.to_string(),
        pid: std::process::id(),
    };
    let bytes = serde_json::to_vec(&mark).map_err(|error| error.to_string())?;
    write_atomic(&paths.ready_mark(), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: u64) -> Duration {
        Duration::from_secs(value)
    }

    #[test]
    fn restarts_back_off_and_recover() {
        let mut backoff = Backoff::default();
        assert_eq!(backoff.after_exit(seconds(1)), seconds(5));
        assert_eq!(backoff.after_exit(seconds(1)), seconds(30));
        assert_eq!(backoff.after_exit(seconds(1)), seconds(120));
        assert_eq!(backoff.after_exit(seconds(1)), seconds(120));
        assert!(!backoff.is_down());
        assert_eq!(backoff.after_exit(seconds(1)), seconds(120));
        assert!(backoff.is_down());
        assert_eq!(backoff.after_exit(seconds(1)), DOWN_RETRY);
        // 10 分動けば数え直す。
        assert_eq!(backoff.after_exit(STABLE_AFTER), seconds(5));
        assert!(!backoff.is_down());
    }

    fn laid_out() -> (crate::server::tests::TestRoot, ServerPaths) {
        let dir = crate::server::tests::TestRoot::created("watchdog");
        let paths = ServerPaths::new(&dir.0).expect("paths");
        fs::create_dir_all(paths.state()).expect("state");
        fs::write(paths.root.join("piep-server.exe"), b"fixed bootstrap").expect("bootstrap");
        for version in ["1.0.0", "1.1.0"] {
            fs::create_dir_all(paths.version_dir(version)).expect("version");
            fs::write(paths.version_dir(version).join("piep-server.exe"), version).expect("exe");
        }
        write_atomic(&paths.current_version(), b"1.0.0").expect("current");
        (dir, paths)
    }

    #[test]
    fn a_pending_version_becomes_current_and_can_be_rolled_back() {
        let (_dir, paths) = laid_out();
        let database = rusqlite::Connection::open(paths.store()).expect("db");
        database
            .execute_batch("CREATE TABLE t(v); INSERT INTO t VALUES (1);")
            .expect("seed");
        update::back_up_databases(&paths).expect("backup");
        database.execute("UPDATE t SET v = 2", []).expect("change");
        drop(database);

        write_atomic(&paths.pending_version(), b"1.1.0").expect("pending");
        assert_eq!(switch_to_pending(&paths).expect("switch"), "1.1.0");
        assert_eq!(current_version(&paths).as_deref(), Some("1.1.0"));
        assert!(paths.pending_version().exists());

        roll_back(&paths, "1.1.0", Some("起動しない")).expect("roll back");
        assert!(!paths.pending_version().exists());
        assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
        let record = UpdateRecord::load(&paths);
        assert_eq!(record.skipped, vec!["1.1.0".to_string()]);
        assert!(record.last_error.expect("error").contains("1.0.0"));
        let database = rusqlite::Connection::open(paths.store()).expect("db");
        let value: i64 = database
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .expect("value");
        assert_eq!(value, 1);
    }

    #[test]
    fn a_pending_version_without_its_program_is_refused() {
        let (_dir, paths) = laid_out();
        write_atomic(&paths.pending_version(), b"9.9.9").expect("pending");
        assert!(switch_to_pending(&paths).is_err());
        assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
    }

    #[test]
    fn an_interrupted_switch_keeps_the_update_pending_until_ready() {
        let (_dir, paths) = laid_out();
        write_atomic(&paths.pending_version(), b"1.1.0").unwrap();
        switch_to_pending(&paths).unwrap();
        assert!(
            paths.pending_version().exists(),
            "起動確認前に更新の印を消さない"
        );
        // ここで見張り役が止まって、同じ切り替えをやり直す。
        switch_to_pending(&paths).unwrap();
        assert_eq!(
            read_version_file(&paths.previous_version()).as_deref(),
            Some("1.0.0")
        );
        roll_back(&paths, "1.1.0", Some("起動に失敗")).unwrap();
        assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
        assert!(!paths.pending_version().exists());
    }

    #[test]
    fn resuming_after_the_current_pointer_was_written_keeps_the_previous_version() {
        let (_dir, paths) = laid_out();
        write_atomic(&paths.pending_version(), b"1.1.0").unwrap();
        write_atomic(&paths.previous_version(), b"1.0.0").unwrap();
        write_atomic(&paths.current_version(), b"1.1.0").unwrap();
        switch_to_pending(&paths).unwrap();
        assert_eq!(
            read_version_file(&paths.previous_version()).as_deref(),
            Some("1.0.0")
        );
        roll_back(&paths, "1.1.0", Some("起動に失敗")).unwrap();
        assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
    }

    #[test]
    fn an_interrupted_rollback_finishes_before_retrying_the_pending_version() {
        for current in ["1.1.0", "1.0.0"] {
            let (_dir, paths) = laid_out();
            let database = rusqlite::Connection::open(paths.store()).unwrap();
            database
                .execute_batch("CREATE TABLE t(v); INSERT INTO t VALUES (1);")
                .unwrap();
            update::back_up_databases(&paths).unwrap();
            database.execute("UPDATE t SET v = 2", []).unwrap();
            drop(database);
            write_atomic(&paths.previous_version(), b"1.0.0").unwrap();
            write_atomic(&paths.pending_version(), b"1.1.0").unwrap();
            write_atomic(&paths.current_version(), current.as_bytes()).unwrap();
            write_atomic(&rollback_mark(&paths), b"1.1.0").unwrap();

            resume_rollback(&paths).unwrap();

            assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
            assert!(!paths.pending_version().exists());
            assert!(!rollback_mark(&paths).exists());
            assert_eq!(UpdateRecord::load(&paths).skipped, ["1.1.0"]);
            let database = rusqlite::Connection::open(paths.store()).unwrap();
            let value: i64 = database
                .query_row("SELECT v FROM t", [], |row| row.get(0))
                .unwrap();
            assert_eq!(value, 1);
        }
    }

    fn block_database_restore(paths: &ServerPaths) {
        let database = rusqlite::Connection::open(paths.store()).unwrap();
        database
            .execute_batch("CREATE TABLE t(v); INSERT INTO t VALUES (1);")
            .unwrap();
        drop(database);
        update::back_up_databases(paths).unwrap();
        fs::remove_file(paths.store()).unwrap();
        // ファイルの場所をディレクトリで塞ぎ、DB の差し替えだけを失敗させる。
        fs::create_dir(paths.store()).unwrap();
        write_atomic(&paths.pending_version(), b"1.1.0").unwrap();
        switch_to_pending(paths).unwrap();
        write_atomic(&rollback_mark(paths), b"1.1.0").unwrap();
    }

    #[test]
    fn a_failed_database_restore_is_logged_and_retried() {
        let (_dir, paths) = laid_out();
        block_database_restore(&paths);
        let mut waits = Vec::new();

        resume_rollbacks(&paths, &mut Backoff::default(), |wait| {
            waits.push(wait);
            assert!(rollback_mark(&paths).exists());
            assert_eq!(current_version(&paths).as_deref(), Some("1.1.0"));
            let log = fs::read_to_string(paths.watchdog_log()).unwrap();
            assert!(log.contains("state.db を戻せません"));
            fs::remove_dir(paths.store()).unwrap();
        });

        assert_eq!(waits, [seconds(5)]);
        assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
        assert!(!paths.pending_version().exists());
        assert!(!rollback_mark(&paths).exists());
        let database = rusqlite::Connection::open(paths.store()).unwrap();
        let value: i64 = database
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 1);
    }

    #[test]
    #[should_panic(expected = "復旧の再試行を3回観察した")]
    fn a_persistently_failed_restore_keeps_the_new_body_from_starting() {
        let (_dir, paths) = laid_out();
        block_database_restore(&paths);
        // spawn_body を通ると、実行ファイルの起動に失敗する場合もこの印を消す。
        write_atomic(&paths.ready_mark(), b"ready").unwrap();
        let mut waits = Vec::new();
        let _ = run_watchdog(&paths, |wait| {
            waits.push(wait);
            assert!(rollback_mark(&paths).exists());
            assert!(paths.pending_version().exists());
            assert!(paths.ready_mark().exists());
            if waits.len() == 3 {
                assert_eq!(waits, [seconds(5), seconds(30), seconds(120)]);
                panic!("復旧の再試行を3回観察した");
            }
        });
    }

    #[test]
    fn a_manual_rollback_is_deferred_until_the_body_exits_and_can_resume() {
        let (_dir, paths) = laid_out();
        let database = rusqlite::Connection::open(paths.store()).unwrap();
        database
            .execute_batch("CREATE TABLE t(v); INSERT INTO t VALUES (1);")
            .unwrap();
        update::back_up_databases(&paths).unwrap();
        database.execute("UPDATE t SET v = 2", []).unwrap();
        write_atomic(&paths.previous_version(), b"1.0.0").unwrap();
        write_atomic(&paths.current_version(), b"1.1.0").unwrap();

        write_requested_rollback(&paths, "1.0.0").unwrap();
        assert_eq!(current_version(&paths).as_deref(), Some("1.1.0"));
        assert_eq!(
            database
                .query_row("SELECT v FROM t", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(delegate_bootstrap(&paths, &paths.root.join("piep-server.exe")).is_none());
        drop(database);

        resume_requested_rollback(&paths).unwrap();
        assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
        assert!(!requested_rollback_mark(&paths).exists());
        assert_eq!(UpdateRecord::load(&paths).skipped, ["1.1.0"]);
        assert!(UpdateRecord::load(&paths).last_error.is_none());
        let database = rusqlite::Connection::open(paths.store()).unwrap();
        assert_eq!(
            database
                .query_row("SELECT v FROM t", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(database);

        // ポインターを書いた直後、依頼の印を消すより前に中断した場合。
        write_atomic(&requested_rollback_mark(&paths), b"1.0.0").unwrap();
        resume_requested_rollback(&paths).unwrap();
        assert_eq!(UpdateRecord::load(&paths).skipped, ["1.1.0"]);
        assert!(!requested_rollback_mark(&paths).exists());
        assert!(write_requested_rollback(&paths, "9.9.9").is_err());
    }

    #[test]
    fn only_watchdogs_that_support_deferred_rollback_advertise_it_to_the_body() {
        let (_dir, paths) = laid_out();
        let command = body_command(&paths, "1.1.0");
        let capability = command
            .get_envs()
            .find(|(name, _)| *name == DEFERRED_ROLLBACK_ENV)
            .and_then(|(_, value)| value.and_then(|value| value.to_str()));
        assert!(check_rollback_capability(capability).is_ok());
        for unsupported in [None, Some(""), Some("0"), Some("2")] {
            assert!(check_rollback_capability(unsupported)
                .unwrap_err()
                .contains("再起動"));
        }
    }

    #[test]
    fn settling_keeps_only_the_current_and_previous_versions() {
        let (_dir, paths) = laid_out();
        fs::create_dir_all(paths.version_dir("0.9.0")).expect("older");
        fs::create_dir_all(paths.versions().join(".incoming")).expect("incoming");
        write_atomic(&paths.previous_version(), b"1.0.0").expect("previous");
        write_atomic(&paths.current_version(), b"1.1.0").expect("current");
        write_atomic(&paths.pending_version(), b"1.1.0").expect("pending");
        settle(&paths, "1.1.0");
        assert!(!paths.pending_version().exists());
        assert_eq!(current_version(&paths).as_deref(), Some("1.1.0"));
        assert!(paths.version_dir("1.1.0").exists());
        assert!(paths.version_dir("1.0.0").exists());
        assert!(!paths.version_dir("0.9.0").exists());
        assert!(paths.root.join("piep-server.exe").exists());
        assert_eq!(
            fs::read(paths.root.join("piep-server.exe")).unwrap(),
            b"fixed bootstrap"
        );
        assert!(UpdateRecord::load(&paths).last_applied_at.is_some());
    }

    #[test]
    fn the_fixed_bootstrap_selects_the_current_watchdog_without_replacing_the_entry() {
        let (_dir, paths) = laid_out();
        let entry = paths.root.join("piep-server.exe");
        assert_eq!(
            bootstrap_target(&paths, &entry).unwrap(),
            Some(paths.version_dir("1.0.0").join("piep-server.exe"))
        );
        write_atomic(&paths.current_version(), b"1.1.0").unwrap();
        // current の更新直後に止まっても、同じ固定入口から新しい見張り役へ進める。
        assert_eq!(
            bootstrap_target(&paths, &entry).unwrap(),
            Some(paths.version_dir("1.1.0").join("piep-server.exe"))
        );
        assert_eq!(
            bootstrap_target(&paths, &paths.version_dir("1.1.0").join("piep-server.exe")).unwrap(),
            None
        );
        assert_eq!(fs::read(entry).unwrap(), b"fixed bootstrap");
    }

    #[test]
    #[cfg(windows)]
    fn settling_does_not_touch_a_running_bootstrap_executable() {
        use std::os::windows::fs::OpenOptionsExt;
        let (_dir, paths) = laid_out();
        let entry = paths.root.join("piep-server.exe");
        // 実行中exeより厳しい共有設定で保持する。更新が入口をrename/上書きしないことを確かめる。
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&entry)
            .unwrap();
        write_atomic(&paths.previous_version(), b"1.0.0").unwrap();
        write_atomic(&paths.current_version(), b"1.1.0").unwrap();
        write_atomic(&paths.pending_version(), b"1.1.0").unwrap();
        settle(&paths, "1.1.0");
        assert!(entry.exists());
        assert!(!paths.root.join("piep-server.new.exe").exists());
        assert!(!paths.root.join("piep-server.old.exe").exists());
        drop(locked);
        assert_eq!(fs::read(entry).unwrap(), b"fixed bootstrap");
    }

    #[test]
    fn an_unlaunchable_pending_watchdog_falls_back_to_the_fixed_entry_and_rolls_back() {
        let (_dir, paths) = laid_out();
        write_atomic(&paths.previous_version(), b"1.0.0").unwrap();
        write_atomic(&paths.current_version(), b"1.1.0").unwrap();
        write_atomic(&paths.pending_version(), b"1.1.0").unwrap();
        // fixture の新exeはダミーなので、実際のプロセス起動が失敗する。
        assert_eq!(
            delegate_bootstrap(&paths, &paths.root.join("piep-server.exe")),
            None
        );
        let mut backoff = Backoff::default();
        let applying = resume_pending_update(&paths, &mut backoff)
            .unwrap()
            .unwrap();
        let reason = spawn_body(&paths, &applying, &JobGuard::new()).unwrap_err();
        fail_update(&paths, &applying, &reason, &mut backoff);
        assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
        assert!(!paths.pending_version().exists());
        assert_eq!(
            fs::read(paths.root.join("piep-server.exe")).unwrap(),
            b"fixed bootstrap"
        );
    }

    /// 壊れた exe を起動しても、見えない窓で止まらずにすぐ失敗が返る。
    /// 中身が PE でない exe を Windows は 16 ビットのプログラムとして扱い、既定では
    /// 「サポートされていない 16 ビット アプリケーション」の窓を出して閉じられるまで待つ。
    /// 見張り役は画面のない所で動くので、待てば更新の戻しも再起動も止まる。
    #[cfg(windows)]
    #[test]
    fn a_corrupt_program_fails_at_once_instead_of_waiting_on_a_dialog() {
        use windows::Win32::System::Diagnostics::Debug::{SetErrorMode, THREAD_ERROR_MODE};
        // 実行役のように、窓を抑える設定を親から受け継いでいない状態にする。
        // nextest は試験ごとに別のプロセスなので、ほかの試験には残らない。
        // SAFETY: このプロセスのエラーの扱いを既定に戻すだけである。
        unsafe { SetErrorMode(THREAD_ERROR_MODE(0)) };
        let (_dir, paths) = laid_out();
        let started = Instant::now();
        assert!(spawn_body(&paths, "1.1.0", &JobGuard::new()).is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_missing_selected_pending_program_is_rolled_back_from_the_fixed_entry() {
        let (_dir, paths) = laid_out();
        write_atomic(&paths.previous_version(), b"1.0.0").unwrap();
        write_atomic(&paths.current_version(), b"1.1.0").unwrap();
        write_atomic(&paths.pending_version(), b"1.1.0").unwrap();
        fs::remove_file(paths.version_dir("1.1.0").join("piep-server.exe")).unwrap();
        assert_eq!(
            delegate_bootstrap(&paths, &paths.root.join("piep-server.exe")),
            None
        );
        assert_eq!(
            resume_pending_update(&paths, &mut Backoff::default()).unwrap(),
            None
        );
        assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
        assert!(!paths.pending_version().exists());
        assert_eq!(UpdateRecord::load(&paths).skipped, vec!["1.1.0"]);
    }
}
