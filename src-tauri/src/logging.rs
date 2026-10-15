//! 記録の行き先。
//!
//! `log::` の呼び出しはコードの中に 200 箇所以上あるのに、**ロガーが一つも
//! 設定されていなかった**。`log` クレートは実装が無いと出力先を持たず、
//! マクロは引数すら評価しない。つまり「取り込みに失敗した」「索引の後始末を
//! 落とした」といった記録は、どこにも残らないまま捨てられていた。
//!
//! 何かが起きたあとに調べられないのは、起きたこと自体より困る。ここでは
//! アプリデータの下の `logs/piep.log` へ書き出す。
//!
//! 外部の実装は足さない。要るのは「行を1本、順番に、落とさずに書く」ことだけ
//! で、そのために依存を増やす理由が無い。
//!
//! **記録のために落ちない。** ファイルへ書けないときは標準エラーへ理由を出し、
//! アプリ本体は続ける。記録障害まで黙って捨てると調査不能になる。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 1本のファイルの上限。超えたら1世代だけ残して切り替える。
///
/// 残すのは2本まで。長く走らせるほど古い行は要らなくなるし、際限なく太る
/// ファイルを利用者のディスクに置きたくない。
const MAX_BYTES: u64 = 8 * 1024 * 1024;

fn report_file_error(action: &str, error: &dyn std::fmt::Display) {
    // log:: を使うと同じロガーへ戻って再帰するため、ここだけは直接stderrへ出す。
    eprintln!("piep logger: {action}: {error}");
}

struct FileLogger {
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl FileLogger {
    fn rotate_if_needed(&self, file: &mut Option<File>) {
        let too_big = match file.as_ref().map(File::metadata).transpose() {
            Ok(metadata) => metadata.is_some_and(|meta| meta.len() >= MAX_BYTES),
            Err(error) => {
                report_file_error("ログサイズを確認できません", &error);
                false
            }
        };
        if !too_big {
            return;
        }
        // 先に閉じてから動かす。Windows は開いたままのファイルを名前変更できない。
        *file = None;
        let previous = self.path.with_extension("log.1");
        if let Err(error) = std::fs::remove_file(&previous) {
            if error.kind() != std::io::ErrorKind::NotFound {
                report_file_error("前回ログを削除できません", &error);
            }
        }
        if let Err(error) = std::fs::rename(&self.path, &previous) {
            report_file_error("ログをローテーションできません", &error);
        }
        *file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(handle) => Some(handle),
            Err(error) => {
                report_file_error("ログファイルを開き直せません", &error);
                None
            }
        };
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // **他所の Debug は書かない。** 索引を作り直すあいだ、tantivy は
        // ファイルを開くたびに1行出す。開発版では記録がそれで埋まり
        // （8.4MB まで膨らんで回転していた）、こちらの記録が読めなくなるうえ、
        // 1行ごとの同期書き込みが作り直しそのものを遅くする。
        //
        // piep が自分で書いたものは全部残す。困ったときに読むのはこちらである。
        if metadata.level() > log::Level::Info && !metadata.target().starts_with("piep") {
            return false;
        }
        true
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} {} {}\n",
            chrono::Utc::now().to_rfc3339(),
            record.level(),
            record.target(),
            record.args()
        );
        let Ok(mut guard) = self.file.lock() else {
            return;
        };
        self.rotate_if_needed(&mut guard);
        if let Some(handle) = guard.as_mut() {
            if let Err(error) = handle.write_all(line.as_bytes()) {
                report_file_error("ログを書き込めません", &error);
                *guard = None;
            }
        }
        // 端末からも読めるようにしておく。`tauri dev` はここを拾う。
        eprint!("{line}");
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(handle) = guard.as_mut() {
                if let Err(error) = handle.flush() {
                    report_file_error("ログをflushできません", &error);
                    *guard = None;
                }
            }
        }
    }
}

static INSTALLED: OnceLock<()> = OnceLock::new();

/// 記録の行き先を決める。二度目以降は何もしない。
///
/// 失敗しても返り値で騒がない。ここで起動を止める理由が無いからである。
pub fn install(app_data: &Path) {
    if INSTALLED.get().is_some() {
        return;
    }
    let dir = app_data.join("logs");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        report_file_error("ログディレクトリを作成できません", &error);
    }
    let path = dir.join("piep.log");
    let file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(handle) => Some(handle),
        Err(error) => {
            report_file_error("ログファイルを開けません", &error);
            None
        }
    };
    let logger = FileLogger {
        path,
        file: Mutex::new(file),
    };
    // `debug` は開発時だけ。配布物で毎行書くと、量のわりに読む理由が無い。
    let level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    if let Err(error) = log::set_boxed_logger(Box::new(logger)) {
        report_file_error("ロガーを登録できません", &error);
        return;
    }
    log::set_max_level(level);
    if INSTALLED.set(()).is_err() {
        eprintln!("piep logger: 初期化状態を記録できません");
    }
    log::info!("piep {} の記録を開始しました", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "piep-logging-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn logger_at(dir: &Path) -> FileLogger {
        let path = dir.join("piep.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        FileLogger {
            path,
            file: Mutex::new(file),
        }
    }

    /// 他所の Debug は書かない。索引の作り直し中、tantivy はファイルを開く
    /// たびに1行出す。記録がそれで埋まると、こちらの記録が読めなくなる。
    /// **piep が自分で書いたものは段階に関わらず残す。**
    #[test]
    fn only_our_own_chatter_survives_at_debug_level() {
        use log::Log;
        let logger = logger_at(&temp_dir("filter"));
        let allowed = |level: log::Level, target: &str| {
            logger.enabled(&log::Metadata::builder().level(level).target(target).build())
        };
        assert!(!allowed(
            log::Level::Debug,
            "tantivy::directory::mmap_directory"
        ));
        assert!(!allowed(log::Level::Trace, "rustls::client::hs"));
        assert!(allowed(log::Level::Debug, "piep::commands::database"));
        assert!(allowed(log::Level::Trace, "piep_lib::database"));
        // 外から来たものでも、警告と誤りは落とさない。
        assert!(allowed(log::Level::Warn, "tantivy::indexer"));
        assert!(allowed(log::Level::Info, "tauri_plugin_updater::updater"));
    }

    /// 記録は残らなければ意味が無い。行の形まで含めて確かめる。
    #[test]
    fn a_record_reaches_the_file() {
        use log::Log;
        let dir = temp_dir("write");
        let logger = logger_at(&dir);
        logger.log(
            &log::Record::builder()
                .args(format_args!("取り込みに失敗しました"))
                .level(log::Level::Warn)
                .target("piep::test")
                .build(),
        );
        logger.flush();

        let written = std::fs::read_to_string(dir.join("piep.log")).unwrap();
        assert!(written.contains("WARN"), "{written}");
        assert!(written.contains("piep::test"), "{written}");
        assert!(written.contains("取り込みに失敗しました"), "{written}");
    }

    /// 際限なく太らせない。切り替えても、直前の1世代は残す。
    #[test]
    fn the_file_is_replaced_once_it_grows_past_the_cap() {
        use log::Log;
        let dir = temp_dir("rotate");
        let path = dir.join("piep.log");
        std::fs::write(&path, vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();
        let logger = logger_at(&dir);

        logger.log(
            &log::Record::builder()
                .args(format_args!("切り替えたあとの1行目"))
                .level(log::Level::Info)
                .target("piep::test")
                .build(),
        );
        logger.flush();

        let current = std::fs::read_to_string(&path).unwrap();
        assert!(current.contains("切り替えたあとの1行目"));
        assert!(
            current.len() < MAX_BYTES as usize,
            "切り替わっていない: {}",
            current.len()
        );
        assert!(dir.join("piep.log.1").exists(), "直前の1世代が残っていない");
    }

    /// 書けない場所を渡されても、そこで落ちない。
    #[test]
    fn a_broken_destination_does_not_take_the_app_down() {
        use log::Log;
        let dir = temp_dir("broken");
        let logger = FileLogger {
            path: dir.join("piep.log"),
            file: Mutex::new(None),
        };
        logger.log(
            &log::Record::builder()
                .args(format_args!("行き先が無くても呼べる"))
                .level(log::Level::Error)
                .target("piep::test")
                .build(),
        );
        logger.flush();
    }
}
