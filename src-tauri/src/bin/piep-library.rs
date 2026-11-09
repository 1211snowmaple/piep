//! 新形式の棚を、本番の棚から独立して検証・バックアップ・原本復元する道具。

use piep_lib::library::{
    Fingerprint, LegacyImporter, Library, LibraryId, ResourceReference, ResourceScope,
};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn main() {
    if let Err(error) = run() {
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

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("init") if args.len()==3=>{
            let library=Library::create(Path::new(&args[2]))?;
            println!("{}",serde_json::json!({"library_id":library.id}));
        },
        Some("list") if args.len()==3=>{
            let library=Library::open(Path::new(&args[2]))?;
            println!("{}",serde_json::to_string_pretty(&library.works(1000)?)?);
        },
        Some("verify") if args.len()==3=>{
            let library=Library::open(Path::new(&args[2]))?;
            let unresolved=library.activation_problems()?;
            println!("{}",serde_json::json!({"verified_files":library.verify()?,"storage":library.objects.stats()?,"unresolved_migration":unresolved}));
            if !unresolved.is_empty() {
                return Err("ファイルの検証は完了しましたが、未解決の移行データが残っています".into());
            }
        },
        Some("suggestion") if args.len()==4=>{
            let library=Library::open(Path::new(&args[2]))?;
            let id=LibraryId::parse_str(&args[3])?;
            println!("{}",serde_json::to_string_pretty(&library.collection_suggestion(id)?)?);
        },
        Some("acquisition") if args.len()==4=>{
            let library=Library::open(Path::new(&args[2]))?;
            let id=LibraryId::parse_str(&args[3])?;
            println!("{}",serde_json::to_string_pretty(&library.acquisition_status(id)?)?);
        },
        Some("source-state") if args.len()==4=>{
            let library=Library::open(Path::new(&args[2]))?;
            let id=LibraryId::parse_str(&args[3])?;
            println!("{}",serde_json::to_string_pretty(&library.source_entity_state(id)?)?);
        },
        Some("maintenance") if args.len()==5=>{
            let library=Library::open(Path::new(&args[2]))?;
            let entity=LibraryId::parse_str(&args[3])?;
            let snapshot=LibraryId::parse_str(&args[4])?;
            println!("{}",serde_json::to_string_pretty(&library.maintenance_history(entity,snapshot)?)?);
        },
        Some("saved-searches") if args.len()==4=>{
            let library=Library::open(Path::new(&args[2]))?;
            let user=LibraryId::parse_str(&args[3])?;
            println!("{}",serde_json::to_string_pretty(&library.saved_searches(user)?)?);
        },
        Some("job-history") if args.len()==3 || args.len()==4=>{
            let library=Library::open(Path::new(&args[2]))?;
            let before=args.get(3).map(|s|LibraryId::parse_str(s)).transpose()?;
            println!("{}",serde_json::to_string_pretty(&library.job_history_page(before,100)?)?);
        },
        Some("job-history-items"|"job-history-logs") if args.len()==4 || args.len()==5=>{
            let library=Library::open(Path::new(&args[2]))?;
            let job=LibraryId::parse_str(&args[3])?;
            let after=args.get(4).map(|s|s.parse::<i64>()).transpose()?;
            if args[1]=="job-history-items" {
                println!("{}",serde_json::to_string_pretty(&library.job_history_items(job,after,100)?)?);
            } else {
                println!("{}",serde_json::to_string_pretty(&library.job_history_logs(job,after,100)?)?);
            }
        },
        Some("job-history-payload") if args.len()==5=>{
            let library=Library::open(Path::new(&args[2]))?;
            let job=LibraryId::parse_str(&args[3])?;
            let item=LibraryId::parse_str(&args[4])?;
            println!("{}",serde_json::to_string(&library.job_history_payload(job,item)?)?);
        },
        Some("resources") if args.len()==6=>{
            let library=Library::open(Path::new(&args[2]))?;
            let entity=LibraryId::parse_str(&args[3])?;
            let scope=resource_scope(&args[4],&args[5])?;
            println!("{}",serde_json::to_string_pretty(&library.resource_manifest(entity,scope)?)?);
        },
        Some("reader") if args.len()==6=>{
            let library=Library::open(Path::new(&args[2]))?;
            let entity=LibraryId::parse_str(&args[3])?;
            let scope=resource_scope(&args[4],&args[5])?;
            println!("{}",serde_json::to_string_pretty(&library.reader_document(entity,scope)?)?);
        },
        Some("export-resource") if args.len()==8 || args.len()==9=>{
            let library=Library::open(Path::new(&args[2]))?;
            let entity=LibraryId::parse_str(&args[3])?;
            let scope=resource_scope(&args[4],&args[5])?;
            let (reference,output)=match args[6].as_str() {
                "body" if args.len()==8=>(ResourceReference::Body,&args[7]),
                "original" if args.len()==9=>(ResourceReference::Original {observation:LibraryId::parse_str(&args[7])?},&args[8]),
                "media" if args.len()==9=>(ResourceReference::Media {slot:args[7].clone()},&args[8]),
                "file" if args.len()==9=>(ResourceReference::File {slot:args[7].clone()},&args[8]),
                _=>return Err("body <出力先> / original <取得記録UUID> <出力先> / media|file <資産の識別> <出力先> で指定してください".into()),
            };
            library.export_resource(entity,scope,&reference,Path::new(output))?;
            println!("指定した保存状態からファイルを検証して書き出しました");
        },
        Some("transfer-manifest") if args.len()==7 || args.len()==8=>{
            let library=Library::open(Path::new(&args[2]))?;
            let entity=LibraryId::parse_str(&args[3])?;
            let scope=resource_scope(&args[4],&args[5])?;
            let reference=match args[6].as_str() {
                "body" if args.len()==7=>ResourceReference::Body,
                "original" if args.len()==8=>ResourceReference::Original {observation:LibraryId::parse_str(&args[7])?},
                "media" if args.len()==8=>ResourceReference::Media {slot:args[7].clone()},
                "file" if args.len()==8=>ResourceReference::File {slot:args[7].clone()},
                _=>return Err("body / original <取得記録UUID> / media|file <資産の識別> で指定してください".into()),
            };
            // ワイヤー上の元バイト列を検証できるよう、改行を追加しない。
            std::io::stdout().write_all(&library.resource_transfer_manifest(entity,scope,&reference)?.encode()?)?;
        },
        Some("verify-transfer") if args.len()==5=>{
            use piep_lib::library::{protocol, ObjectStore};
            let library=Library::open(Path::new(&args[2]))?;
            let manifest_path=Path::new(&args[3]);
            piep_storage::regular_file(manifest_path)?;
            let mut bytes=Vec::new();
            std::fs::File::open(manifest_path)?.take(protocol::MAX_MANIFEST_BYTES as u64+1).read_to_end(&mut bytes)?;
            let manifest=protocol::ValidatedTransfer::parse(&bytes)?;
            let target=&manifest.manifest().target;
            if target.library_id.get()!=library.id { return Err("異なる棚の転送記録です".into()); }
            let scope=match target.scope {
                protocol::ResourceScope::Snapshot(id)=>ResourceScope::Snapshot(id.get()),
                protocol::ResourceScope::Edit(id)=>ResourceScope::Edit(id.get()),
            };
            let reference=match &target.resource {
                protocol::ResourceReference::Body=>ResourceReference::Body,
                protocol::ResourceReference::Original {observation}=>ResourceReference::Original {observation:observation.get()},
                protocol::ResourceReference::Media {slot}=>ResourceReference::Media {slot:slot.clone()},
                protocol::ResourceReference::File {slot}=>ResourceReference::File {slot:slot.clone()},
            };
            let current=library.resource_transfer_manifest(target.entity_id.get(),scope,&reference)?;
            if current.file!=manifest.manifest().file { return Err("資産に所属するファイルの記録が一致しません".into()); }
            let receiver=ObjectStore::open(Path::new(&args[4]))?;
            for (ordinal,chunk) in manifest.file().chunks.iter().enumerate() {
                let mut encoded=Vec::new();
                library.export_resource_chunk(target.entity_id.get(),scope,&reference,manifest.file().id,ordinal,&mut encoded)?;
                receiver.import_chunk(chunk,std::io::Cursor::new(encoded))?;
            }
            receiver.verify(manifest.file())?;
            println!("{}",serde_json::json!({"manifest":manifest.id().to_string(),"file":manifest.file().id.to_string(),"length":manifest.file().length.to_string(),"verified":true}));
        },
        Some("backup") if args.len()==4=>{
            let library=Library::open(Path::new(&args[2]))?;
            library.backup(Path::new(&args[3]))?;
            println!("バックアップを検証して公開しました");
        },
        Some("export-original") if args.len()==5=>{
            let library=Library::open(Path::new(&args[2]))?;
            let file=library.file(Fingerprint::parse(&args[3])?)?;
            library.objects.export_new(&file,Path::new(&args[4]))?;
            println!("元ファイルのバイト列を復元しました");
        },
        Some("import-legacy") if args.len()==4=>{
            let source=LegacyImporter::open(Path::new(&args[2]))?;
            let target_path=Path::new(&args[3]);
            let target=if target_path.join("library.json").try_exists()? { Library::open(target_path)? } else { Library::create(target_path)? };
            let report=source.import(&target,&AtomicBool::new(false),|report|eprintln!("{} 作品・{} 保存状態",report.works,report.snapshots))?;
            println!("{}",serde_json::to_string_pretty(&report)?);
            if !report.unresolved.is_empty() {
                return Err("未解決のデータがあります。この移行先へ本番の棚を切り替えてはいけません".into());
            }
        },
        _=>return Err("使用方法: piep-library init|list|verify <新棚> / suggestion <新棚> <提案UUID> / acquisition <新棚> <取得UUID> / source-state <新棚> <対象UUID> / maintenance <新棚> <作品UUID> <保存状態UUID> / saved-searches <新棚> <利用者UUID> / job-history <新棚> [前のページ末尾のジョブUUID] / job-history-items|job-history-logs <新棚> <ジョブUUID> [前のページ末尾のsequence] / job-history-payload <新棚> <ジョブUUID> <項目UUID> / resources|reader <新棚> <対象UUID> snapshot|edit <保存状態UUID> / export-resource <新棚> <対象UUID> snapshot|edit <保存状態UUID> body|original|media|file [資産の識別] <新規出力先> / transfer-manifest <新棚> <対象UUID> snapshot|edit <保存状態UUID> body|original|media|file [資産の識別] / verify-transfer <新棚> <転送記録JSON> <検証用の実体領域> / backup <新棚> <新規出力先> / export-original <新棚> <ファイルSHA-256> <新規ファイル> / import-legacy <旧棚> <新棚>".into()),
    }
    Ok(())
}
