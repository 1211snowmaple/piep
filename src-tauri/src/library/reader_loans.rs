//! 借りたままの読み取りを見張る。
//!
//! 読み取りの接続は借りているあいだ同じ時点を読み続けるので、返されないものが一本でもあると
//! WAL を台帳へ書き戻せず、WAL が際限なく膨らむ。どこで借りたものが返されていないかを
//! 次に起きたときに名指しできるよう、借りた場所・スレッド・時刻を記録し、長く借りたままの
//! ものを一度だけログに出す。借りる・返すの費用はロック一回と時刻の読み取りだけ。
//!
//! 借りた場所が分かるのは `Library::reader` で借りた読み取りだけである。それ以外のもの
//! （生の接続や、ほかのプロセス）が書き戻しを止めても分かるよう、見張りは数分ごとに WAL の
//! 様子を読み（`wal_checkpoint(NOOP)`。書き戻しはせず、フレーム数と書き戻し済みの数だけを
//! 読む。書き戻しそのものは保存の確定ごとの自動の書き戻しに任せる）、次のどちらかが続けば、
//! いま借りられている読み取りを全部ログに出す。
//!
//! - 書き戻せない分が溜まったまま、書き戻し済みの数が進まない（古い時点を読み続けている）。
//! - 同じ WAL（ヘッダーの salt が同じ）が大きいまま伸び続ける（書き戻しは進むが、読み取りが
//!   途切れず頭から使い直せない）。
//!
//! ログは一覧のロックを放してから書き、WAL のヘッダーも一覧と書き手のロックの外で読む
//! （ファイルの読み書きが詰まっても、保存・借りる・返す側を待たせない）。

use rusqlite::Connection;
use std::collections::HashMap;
use std::io::Read;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// 見張りが見に行く間隔。
const INTERVAL: Duration = Duration::from_secs(30);
/// これ以上借りたままの読み取りをログに出す。
const LONG: Duration = Duration::from_secs(60);
/// WAL の様子を読む間隔。
const WAL_EVERY: Duration = Duration::from_secs(5 * 60);
/// 書き戻せずに残ってよいフレームの数（4 KiB のページで約 256 MiB）。
const WAL_BACKLOG: i64 = 65_536;
/// 使い直されないまま、ここまで伸びてよい WAL のフレーム数（約 1 GiB）。
const WAL_LARGE: i64 = 262_144;
/// 書き戻しが進まないまま、これだけ経ったらログに出す。
const WAL_STALL: Duration = Duration::from_secs(10 * 60);

struct Loan {
    since: Instant,
    place: &'static Location<'static>,
    thread: Thread,
    reported: bool,
}

struct Table {
    next: u64,
    open: HashMap<u64, Loan>,
    /// WAL の様子を読む書き手の接続。棚を閉じるとき（`Loans` を捨てるとき）に外す。
    writer: Weak<Mutex<Connection>>,
    /// WAL のファイル。ヘッダーの salt を読む。
    wal_path: Option<PathBuf>,
    /// 見張りが WAL のファイルを読むあいだ持つ。棚を閉じる側もこれを待つので、閉じたあとに
    /// ファイルを開かない（閉じた直後のフォルダーの名前の変更と当たらない）。
    io: Arc<Mutex<()>>,
    wal: WalWatch,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 棚ごとの、いま借りられている読み取りの一覧。
pub(super) struct Loans {
    table: Arc<Mutex<Table>>,
}

/// 書き戻しの進み具合の見張り。時刻と数だけを受け取り、ログに出すことを返す。
#[derive(Debug, Default)]
pub(crate) struct WalWatch {
    checked: Option<Instant>,
    frames: i64,
    checkpointed: i64,
    generation: Option<u64>,
    stalled_since: Option<Instant>,
    reported: bool,
}

/// 一度に読んだ WAL の様子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WalState {
    /// WAL のフレーム数。
    pub(crate) frames: i64,
    /// そのうち書き戻し済みの数。
    pub(crate) checkpointed: i64,
    /// WAL を頭から使い直すたびに変わる値（ヘッダーの salt）。読めなければ無い。
    pub(crate) generation: Option<u64>,
}

/// 書き戻しの見張りが伝えること。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WalEvent {
    /// 書き戻せない分が溜まったまま進んでいないか、WAL が大きいまま使い直されていない。
    Stalled {
        frames: i64,
        backlog: i64,
        minutes: u64,
    },
    /// 止まっていた書き戻しが戻った。
    Recovered,
}

impl WalWatch {
    /// 次に WAL の様子を読むころか。
    fn due(&self, now: Instant) -> bool {
        self.checked
            .is_none_or(|checked| now.duration_since(checked) >= WAL_EVERY)
    }

    /// 読んだ WAL の様子を受け取る。
    pub(crate) fn observe(&mut self, now: Instant, state: WalState) -> Option<WalEvent> {
        let WalState {
            frames,
            checkpointed,
            generation,
        } = state;
        self.checked = Some(now);
        let backlog = frames - checkpointed;
        // 使い直していれば（salt が変わった・フレーム数が減った）、書き戻しは追いついたことが
        // ある。止まっていた時間はそこで切る。
        let reused = frames < self.frames
            || matches!((generation, self.generation), (Some(seen), Some(before)) if seen != before);
        let progressed = reused || checkpointed != self.checkpointed;
        // 伸び続けているかは、同じ WAL（使い直していない）と確かめられたときだけ言う。
        // 見ていない間に使い直して前より伸びたものや、書き戻し終えて止まっているだけの
        // 大きな WAL は、伸び続けているとは言わない。
        let grew_in_place = !reused
            && generation.is_some()
            && generation == self.generation
            && frames > self.frames;
        self.frames = frames;
        self.checkpointed = checkpointed;
        self.generation = generation;
        let pinned = backlog >= WAL_BACKLOG && !progressed;
        let unbounded = frames >= WAL_LARGE && grew_in_place;
        if !pinned && !unbounded {
            self.stalled_since = None;
            return std::mem::take(&mut self.reported).then_some(WalEvent::Recovered);
        }
        let since = *self.stalled_since.get_or_insert(now);
        let stalled = now.duration_since(since);
        if self.reported || stalled < WAL_STALL {
            return None;
        }
        self.reported = true;
        Some(WalEvent::Stalled {
            frames,
            backlog,
            minutes: stalled.as_secs() / 60,
        })
    }
}

/// 書き手の接続で、WAL のフレーム数と書き戻し済みの数を読む。書き戻しはしない（NOOP）ので
/// 一瞬で終わる。書き手が使用中なら読まない（保存を待たせない）。WAL でなければ読まない。
fn wal_counts(writer: &Mutex<Connection>) -> Result<Option<(i64, i64)>, rusqlite::Error> {
    let Ok(conn) = writer.try_lock() else {
        return Ok(None);
    };
    let (frames, checkpointed) = conn.query_row("PRAGMA wal_checkpoint(NOOP)", [], |r| {
        Ok((r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
    })?;
    // WAL でない（-1）ときは見張らない。
    Ok((frames >= 0).then_some((frames, checkpointed)))
}

/// 読んだ数と、その前後で読んだ salt を一つの様子にする。前後の salt が違えば、読むあいだに
/// 使い直したので、どちらの WAL の数か分からない（`generation` を無しにする）。使い直した直後で
/// まだ一つもフレームが無いときは、ヘッダーが前の WAL のままのことがあるので読まない。
fn wal_state(counts: (i64, i64), before: Option<u64>, after: Option<u64>) -> WalState {
    let (frames, checkpointed) = counts;
    WalState {
        frames,
        checkpointed,
        generation: (frames > 0 && before == after).then_some(before).flatten(),
    }
}

/// WAL のヘッダーの salt（16〜24 バイト目）。使い直すたびに書き換わる。
fn wal_generation(path: &Path) -> Option<u64> {
    let mut header = [0u8; 24];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .ok()?;
    Some(u64::from_be_bytes(header[16..24].try_into().ok()?))
}

/// 長く借りたままの読み取り一本の様子。
#[derive(Debug)]
pub(crate) struct LongLoan {
    pub(crate) place: &'static Location<'static>,
    pub(crate) thread: String,
    pub(crate) held: Duration,
}

impl std::fmt::Display for LongLoan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}（スレッド {}、{} 秒）",
            self.place.file(),
            self.place.line(),
            self.thread,
            self.held.as_secs()
        )
    }
}

impl Loans {
    pub(super) fn new(writer: &Arc<Mutex<Connection>>) -> Self {
        let wal_path = lock(writer)
            .path()
            .filter(|path| !path.is_empty())
            .map(|path| PathBuf::from(format!("{path}-wal")));
        let table = Arc::new(Mutex::new(Table {
            next: 0,
            open: HashMap::new(),
            writer: Arc::downgrade(writer),
            wal_path,
            io: Arc::new(Mutex::new(())),
            wal: WalWatch::default(),
        }));
        watch(Arc::downgrade(&table));
        Self { table }
    }

    /// 棚を閉じる。見張りが書き手の接続や WAL のファイルを使っていれば、手放すまで待つ。
    fn close(&self) {
        let io = Arc::clone(&lock(&self.table).io);
        let _io = lock(&io);
        lock(&self.table).writer = Weak::new();
    }

    /// 借りた読み取りを載せ、外すときの番号を返す。
    pub(super) fn lend(&self, place: &'static Location<'static>) -> u64 {
        let loan = Loan {
            since: Instant::now(),
            place,
            thread: std::thread::current(),
            reported: false,
        };
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let id = table.next;
        table.next += 1;
        table.open.insert(id, loan);
        id
    }

    pub(super) fn give_back(&self, id: u64) {
        let returned = self
            .table
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .open
            .remove(&id);
        // 出したものが結局返ったのか、返らないままなのかを見分けられるようにする。
        if let Some(loan) = returned.filter(|loan| loan.reported) {
            log::warn!(
                "長く借りていた台帳の読み取りが返されました: {}",
                describe(&loan)
            );
        }
    }

    /// `longer_than` 以上借りたままで、まだ出していないものを返し、出した印を付ける。
    #[cfg(test)]
    pub(crate) fn newly_long(&self, longer_than: Duration) -> Vec<LongLoan> {
        newly_long(&self.table, longer_than)
    }

    /// いま借りられている読み取りを、出したかどうかに関わらず全部返す。
    #[cfg(test)]
    pub(crate) fn open(&self) -> Vec<LongLoan> {
        let table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        table.open.values().map(describe).collect()
    }
}

impl Drop for Loans {
    fn drop(&mut self) {
        self.close();
    }
}

fn describe(loan: &Loan) -> LongLoan {
    LongLoan {
        place: loan.place,
        thread: match loan.thread.name() {
            Some(name) => name.to_owned(),
            None => format!("{:?}", loan.thread.id()),
        },
        held: loan.since.elapsed(),
    }
}

fn newly_long(table: &Mutex<Table>, longer_than: Duration) -> Vec<LongLoan> {
    let mut table = table.lock().unwrap_or_else(|e| e.into_inner());
    let mut found = Vec::new();
    for loan in table.open.values_mut() {
        if !loan.reported && loan.since.elapsed() >= longer_than {
            loan.reported = true;
            found.push(describe(loan));
        }
    }
    found
}

/// 見張りは処理全体で一本。開いている棚の一覧を順に見て、閉じた棚は外す。
fn watch(table: Weak<Mutex<Table>>) {
    static WATCHED: OnceLock<Mutex<Vec<Weak<Mutex<Table>>>>> = OnceLock::new();
    let mut started = false;
    let watched = WATCHED.get_or_init(|| {
        started = true;
        Mutex::new(Vec::new())
    });
    {
        let mut watched = watched.lock().unwrap_or_else(|e| e.into_inner());
        watched.retain(|table| table.strong_count() > 0);
        watched.push(table);
    }
    if !started {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("piep-reader-watch".into())
        .spawn(move || loop {
            std::thread::sleep(INTERVAL);
            let tables: Vec<_> = {
                let mut watched = watched.lock().unwrap_or_else(|e| e.into_inner());
                watched.retain(|table| table.strong_count() > 0);
                watched.iter().filter_map(Weak::upgrade).collect()
            };
            for table in tables {
                for loan in newly_long(&table, LONG) {
                    log::warn!("台帳の読み取りが長く返されていません: {loan}");
                }
                watch_wal(&table);
            }
        });
    if let Err(error) = spawned {
        log::warn!("読み取りの見張りを始められません: {error}");
    }
}

/// WAL の様子を読み、止まっていれば、いま借りられている読み取りを全部ログに出す。
fn watch_wal(table: &Mutex<Table>) {
    if let Some(message) = wal_report(table) {
        log::warn!("{message}");
    }
}

/// WAL の様子を読み、ログに出すことがあれば文言を返す。
///
/// 数を読むあいだだけ一覧のロックと書き手の接続を持ち、使い終えてから放す。WAL のヘッダーは
/// どちらのロックも持たずに読む。棚を閉じる側（`Loans::close`）は `io` と一覧のロックを待つ
/// ので、閉じたあとまで接続を生かさず、ファイルも開かない。ログはロックを放してから書く
/// （呼び出し元）。
fn wal_report(table: &Mutex<Table>) -> Option<String> {
    let now = Instant::now();
    let (io, path) = {
        let table = lock(table);
        if !table.wal.due(now) {
            return None;
        }
        (Arc::clone(&table.io), table.wal_path.clone())
    };
    let _io = lock(&io);
    // `io` を持ってから、棚がまだ開いているかを確かめる（閉じる側は `io` を持って外す）。
    if lock(table).writer.strong_count() == 0 {
        return None;
    }
    let salt = || path.as_deref().and_then(wal_generation);
    let before = salt();
    let counts = {
        let mut table = lock(table);
        let writer = table.writer.upgrade()?;
        let counts = wal_counts(&writer);
        drop(writer);
        match counts {
            Ok(counts) => counts?,
            Err(error) => {
                // 次の間隔まで出さない。
                table.wal.checked = Some(now);
                return Some(format!("台帳の WAL の様子を読めません: {error}"));
            }
        }
    };
    let state = wal_state(counts, before, salt());
    let mut table = lock(table);
    match table.wal.observe(now, state)? {
        WalEvent::Stalled {
            frames,
            backlog,
            minutes,
        } => {
            let loans: Vec<String> = table
                .open
                .values()
                .map(|loan| describe(loan).to_string())
                .collect();
            let holders = if loans.is_empty() {
                "なし（棚の外の接続か、ほかのプロセスが読んでいる）".to_owned()
            } else {
                loans.join("、")
            };
            Some(format!(
                "台帳の WAL が {minutes} 分のあいだ書き戻しも使い直しもされていません（{frames} フレーム、未書き戻し {backlog}）。いま借りられている読み取り: {holders}"
            ))
        }
        WalEvent::Recovered => Some("台帳の WAL の書き戻しが戻りました".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, minutes: u64) -> Instant {
        start + Duration::from_secs(minutes * 60)
    }

    /// 使い直していない、同じ WAL の様子。
    fn same(frames: i64, checkpointed: i64) -> WalState {
        reused(frames, checkpointed, 1)
    }

    fn reused(frames: i64, checkpointed: i64, generation: u64) -> WalState {
        WalState {
            frames,
            checkpointed,
            generation: Some(generation),
        }
    }

    #[test]
    fn a_backlog_that_does_not_move_is_reported_once_and_its_recovery_too() {
        let start = Instant::now();
        let mut watch = WalWatch::default();
        assert_eq!(watch.observe(at(start, 0), same(100_000, 4)), None);
        assert_eq!(watch.observe(at(start, 5), same(120_000, 4)), None);
        assert_eq!(
            watch.observe(at(start, 15), same(150_000, 4)),
            Some(WalEvent::Stalled {
                frames: 150_000,
                backlog: 149_996,
                minutes: 10
            })
        );
        assert_eq!(watch.observe(at(start, 20), same(160_000, 4)), None);
        // 書き戻しが追いつき、書き手が WAL を頭から使い直した。
        assert_eq!(
            watch.observe(at(start, 25), reused(200, 200, 2)),
            Some(WalEvent::Recovered)
        );
        assert_eq!(watch.observe(at(start, 30), reused(300, 300, 2)), None);
    }

    #[test]
    fn a_large_backlog_that_keeps_draining_is_not_a_stall() {
        let start = Instant::now();
        let mut watch = WalWatch::default();
        for step in 0..10u64 {
            let checkpointed = 1_000 * (step as i64 + 1);
            assert_eq!(
                watch.observe(at(start, 5 * step), same(200_000, checkpointed)),
                None
            );
        }
    }

    /// 書き戻しは追いついていても、読み取りが途切れず WAL を頭から使い直せないと伸び続ける。
    #[test]
    fn a_wal_that_keeps_growing_without_being_reused_is_reported() {
        let start = Instant::now();
        let mut watch = WalWatch::default();
        let mut frames = 0;
        for step in 0..3u64 {
            frames = 300_000 + 10_000 * step as i64;
            assert_eq!(
                watch.observe(at(start, 5 * step), same(frames, frames - 10)),
                None
            );
        }
        frames += 10_000;
        assert_eq!(
            watch.observe(at(start, 15), same(frames, frames - 10)),
            Some(WalEvent::Stalled {
                frames,
                backlog: 10,
                minutes: 10
            })
        );
        // 使い直された。
        assert_eq!(
            watch.observe(at(start, 20), reused(500, 500, 2)),
            Some(WalEvent::Recovered)
        );
    }

    /// 見ていない間に使い直し、前より伸びた WAL は、伸び続けているとは言わない。
    #[test]
    fn a_large_wal_that_is_reused_between_looks_is_not_a_stall() {
        let start = Instant::now();
        let mut watch = WalWatch::default();
        for step in 0..10u64 {
            let frames = 300_000 + 10_000 * step as i64;
            assert_eq!(
                watch.observe(at(start, 5 * step), reused(frames, frames, step)),
                None
            );
        }
    }

    /// 書き戻し終えて書き込みが止まっただけの大きな WAL は、止まっているとは言わない。
    #[test]
    fn a_large_wal_that_is_fully_written_back_and_idle_is_not_a_stall() {
        let start = Instant::now();
        let mut watch = WalWatch::default();
        for step in 0..10u64 {
            assert_eq!(
                watch.observe(at(start, 5 * step), same(400_000, 400_000)),
                None
            );
        }
    }

    /// 使い直したかを確かめられないときは、伸び続けているとは言わない。
    #[test]
    fn growth_is_not_reported_when_the_wal_cannot_be_told_apart() {
        let start = Instant::now();
        let mut watch = WalWatch::default();
        for step in 0..10u64 {
            let frames = 300_000 + 10_000 * step as i64;
            let state = WalState {
                frames,
                checkpointed: frames,
                generation: None,
            };
            assert_eq!(watch.observe(at(start, 5 * step), state), None);
        }
    }

    /// 書き戻し済みの数が同じでも、見ていない間に WAL を使い直していれば、止まっていたとは
    /// 言わない（止まっていた時間はそこで切る）。
    #[test]
    fn a_backlog_in_a_reused_wal_does_not_carry_the_stall_over() {
        let start = Instant::now();
        let mut watch = WalWatch::default();
        assert_eq!(watch.observe(at(start, 0), reused(100_000, 4, 1)), None);
        for step in 1..10u64 {
            assert_eq!(
                watch.observe(at(start, 5 * step), reused(100_000, 4, step + 1)),
                None
            );
        }
    }

    /// 前後で salt が違えば、どちらの WAL の数か分からない。まだフレームの無い WAL の
    /// ヘッダーは前の WAL のままのことがあるので使わない。
    #[test]
    fn the_salt_is_used_only_when_it_holds_still_around_the_counts() {
        assert_eq!(wal_state((10, 5), Some(1), Some(1)).generation, Some(1));
        assert_eq!(wal_state((10, 5), Some(1), Some(2)).generation, None);
        assert_eq!(wal_state((0, 0), Some(1), Some(1)).generation, None);
        assert_eq!(wal_state((10, 5), None, None).generation, None);
    }

    /// 開いたままの読み取りが一本あると、書き戻しはその時点で止まる（SQLite の答えの形の確認）。
    #[test]
    fn an_open_read_pins_the_checkpoint_where_it_began() {
        let dir = std::env::temp_dir().join(format!("piep-wal-watch-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let writer = Connection::open(&path).unwrap();
        writer.pragma_update(None, "journal_mode", "WAL").unwrap();
        writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        writer.execute_batch("CREATE TABLE t(v BLOB)").unwrap();
        let reader = Connection::open(&path).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        for _ in 0..50 {
            writer
                .execute("INSERT INTO t VALUES(zeroblob(8192))", [])
                .unwrap();
        }
        let passive = |writer: &Mutex<Connection>| {
            writer
                .lock()
                .unwrap()
                .execute_batch("PRAGMA wal_checkpoint(PASSIVE)")
                .unwrap()
        };
        let writer = Mutex::new(writer);
        let wal = dir.join("t.db-wal");
        let look = |writer: &Mutex<Connection>| {
            let before = wal_generation(&wal);
            let counts = wal_counts(writer).unwrap().unwrap();
            wal_state(counts, before, wal_generation(&wal))
        };
        passive(&writer);
        let pinned = look(&writer);
        assert!(pinned.frames - pinned.checkpointed >= 50, "{pinned:?}");
        assert!(pinned.generation.is_some());
        reader.execute_batch("ROLLBACK").unwrap();
        // 見るだけで、書き戻しはしない。
        assert_eq!(look(&writer), pinned);
        passive(&writer);
        let drained = look(&writer);
        assert_eq!(drained.frames, drained.checkpointed);
        assert_eq!(drained.generation, pinned.generation);
        // 書き戻し終えたあとの書き込みは WAL を頭から使い直し、ヘッダーの salt が変わる。
        writer
            .lock()
            .unwrap()
            .execute("INSERT INTO t VALUES(zeroblob(8))", [])
            .unwrap();
        let restarted = look(&writer);
        assert!(restarted.frames < drained.frames, "{restarted:?}");
        assert_ne!(restarted.generation, drained.generation);
        drop(reader);
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 棚を閉じたら、見張りは書き手の接続をもう使えない（閉じたあとまで生かさない）。
    #[test]
    fn closing_takes_the_writer_away_from_the_watch() {
        let writer = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        let loans = Loans::new(&writer);
        let table = Arc::downgrade(&loans.table);
        drop(loans);
        assert!(table.upgrade().is_none());
        assert_eq!(Arc::strong_count(&writer), 1);
        let watched = Arc::new(Mutex::new(Table {
            next: 0,
            open: HashMap::new(),
            writer: Arc::downgrade(&writer),
            wal_path: None,
            io: Arc::new(Mutex::new(())),
            wal: WalWatch::default(),
        }));
        let loans = Loans {
            table: watched.clone(),
        };
        drop(loans);
        assert!(watched.lock().unwrap().writer.upgrade().is_none());
    }
}
