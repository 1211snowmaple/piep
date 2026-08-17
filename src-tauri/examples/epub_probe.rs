use piep_lib::epub::builder::EpubBuilder;
use piep_lib::epub::converter;
use piep_lib::epub::intermediate::ImageCompressOptions;
use piep_lib::epub::template::TemplateManager;
use piep_lib::epub::validate;
use std::path::{Path, PathBuf};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 {
        return Err("usage: epub_probe <json> <source> <assets> <template> <output>".into());
    }
    let json_path = Path::new(&args[1]);
    let data: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(json_path).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let manifest = converter::convert_to_manifest(&data, &args[2], Path::new(&args[3]))?;

    let template_root = std::env::temp_dir().join("piep-epub-probe-templates");
    let manager = TemplateManager::new(template_root);
    manager.initialize_defaults()?;
    let contents = manager.load_template_contents(&args[4])?;
    let settings = manager.read_settings(&args[4]);
    let output = PathBuf::from(&args[5]);
    EpubBuilder::new(
        manifest,
        contents,
        settings,
        ImageCompressOptions::default(),
    )
    .build(&output)?;
    let report = validate::validate_epub(&output)?;
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if report.valid {
        Ok(())
    } else {
        Err("internal validation failed".into())
    }
}
