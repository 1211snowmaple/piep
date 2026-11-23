//! 借りたままの読み取りを見張る。
//!
//! 読み取りの接続は借りているあいだ同じ時点を読み続けるので、返されないものが一本でもあると
//! WAL を台帳へ書き戻せず、WAL が際限なく膨らむ。どこで借りたものが返されていないかを
//! 次に起きたときに名指しできるよう、借りた場所・スレッド・時刻を記録し、長く借りたままの
//! ものを一度だけログに出す。借りる・返すの費用はロック一回と時刻の読み取りだけ。

use std::collections::HashMap;
use std::panic::Location;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// 見張りが見に行く間隔。
const INTERVAL: Duration = Duration::from_secs(30);
/// これ以上借りたままの読み取りをログに出す。
const LONG: Duration = Duration::from_secs(60);

struct Loan {
    since: Instant,
    place: &'static Location<'static>,
    thread: Thread,
    reported: bool,
}

#[derive(Default)]
struct Table {
    next: u64,
    open: HashMap<u64, Loan>,
}

/// 棚ごとの、いま借りられている読み取りの一覧。
pub(super) struct Loans {
    table: Arc<Mutex<Table>>,
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
    pub(super) fn new() -> Self {
        let table = Arc::new(Mutex::new(Table::default()));
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
            }
        });
    if let Err(error) = spawned {
        log::warn!("読み取りの見張りを始められません: {error}");
    }
}
