//! 借りたままの読み取りを見張る。
//!
//! 読み取りの接続は借りているあいだ同じ時点を読み続けるので、返されないものが一本でもあると
//! WAL を台帳へ書き戻せず、WAL が際限なく膨らむ。どこで借りたものが返されていないかを
//! 次に起きたときに名指しできるよう、借りた場所・スレッド・時刻を記録し、長く借りたままの
//! ものを一度だけログに出す。借りる・返すの費用はロック一回と時刻の読み取りだけ。
//!
//! 借りた場所が分かるのは `Library::reader` で借りた読み取りだけである。それ以外のもの
//! （生の接続や、ほかのプロセス）が書き戻しを止めても分かるよう、見張りは数分ごとに書き手の
//! 接続で書き戻しを試し（`wal_checkpoint(PASSIVE)`。待たずにできる分だけ書き戻す）、書き戻せない
//! 分が溜まったまま進まなければ、いま借りられている読み取りを全部ログに出す。

use rusqlite::Connection;
use std::collections::HashMap;
use std::panic::Location;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// 見張りが見に行く間隔。
const INTERVAL: Duration = Duration::from_secs(30);
/// これ以上借りたままの読み取りをログに出す。
const LONG: Duration = Duration::from_secs(60);
/// 書き戻しを試す間隔。
const WAL_EVERY: Duration = Duration::from_secs(5 * 60);
/// 書き戻せずに残ってよいフレームの数（4 KiB のページで約 256 MiB）。
const WAL_BACKLOG: i64 = 65_536;
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
    /// 書き戻しを試す書き手の接続。棚を閉じれば消える。
    writer: Weak<Mutex<Connection>>,
    wal: WalWatch,
}

/// 棚ごとの、いま借りられている読み取りの一覧。
pub(super) struct Loans {
    table: Arc<Mutex<Table>>,
}

/// 書き戻しの進み具合の見張り。時刻と数だけを受け取り、ログに出すことを返す。
#[derive(Debug, Default)]
pub(crate) struct WalWatch {
    checked: Option<Instant>,
    checkpointed: i64,
    stalled_since: Option<Instant>,
    reported: bool,
}

/// 書き戻しの見張りが伝えること。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WalEvent {
    /// 書き戻せない分が溜まったまま進んでいない。
    Stalled { backlog: i64, minutes: u64 },
    /// 止まっていた書き戻しが戻った。
    Recovered,
}

impl WalWatch {
    /// 次に書き戻しを試すころか。
    fn due(&self, now: Instant) -> bool {
        self.checked
            .is_none_or(|checked| now.duration_since(checked) >= WAL_EVERY)
    }

    /// `wal_checkpoint` の答え（WAL のフレーム数と、書き戻し済みのフレーム数）を受け取る。
    pub(crate) fn observe(
        &mut self,
        now: Instant,
        frames: i64,
        checkpointed: i64,
    ) -> Option<WalEvent> {
        self.checked = Some(now);
        let backlog = frames - checkpointed;
        // 書き手が WAL を頭から使い直すと、書き戻し済みの数は小さくなる。
        let progressed = checkpointed != self.checkpointed;
        self.checkpointed = checkpointed;
        if backlog < WAL_BACKLOG {
            self.stalled_since = None;
            return std::mem::take(&mut self.reported).then_some(WalEvent::Recovered);
        }
        if progressed {
            self.stalled_since = None;
            return None;
        }
        let since = *self.stalled_since.get_or_insert(now);
        let stalled = now.duration_since(since);
        if self.reported || stalled < WAL_STALL {
            return None;
        }
        self.reported = true;
        Some(WalEvent::Stalled {
            backlog,
            minutes: stalled.as_secs() / 60,
        })
    }
}

/// 書き手の接続で、待たずにできる分だけ書き戻す。WAL のフレーム数と書き戻し済みの数を返す。
/// 書き手が使用中なら試さない（保存を待たせない）。
fn checkpoint(writer: &Mutex<Connection>) -> Option<(i64, i64)> {
    let conn = writer.try_lock().ok()?;
    match conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| {
        Ok((r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
    }) {
        // WAL でない（-1）ときは見張らない。
        Ok((frames, checkpointed)) if frames >= 0 => Some((frames, checkpointed)),
        Ok(_) => None,
        Err(error) => {
            log::warn!("台帳の書き戻しを試せません: {error}");
            None
        }
    }
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
        let table = Arc::new(Mutex::new(Table {
            next: 0,
            open: HashMap::new(),
            writer: Arc::downgrade(writer),
            wal: WalWatch::default(),
        }));
        watch(Arc::downgrade(&table));
        Self { table }
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

/// 書き戻しを試し、止まっていれば、いま借りられている読み取りを全部ログに出す。
fn watch_wal(table: &Mutex<Table>) {
    let writer = {
        let table = table.lock().unwrap_or_else(|e| e.into_inner());
        if !table.wal.due(Instant::now()) {
            return;
        }
        table.writer.upgrade()
    };
    let Some(writer) = writer else { return };
    // 書き戻しは長くかかることがあるので、一覧のロックを持たずに行う。
    let Some((frames, checkpointed)) = checkpoint(&writer) else {
        return;
    };
    let mut table = table.lock().unwrap_or_else(|e| e.into_inner());
    match table.wal.observe(Instant::now(), frames, checkpointed) {
        Some(WalEvent::Stalled { backlog, minutes }) => {
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
            log::warn!(
                "台帳の WAL を書き戻せないまま {minutes} 分たちました（未書き戻し {backlog} フレーム）。いま借りられている読み取り: {holders}"
            );
        }
        Some(WalEvent::Recovered) => log::warn!("台帳の WAL の書き戻しが戻りました"),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, minutes: u64) -> Instant {
        start + Duration::from_secs(minutes * 60)
    }

    #[test]
    fn a_backlog_that_does_not_move_is_reported_once_and_its_recovery_too() {
        let start = Instant::now();
        let mut watch = WalWatch::default();
        assert_eq!(watch.observe(at(start, 0), 100_000, 4), None);
        assert_eq!(watch.observe(at(start, 5), 120_000, 4), None);
        assert_eq!(
            watch.observe(at(start, 15), 150_000, 4),
            Some(WalEvent::Stalled {
                backlog: 149_996,
                minutes: 10
            })
        );
        assert_eq!(watch.observe(at(start, 20), 160_000, 4), None);
        // 書き戻しが追いつき、書き手が WAL を頭から使い直した。
        assert_eq!(
            watch.observe(at(start, 25), 200, 200),
            Some(WalEvent::Recovered)
        );
        assert_eq!(watch.observe(at(start, 30), 300, 300), None);
    }

    #[test]
    fn a_large_backlog_that_keeps_draining_is_not_a_stall() {
        let start = Instant::now();
        let mut watch = WalWatch::default();
        for step in 0..10u64 {
            let checkpointed = 1_000 * (step as i64 + 1);
            assert_eq!(
                watch.observe(at(start, 5 * step), 500_000, checkpointed),
                None
            );
        }
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
        let writer = Mutex::new(writer);
        let (frames, pinned) = checkpoint(&writer).unwrap();
        assert!(frames - pinned >= 50, "{frames} {pinned}");
        reader.execute_batch("ROLLBACK").unwrap();
        let (frames, checkpointed) = checkpoint(&writer).unwrap();
        assert_eq!(frames, checkpointed);
        drop(reader);
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
