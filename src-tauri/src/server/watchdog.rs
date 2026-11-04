//! 見張り役。本体を子として動かし、落ちたら立ち上げ直し、更新を当てて、だめなら戻す。
//!
//! 機能を持たないのは、自分自身はすぐには置き換えられないからである。

use super::standalone::ServerPaths;
use super::update::{
    self, current_version, read_version_file, restore_databases, write_atomic, UpdateRecord,
    EXIT_APPLY_UPDATE, VERSION,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
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

/// 前の版と DB の写しへ戻し、失敗した版を飛ばすよう記録する。
fn roll_back(paths: &ServerPaths, failed: &str, reason: &str) -> Result<(), String> {
    let previous = read_version_file(&paths.previous_version()).ok_or("戻す版がありません")?;
    restore_databases(paths)?;
    write_atomic(&paths.current_version(), previous.as_bytes())?;
    let mut record = UpdateRecord::load(paths);
    if !record.skipped.iter().any(|version| version == failed) {
        record.skipped.push(failed.to_string());
    }
    record.last_error = Some(format!(
        "版 {failed} に更新できなかったので、{previous} に戻しました: {reason}"
    ));
    record.save(paths);
    clear_pending(paths)?;
    Ok(())
}

fn clear_pending(paths: &ServerPaths) -> Result<(), String> {
    match fs::remove_file(paths.pending_version()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("待機中の印を消せません: {error}")),
    }
}

/// 更新が定着したあとの片付け。直前の版だけ残し、見張り役を次の起動で新しくする。
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
    if let Err(error) = refresh_watchdog(paths, version) {
        note(paths, &format!("見張り役を新しくできません: {error}"));
    }
    let mut record = UpdateRecord::load(paths);
    record.last_error = None;
    record.last_applied_at = Some(Utc::now().to_rfc3339());
    record.save(paths);
}

/// 動いている実行ファイルは上書きできないが、名前は変えられる。効くのは次の起動から。
/// 次の起動の入口なので、無い瞬間を作らない。先に全部写してから名前を入れ替える。
fn refresh_watchdog(paths: &ServerPaths, version: &str) -> Result<(), String> {
    let installed = paths.root.join("piep-server.exe");
    let incoming = paths.root.join("piep-server.new.exe");
    let retired = paths.root.join("piep-server.old.exe");
    fs::copy(
        paths.version_dir(version).join("piep-server.exe"),
        &incoming,
    )
    .map_err(|error| error.to_string())?;
    if retired.exists() {
        let _ = fs::remove_file(&retired);
    }
    if installed.exists() {
        fs::rename(&installed, &retired).map_err(|error| error.to_string())?;
    }
    if let Err(error) = fs::rename(&incoming, &installed) {
        let _ = fs::rename(&retired, &installed);
        return Err(error.to_string());
    }
    Ok(())
}

/// 本体を子として動かす。見張り役が消えたら一緒に消えるようにする。
fn spawn_body(paths: &ServerPaths, version: &str, job: &JobGuard) -> Result<Child, String> {
    let _ = fs::remove_file(paths.ready_mark());
    let executable = paths.version_dir(version).join("piep-server.exe");
    let child = Command::new(&executable)
        .arg("run")
        .arg("--data")
        .arg(&paths.root)
        .spawn()
        .map_err(|error| format!("{} を起動できません: {error}", executable.display()))?;
    job.adopt(&child);
    Ok(child)
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
    // 同じ置き場を 2 つの見張り役で見ない。
    let lock = File::create(paths.watchdog_lock())
        .map_err(|error| format!("置き場を確保できません: {error}"))?;
    lock.try_lock()
        .map_err(|_| "この置き場ではすでに piep-server が動いています".to_string())?;
    let job = JobGuard::new();
    // 前の更新で退かした見張り役。もう動いていないので消せる。
    let _ = fs::remove_file(paths.root.join("piep-server.old.exe"));
    let mut backoff = Backoff::default();
    note(paths, &format!("見張り役を始めました（{VERSION}）"));

    // 更新の途中で機械が落ちたら、切り替える前の状態から続ける。
    let mut applying = if paths.pending_version().exists() {
        match switch_to_pending(paths) {
            Ok(version) => Some(version),
            Err(error) => {
                note(paths, &format!("更新できません: {error}"));
                let _ = fs::remove_file(paths.pending_version());
                None
            }
        }
    } else {
        None
    };

    loop {
        let version = current_version(paths).ok_or("動かす版が書かれていません")?;
        let started = Instant::now();
        let mut child = match spawn_body(paths, &version, &job) {
            Ok(child) => child,
            Err(error) => {
                note(paths, &error);
                if let Some(failed) = applying.take() {
                    fail_update(paths, &failed, &error)?;
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
                    fail_update(paths, &target, &reason)?;
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

fn fail_update(paths: &ServerPaths, failed: &str, reason: &str) -> Result<(), String> {
    note(paths, &format!("update_failed: 版 {failed}: {reason}"));
    roll_back(paths, failed, reason)
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
        fs::create_dir_all(paths.library()).expect("library");
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
        let database = rusqlite::Connection::open(paths.database()).expect("db");
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

        roll_back(&paths, "1.1.0", "起動しない").expect("roll back");
        assert!(!paths.pending_version().exists());
        assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
        let record = UpdateRecord::load(&paths);
        assert_eq!(record.skipped, vec!["1.1.0".to_string()]);
        assert!(record.last_error.expect("error").contains("1.0.0"));
        let database = rusqlite::Connection::open(paths.database()).expect("db");
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
        roll_back(&paths, "1.1.0", "起動に失敗").unwrap();
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
        roll_back(&paths, "1.1.0", "起動に失敗").unwrap();
        assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
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
        assert!(UpdateRecord::load(&paths).last_applied_at.is_some());
    }
}
