//! 新形式の棚を、本番の棚から独立して検証・バックアップ・原本復元する道具。

use piep_lib::library::{Fingerprint, Library, LibraryId, ResourceReference, ResourceScope};
use std::io::{Read, Write};
use std::path::Path;

const LIST_LIMIT: usize = 1_000;
const USAGE: &str = concat!(
    "使用方法: piep-library <コマンド> <引数>\n",
    "  init <棚>\n",
    "  list <棚>\n",
    "  verify <棚>\n",
    "  suggestion <棚> <提案UUID>\n",
    "  acquisition <棚> <取得UUID>\n",
    "  source-state <棚> <対象UUID>\n",
    "  saved-searches <棚> <利用者UUID>\n",
    "  resources <棚> <対象UUID> snapshot|edit <保存状態UUID>\n",
    "  reader <棚> <対象UUID> snapshot|edit <保存状態UUID>\n",
    "  export-resource <棚> <対象UUID> snapshot|edit <保存状態UUID> ",
    "(body | original <取得記録UUID> | media|file <資産の識別>) <新規出力先>\n",
    "  transfer-manifest <棚> <対象UUID> snapshot|edit <保存状態UUID> ",
    "(body | original <取得記録UUID> | media|file <資産の識別>)\n",
    "  verify-transfer <棚> <転送記録JSON> <検証用の実体領域>\n",
    "  backup <棚> <新規出力先>\n",
    "  reclaim <棚>\n",
    "  export-original <棚> <ファイルSHA-256> <新規ファイル>\n",
    "  semantic-sync <棚> <モデルの置き場>\n",
    "  app-open <棚>",
);

#[derive(strum::EnumString)]
#[cfg_attr(test, derive(strum::AsRefStr, strum::EnumIter))]
#[strum(serialize_all = "kebab-case")]
enum Command {
    Init,
    List,
    Verify,
    Suggestion,
    Acquisition,
    SourceState,
    SavedSearches,
    Resources,
    Reader,
    ExportResource,
    TransferManifest,
    VerifyTransfer,
    Backup,
    Reclaim,
    ExportOriginal,
    SemanticSync,
    AppOpen,
}

impl Command {
    fn parse(args: &[String]) -> Option<Self> {
        let command: Self = args.get(1)?.parse().ok()?;
        let operands = match command {
            Self::Init | Self::List | Self::Verify | Self::Reclaim | Self::AppOpen => 1..=1,
            Self::Suggestion
            | Self::Acquisition
            | Self::SourceState
            | Self::SavedSearches
            | Self::Backup
            | Self::SemanticSync => 2..=2,
            Self::VerifyTransfer | Self::ExportOriginal => 3..=3,
            Self::Resources | Self::Reader => 4..=4,
            Self::TransferManifest => 5..=6,
            Self::ExportResource => 6..=7,
        };
        operands.contains(&(args.len() - 2)).then_some(command)
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if let Err(error) = run(&args) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn resource_scope(kind: &str, id: &str) -> Result<ResourceScope, Box<dyn std::error::Error>> {
    let id = LibraryId::parse_str(id)?;
    match kind {
        "snapshot" => Ok(ResourceScope::Snapshot(id)),
        "edit" => Ok(ResourceScope::Edit(id)),
        _ => Err("保存状態は snapshot または edit で指定してください".into()),
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let command = Command::parse(args).ok_or(USAGE)?;
    match command {
        Command::Init => {
            let library = Library::create(Path::new(&args[2]))?;
            println!("{}", serde_json::json!({ "library_id": library.id }));
        }
        Command::List => {
            let library = Library::open(Path::new(&args[2]))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&library.works(LIST_LIMIT)?)?
            );
        }
        Command::Verify => {
            let library = Library::open(Path::new(&args[2]))?;
            println!(
                "{}",
                serde_json::json!({
                    "verified_files": library.verify()?,
                    "storage": library.objects.stats()?,
                })
            );
        }
        Command::Suggestion => {
            let library = Library::open(Path::new(&args[2]))?;
            let id = LibraryId::parse_str(&args[3])?;
            println!(
                "{}",
                serde_json::to_string_pretty(&library.collection_suggestion(id)?)?
            );
        }
        Command::Acquisition => {
            let library = Library::open(Path::new(&args[2]))?;
            let id = LibraryId::parse_str(&args[3])?;
            println!(
                "{}",
                serde_json::to_string_pretty(&library.acquisition_status(id)?)?
            );
        }
        Command::SourceState => {
            let library = Library::open(Path::new(&args[2]))?;
            let id = LibraryId::parse_str(&args[3])?;
            println!(
                "{}",
                serde_json::to_string_pretty(&library.source_entity_state(id)?)?
            );
        }
        Command::SavedSearches => {
            let library = Library::open(Path::new(&args[2]))?;
            let user = LibraryId::parse_str(&args[3])?;
            println!(
                "{}",
                serde_json::to_string_pretty(&library.saved_searches(user)?)?
            );
        }
        Command::Resources => {
            let library = Library::open(Path::new(&args[2]))?;
            let entity = LibraryId::parse_str(&args[3])?;
            let scope = resource_scope(&args[4], &args[5])?;
            println!(
                "{}",
                serde_json::to_string_pretty(&library.resource_manifest(entity, scope)?)?
            );
        }
        Command::Reader => {
            let library = Library::open(Path::new(&args[2]))?;
            let entity = LibraryId::parse_str(&args[3])?;
            let scope = resource_scope(&args[4], &args[5])?;
            println!(
                "{}",
                serde_json::to_string_pretty(&library.reader_document(entity, scope)?)?
            );
        }
        Command::ExportResource => {
            let library = Library::open(Path::new(&args[2]))?;
            let entity = LibraryId::parse_str(&args[3])?;
            let scope = resource_scope(&args[4], &args[5])?;
            let (reference, output) = match args[6].as_str() {
                "body" if args.len() == 8 => (ResourceReference::Body, &args[7]),
                "original" if args.len() == 9 => (
                    ResourceReference::Original {
                        observation: LibraryId::parse_str(&args[7])?,
                    },
                    &args[8],
                ),
                "media" if args.len() == 9 => (
                    ResourceReference::Media {
                        slot: args[7].clone(),
                    },
                    &args[8],
                ),
                "file" if args.len() == 9 => (
                    ResourceReference::File {
                        slot: args[7].clone(),
                    },
                    &args[8],
                ),
                _ => {
                    return Err(concat!(
                        "body <出力先> / original <取得記録UUID> <出力先> / ",
                        "media|file <資産の識別> <出力先> で指定してください"
                    )
                    .into())
                }
            };
            library.export_resource(entity, scope, &reference, Path::new(output))?;
            println!("指定した保存状態からファイルを検証して書き出しました");
        }
        Command::TransferManifest => {
            let library = Library::open(Path::new(&args[2]))?;
            let entity = LibraryId::parse_str(&args[3])?;
            let scope = resource_scope(&args[4], &args[5])?;
            let reference = match args[6].as_str() {
                "body" if args.len() == 7 => ResourceReference::Body,
                "original" if args.len() == 8 => ResourceReference::Original {
                    observation: LibraryId::parse_str(&args[7])?,
                },
                "media" if args.len() == 8 => ResourceReference::Media {
                    slot: args[7].clone(),
                },
                "file" if args.len() == 8 => ResourceReference::File {
                    slot: args[7].clone(),
                },
                _ => {
                    return Err(concat!(
                        "body / original <取得記録UUID> / ",
                        "media|file <資産の識別> で指定してください"
                    )
                    .into())
                }
            };
            // ワイヤー上の元バイト列を検証できるよう、改行を追加しない。
            std::io::stdout().write_all(
                &library
                    .resource_transfer_manifest(entity, scope, &reference)?
                    .encode()?,
            )?;
        }
        Command::VerifyTransfer => {
            use piep_lib::library::{protocol, ObjectStore};
            let library = Library::open(Path::new(&args[2]))?;
            let manifest_path = Path::new(&args[3]);
            piep_storage::regular_file(manifest_path)?;
            let mut bytes = Vec::new();
            std::fs::File::open(manifest_path)?
                .take(protocol::MAX_MANIFEST_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            let manifest = protocol::ValidatedTransfer::parse(&bytes)?;
            let target = &manifest.manifest().target;
            if target.library_id.get() != library.id {
                return Err("異なる棚の転送記録です".into());
            }
            let scope = match target.scope {
                protocol::ResourceScope::Snapshot(id) => ResourceScope::Snapshot(id.get()),
                protocol::ResourceScope::Edit(id) => ResourceScope::Edit(id.get()),
            };
            let reference = match &target.resource {
                protocol::ResourceReference::Body => ResourceReference::Body,
                protocol::ResourceReference::Original { observation } => {
                    ResourceReference::Original {
                        observation: observation.get(),
                    }
                }
                protocol::ResourceReference::Media { slot } => {
                    ResourceReference::Media { slot: slot.clone() }
                }
                protocol::ResourceReference::File { slot } => {
                    ResourceReference::File { slot: slot.clone() }
                }
            };
            let current =
                library.resource_transfer_manifest(target.entity_id.get(), scope, &reference)?;
            if current.file != manifest.manifest().file {
                return Err("資産に所属するファイルの記録が一致しません".into());
            }
            let receiver = ObjectStore::open(Path::new(&args[4]))?;
            for (ordinal, chunk) in manifest.file().chunks.iter().enumerate() {
                let mut encoded = Vec::new();
                library.export_resource_chunk(
                    target.entity_id.get(),
                    scope,
                    &reference,
                    manifest.file().id,
                    ordinal,
                    &mut encoded,
                )?;
                receiver.import_chunk(chunk, std::io::Cursor::new(encoded))?;
            }
            receiver.verify(manifest.file())?;
            println!(
                "{}",
                serde_json::json!({
                    "manifest": manifest.id().to_string(),
                    "file": manifest.file().id.to_string(),
                    "length": manifest.file().length.to_string(),
                    "verified": true,
                })
            );
        }
        Command::Reclaim => {
            let library = Library::open(Path::new(&args[2]))?;
            let before = library.objects.stats()?;
            let report = library.reclaim(piep_lib::library::ReclaimOptions::default())?;
            println!(
                "{}",
                serde_json::json!({
                    "before": before,
                    "report": report,
                    "after": library.objects.stats()?,
                })
            );
        }
        Command::Backup => {
            let library = Library::open(Path::new(&args[2]))?;
            library.backup(Path::new(&args[3]))?;
            println!("バックアップを検証して公開しました");
        }
        Command::ExportOriginal => {
            let library = Library::open(Path::new(&args[2]))?;
            let file = library.file(Fingerprint::parse(&args[3])?)?;
            library.objects.export_new(&file, Path::new(&args[4]))?;
            println!("元ファイルのバイト列を復元しました");
        }
        Command::SemanticSync => {
            // 意味検索の索引を同期する。モデルは <モデルの置き場>/models から読む。
            piep_lib::library_search::set_model_cache_dir(Path::new(&args[3]));
            let library = Library::open(Path::new(&args[2]))?;
            let started = std::time::Instant::now();
            let mut last = std::time::Instant::now();
            let report = library.sync_semantic_index(&|| false, &mut |done, total| {
                if last.elapsed().as_secs() >= 30 {
                    eprintln!("{done}/{total}");
                    last = std::time::Instant::now();
                }
            })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "seconds": started.elapsed().as_secs_f64(),
                    "report": format!("{report:?}"),
                    "state": format!("{:?}", library.semantic_index_state()?),
                }))?
            );
        }
        Command::AppOpen => {
            // アプリと同じ手順で棚を開き、索引を同期して、一覧と検索を一度ずつ引く。
            let state = piep_lib::commands::shelf::Shelf::default();
            let started = std::time::Instant::now();
            state.open(Path::new(&args[2]))?;
            state.sync_index(Path::new(&args[2]));
            let opened = started.elapsed().as_secs_f64();
            let status = state.status();
            let library = state.library()?;
            let t = std::time::Instant::now();
            let list = library.search_works(
                &piep_lib::library_search::TextSearch::current(""),
                &piep_lib::library::WorkListQuery::default(),
            )?;
            let list_ms = t.elapsed().as_secs_f64() * 1000.0;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": status,
                    "open_and_sync_seconds": opened,
                    "list_total": list.page.total,
                    "list_items": list.page.items.len(),
                    "list_ms": list_ms,
                }))?
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn usage_matches_dispatched_commands() {
        let help = run(&[]).unwrap_err().to_string();
        let commands = [
            ("init", 1),
            ("list", 1),
            ("verify", 1),
            ("suggestion", 2),
            ("acquisition", 2),
            ("source-state", 2),
            ("saved-searches", 2),
            ("resources", 4),
            ("reader", 4),
            ("export-resource", 6),
            ("transfer-manifest", 5),
            ("verify-transfer", 3),
            ("backup", 2),
            ("reclaim", 1),
            ("export-original", 3),
            ("semantic-sync", 2),
            ("app-open", 1),
        ];
        let documented: Vec<_> = help
            .lines()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        let dispatched: Vec<_> = Command::iter()
            .map(|command| command.as_ref().to_owned())
            .collect();
        assert_eq!(documented, dispatched);
        assert_eq!(documented, commands.map(|(name, _)| name));

        // 棚にできないパスを渡し、実処理に入ったところで止める。
        let path = std::env::temp_dir().join(format!("piep-cli-{}", uuid::Uuid::now_v7()));
        std::fs::write(&path, []).unwrap();
        let results: Vec<_> = commands
            .map(|(name, arguments)| {
                let mut args = vec!["piep-library".to_owned(), name.to_owned()];
                args.resize(arguments + 2, path.to_string_lossy().into_owned());
                (name, run(&args))
            })
            .into();
        std::fs::remove_file(path).unwrap();
        for (name, result) in results {
            assert_ne!(result.unwrap_err().to_string(), help, "{name}");
        }
    }
}
