//! 新形式の棚を、本番の棚から独立して検証・バックアップ・原本復元する道具。

use piep_lib::library::{
    Fingerprint, LegacyImporter, Library, LibraryId, ResourceReference, ResourceScope,
};
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
        Some("saved-searches") if args.len()==4=>{
            let library=Library::open(Path::new(&args[2]))?;
            let user=LibraryId::parse_str(&args[3])?;
            println!("{}",serde_json::to_string_pretty(&library.saved_searches(user)?)?);
        },
        Some("resources") if args.len()==6=>{
            let library=Library::open(Path::new(&args[2]))?;
            let entity=LibraryId::parse_str(&args[3])?;
            let scope=resource_scope(&args[4],&args[5])?;
            println!("{}",serde_json::to_string_pretty(&library.resource_manifest(entity,scope)?)?);
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
        _=>return Err("使用方法: piep-library init|list|verify <新棚> / suggestion <新棚> <提案UUID> / acquisition <新棚> <取得UUID> / source-state <新棚> <対象UUID> / saved-searches <新棚> <利用者UUID> / resources <新棚> <対象UUID> snapshot|edit <保存状態UUID> / export-resource <新棚> <対象UUID> snapshot|edit <保存状態UUID> body|original|media|file [資産の識別] <新規出力先> / backup <新棚> <新規出力先> / export-original <新棚> <ファイルSHA-256> <新規ファイル> / import-legacy <旧棚> <新棚>".into()),
    }
    Ok(())
}
