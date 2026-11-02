//! 実際の本棚で、サーバーモードの送信と読み取りを突き合わせる。
//!
//! 本棚を丸ごと複製しないよう、抜き出した作品のファイルだけを送る。本棚の DB は読み取り専用で
//! 開き、書き込まない。作業用フォルダーは終わったら消す。
//!
//! ```text
//! cargo run --example server_real_library_check -- <アプリデータ> <作業用フォルダー> [作品数]
//! ```

use piep_lib::server::http::combined_router;
use piep_lib::server::snapshot::{prepare_for_send, PATH_COLUMNS, PORTABLE_PREFIX};
use piep_lib::server::standalone::{assemble, ServerPaths};
use piep_lib::server::store::{Role, ServerStore};
use piep_lib::server::sync_send::{
    pair_admin, scan_library_files, send_prepared, RemoteServer, SendHandle,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn relative_of(value: &str) -> Option<String> {
    value.strip_prefix(PORTABLE_PREFIX).map(str::to_string)
}

fn collect(conn: &Connection, sql: &str, parameter: i64, into: &mut HashSet<String>) {
    let mut stmt = conn.prepare_cached(sql).expect("prepare");
    let rows = stmt
        .query_map([parameter], |row| row.get::<_, Option<String>>(0))
        .expect("query");
    for row in rows {
        if let Some(relative) = row.expect("row").as_deref().and_then(relative_of) {
            into.insert(relative);
        }
    }
}

/// サーバーの中の誤りは応答に出ないので、画面へ出す。
struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

struct Tally {
    label: &'static str,
    ok: usize,
    failed: usize,
    examples: Vec<String>,
}

impl Tally {
    fn new(label: &'static str) -> Self {
        Self {
            label,
            ok: 0,
            failed: 0,
            examples: Vec::new(),
        }
    }
    fn record(&mut self, ok: bool, example: impl FnOnce() -> String) {
        if ok {
            self.ok += 1;
        } else {
            self.failed += 1;
            if self.examples.len() < 5 {
                self.examples.push(example());
            }
        }
    }
    fn print(&self) {
        println!(
            "{:<12} 通過 {:>6} / 失敗 {:>4}",
            self.label, self.ok, self.failed
        );
        for example in &self.examples {
            println!("    {example}");
        }
    }
}

#[tokio::main]
async fn main() {
    log::set_logger(&StderrLogger).expect("logger");
    log::set_max_level(log::LevelFilter::Warn);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [app_data, scratch, rest @ ..] = args.as_slice() else {
        eprintln!("使い方: server_real_library_check <アプリデータ> <作業用フォルダー> [作品数]");
        std::process::exit(2);
    };
    let sample: i64 = rest
        .first()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200);
    let app_data = Path::new(app_data);
    let scratch = Path::new(scratch);
    std::fs::create_dir_all(scratch).expect("scratch");
    run(app_data, scratch, sample).await;
    if let Err(error) = std::fs::remove_dir_all(scratch) {
        println!("作業用フォルダーを消せません（あとで消してください）: {error}");
    }
}

async fn run(app_data: &Path, scratch: &Path, sample: i64) {
    let started = Instant::now();
    let snapshot = scratch.join("piep-snapshot.db");
    {
        let source =
            Connection::open_with_flags(app_data.join("piep.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("open");
        source
            .busy_timeout(Duration::from_secs(30))
            .expect("timeout");
        source
            .execute("VACUUM INTO ?1", [snapshot.to_string_lossy().as_ref()])
            .expect("snapshot");
    }
    println!(
        "DB の写し: {:.1} 秒・{} MiB",
        started.elapsed().as_secs_f32(),
        std::fs::metadata(&snapshot).unwrap().len() / 1024 / 1024
    );
    let started = Instant::now();
    let report = prepare_for_send(&snapshot, app_data).expect("prepare");
    println!(
        "送る形に直す: {:.1} 秒・直したパス {}・外を指すパス {}・{} MiB",
        started.elapsed().as_secs_f32(),
        report.converted,
        report.outside,
        std::fs::metadata(&snapshot).unwrap().len() / 1024 / 1024
    );

    // 抜き出す作品。`id` の順に均等に取る。
    let conn = Connection::open_with_flags(&snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open copy");
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .unwrap();
    let step = (total / sample.max(1)).max(1);
    let mut works: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT id FROM (SELECT id, ROW_NUMBER() OVER (ORDER BY id) AS n FROM downloads) WHERE n % ?1 = 0")
            .unwrap();
        let rows = stmt.query_map([step], |row| row.get(0)).unwrap();
        rows.map(Result::unwrap).collect()
    };
    // 挿絵のある作品を足す。均等に取るだけでは少ない。
    {
        let mut stmt = conn
            .prepare(
                "SELECT download_id FROM (
                     SELECT download_id, ROW_NUMBER() OVER (ORDER BY download_id) AS n
                     FROM (SELECT DISTINCT download_id FROM assets WHERE mime_type LIKE 'image/%')
                 ) WHERE n % ?1 = 0 LIMIT ?2",
            )
            .unwrap();
        let illustrated: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT download_id) FROM assets WHERE mime_type LIKE 'image/%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let rows = stmt
            .query_map([(illustrated / sample.max(1)).max(1), sample], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        works.extend(rows.map(Result::unwrap));
    }
    works.sort_unstable();
    works.dedup();
    let mut wanted = HashSet::new();
    for id in &works {
        for (table, column) in PATH_COLUMNS {
            let sql = match *table {
                "downloads" => format!("SELECT \"{column}\" FROM downloads WHERE id = ?1"),
                "assets" => format!("SELECT \"{column}\" FROM assets WHERE download_id = ?1"),
                "download_versions" => format!("SELECT \"{column}\" FROM download_versions WHERE download_id = ?1"),
                "people" => format!(
                    "SELECT p.\"{column}\" FROM people p JOIN download_people d ON d.person_source = p.source AND d.person_key = p.source_key WHERE d.download_id = ?1"
                ),
                "series" => format!(
                    "SELECT s.\"{column}\" FROM series s JOIN download_series d ON d.series_source = s.source AND d.series_key = s.source_key WHERE d.download_id = ?1"
                ),
                _ => continue,
            };
            collect(&conn, &sql, *id, &mut wanted);
        }
    }
    // 読み手の API が返すのは画像だけ。動画・音声・PDF の 404 は仕様どおりなので数えない。
    let not_images: HashSet<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM assets WHERE mime_type IS NULL OR mime_type NOT LIKE 'image/%'",
            )
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0)).unwrap();
        rows.map(|id| id.unwrap().to_string()).collect()
    };
    drop(conn);
    let started = Instant::now();
    let (all_files, skipped) = scan_library_files(app_data).expect("scan");
    println!(
        "ファイルの一覧: {:.1} 秒・{} 件・名前で飛ばした {skipped}",
        started.elapsed().as_secs_f32(),
        all_files.len()
    );
    let referenced = wanted.len();
    let files: Vec<_> = all_files
        .into_iter()
        .filter(|file| wanted.contains(&file.relative))
        .collect();
    let bytes: u64 = files.iter().map(|file| file.size).sum();
    println!(
        "抜き出し: 作品 {} / {total}・参照 {referenced}・実在 {}・{} MiB",
        works.len(),
        files.len(),
        bytes / 1024 / 1024
    );

    // 本物の HTTP で送る。
    let paths = ServerPaths::new(&scratch.join("server")).expect("paths");
    std::fs::create_dir_all(paths.state()).expect("state");
    let parts = assemble(&paths, None, None).expect("assemble");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let address = listener.local_addr().expect("address");
    let router = combined_router(Arc::clone(&parts.context));
    let handle = axum_server::Handle::new();
    let serving = handle.clone();
    let server = tokio::spawn(async move {
        axum_server::from_tcp(listener)
            .expect("listener")
            .handle(serving)
            .serve(router.into_make_service())
            .await
            .expect("serve");
    });
    let url = format!("http://{address}");
    let remote = RemoteServer {
        url: url.clone(),
        fingerprint: None,
    };
    let store = ServerStore::open(&paths.store()).expect("store");
    let secret = store
        .issue_pairing(Role::Admin, Duration::from_secs(60))
        .expect("pairing");
    let token = pair_admin(&remote, &secret, "real library check")
        .await
        .expect("pair");

    let started = Instant::now();
    let outcome = send_prepared(
        &remote,
        &token,
        &snapshot,
        files.clone(),
        &SendHandle::new(),
    )
    .await
    .expect("send");
    println!(
        "送信と反映: {:.1} 秒・{outcome:?}",
        started.elapsed().as_secs_f32()
    );
    // 索引は抜き出しでは意味がない。本文が足りず、棚の全作品を読みに行くので止める。
    parts.receiver.stop_index_catch_up();

    let started = Instant::now();
    let second = send_prepared(&remote, &token, &snapshot, files, &SendHandle::new())
        .await
        .expect("second send");
    println!(
        "2 回目の送信: {:.1} 秒・変更なし = {}",
        started.elapsed().as_secs_f32(),
        second.unchanged
    );
    assert!(second.unchanged, "同じ本棚を送り直して変更ありになりました");
    parts.receiver.stop_index_catch_up();

    // 抜き出した作品を、読み手の API で読み切る。
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("client");
    let private = [
        app_data.to_string_lossy().replace('\\', "\\\\"),
        scratch.to_string_lossy().replace('\\', "\\\\"),
    ];
    let leaks = |text: &str| private.iter().any(|root| text.contains(root.as_str()));
    let mut metadata = Tally::new("情報");
    let mut pages = Tally::new("ページ");
    let mut covers = Tally::new("表紙");
    let mut assets = Tally::new("画像");
    let mut outlines = Tally::new("目次");
    let mut collections = Tally::new("所属");
    let mut leaked = Tally::new("パスの漏れ");
    let started = Instant::now();
    let asset_pattern = regex::Regex::new(r#"data-asset-id=\\"(\d+)\\""#).unwrap();
    for id in &works {
        let get = |path: String| {
            client
                .get(format!("{url}/api/v1/{path}"))
                .bearer_auth(&token)
                .send()
        };
        let response = get(format!("works/{id}")).await.expect("metadata");
        let ok = response.status().is_success();
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        metadata.record(ok, || format!("{id}: {status}"));
        leaked.record(!leaks(&text), || format!("works/{id}"));

        let response = get(format!("works/{id}/outline")).await.expect("outline");
        let status = response.status();
        outlines.record(status.is_success(), || format!("{id}: {status}"));
        let response = get(format!("works/{id}/collections"))
            .await
            .expect("collections");
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        collections.record(status.is_success(), || format!("{id}: {status}"));
        leaked.record(!leaks(&text), || format!("works/{id}/collections"));

        let response = get(format!("works/{id}/cover")).await.expect("cover");
        let status = response.status();
        // 表紙の無い作品は 404 でよい。500 と 413 だけを失敗に数える。
        covers.record(status.is_success() || status.as_u16() == 404, || {
            format!("{id}: {status}")
        });

        let mut page = 0;
        loop {
            let response = get(format!("works/{id}/pages/{page}")).await.expect("page");
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            pages.record(status.is_success(), || {
                format!(
                    "{id} p{page}: {status} {}",
                    text.chars().take(120).collect::<String>()
                )
            });
            if !status.is_success() {
                break;
            }
            leaked.record(!leaks(&text), || format!("works/{id}/pages/{page}"));
            for found in asset_pattern
                .captures_iter(&text)
                .filter(|found| !not_images.contains(&found[1]))
            {
                let path = format!("works/{id}/assets/{}", &found[1]);
                let response = get(path.clone()).await.expect("asset");
                let status = response.status();
                assets.record(status.is_success(), || format!("{path}: {status}"));
            }
            let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            let count = parsed["pageCount"].as_i64().unwrap_or(1);
            page += 1;
            if page >= count || page >= 5 {
                break;
            }
        }
    }
    println!("読み切り: {:.1} 秒", started.elapsed().as_secs_f32());
    for tally in [
        &metadata,
        &outlines,
        &collections,
        &covers,
        &pages,
        &assets,
        &leaked,
    ] {
        tally.print();
    }

    // 文字なしの一覧と、作者の一覧。
    let search = client
        .post(format!("{url}/api/v1/works/search"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "limit": 30 }))
        .send()
        .await
        .expect("search");
    println!("文字なしの一覧: {}", search.status());
    let people = client
        .get(format!("{url}/api/v1/entities/people?limit=30"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("people");
    println!("作者の一覧: {}", people.status());
    handle.shutdown();
    let _ = server.await;
    drop(store);
    drop(parts);
}
