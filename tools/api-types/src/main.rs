//! 携帯へ渡すデータの型から、画面側の TypeScript の型を生成する。
//!
//! 入力は `crates/piep-mobile-api/src/lib.rs`、出力は `shared/mobileApi.ts`。
//! 型を三か所（サーバー、携帯の Rust、画面）に手で写していたのをやめるためのもの。
//!
//! 知らない書き方に出会ったら、黙って `unknown` にせず止まる。生成物が静かに
//! 緩むと、型を一つにした意味がなくなる。
//!
//! ```text
//! cargo run --manifest-path tools/api-types/Cargo.toml -- --repo .          # 書き出す
//! cargo run --manifest-path tools/api-types/Cargo.toml -- --repo . --check  # 食い違いを調べる
//! ```

use anyhow::{anyhow, bail, ensure, Context, Result};
use std::collections::BTreeSet;
use std::path::PathBuf;
use syn::{Attribute, Fields, GenericArgument, Item, ItemStruct, PathArguments, Type};

const SOURCE: &str = "crates/piep-mobile-api/src/lib.rs";
const OUTPUT: &str = "shared/mobileApi.ts";

fn main() -> Result<()> {
    let mut repo = PathBuf::from(".");
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo" => repo = PathBuf::from(args.next().context("--repo の値がない")?),
            "--check" => check = true,
            other => bail!("知らない引数: {other}"),
        }
    }
    let source = std::fs::read_to_string(repo.join(SOURCE))
        .with_context(|| format!("{SOURCE} を読めない"))?;
    let generated = generate(&source)?;
    let output = repo.join(OUTPUT);
    if check {
        let current = std::fs::read_to_string(&output).unwrap_or_default();
        ensure!(
            current.replace("\r\n", "\n") == generated,
            "{OUTPUT} が {SOURCE} と食い違っている。\n\
             `cargo run --manifest-path tools/api-types/Cargo.toml -- --repo .` で生成し直すこと。"
        );
        eprintln!("{OUTPUT} は最新");
    } else {
        std::fs::write(&output, &generated)?;
        eprintln!("書き出した: {}", output.display());
    }
    Ok(())
}

fn generate(source: &str) -> Result<String> {
    let file = syn::parse_file(source)?;
    let structs: Vec<&ItemStruct> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(item) => Some(item),
            _ => None,
        })
        .collect();
    let known: BTreeSet<String> = structs.iter().map(|item| item.ident.to_string()).collect();
    let mut out = String::from(
        "// 生成物。手で直さない。\n\
         // 元は crates/piep-mobile-api/src/lib.rs。直すときはそちらを直して、\n\
         // `cargo run --manifest-path tools/api-types/Cargo.toml -- --repo .` で生成し直す。\n",
    );
    for item in structs {
        out.push('\n');
        out.push_str(&render_struct(item, &known)?);
    }
    Ok(out)
}

fn render_struct(item: &ItemStruct, known: &BTreeSet<String>) -> Result<String> {
    let name = item.ident.to_string();
    ensure!(
        item.generics.params.is_empty(),
        "{name}: 型引数には対応していない"
    );
    let container = serde_args(&item.attrs);
    ensure!(
        container
            .iter()
            .any(|arg| arg == "rename_all=\"camelCase\""),
        "{name}: #[serde(rename_all = \"camelCase\")] が要る"
    );
    for arg in &container {
        ensure!(
            arg == "rename_all=\"camelCase\"",
            "{name}: 知らない serde の指定: {arg}"
        );
    }
    let Fields::Named(fields) = &item.fields else {
        bail!("{name}: 名前つきの項目を持つ構造体だけに対応している");
    };
    let mut extends = Vec::new();
    let mut body = String::new();
    for field in &fields.named {
        let ident = field.ident.as_ref().expect("named field").to_string();
        let mut optional = false;
        let mut flatten = false;
        for arg in serde_args(&field.attrs) {
            match arg.as_str() {
                // 読むときの補いだけ。送るときは必ず入る。
                "default" => {}
                "flatten" => flatten = true,
                "skip_serializing_if=\"Option::is_none\"" => optional = true,
                other => bail!("{name}.{ident}: 知らない serde の指定: {other}"),
            }
        }
        let mut ty = ts_type(&field.ty, known).with_context(|| format!("{name}.{ident}"))?;
        if optional {
            // 無いときは項目ごと省かれる。null では届かない。
            ty = ty
                .strip_suffix(" | null")
                .with_context(|| format!("{name}.{ident}: 省けるのは Option だけ"))?
                .to_string();
        }
        if flatten {
            ensure!(
                known.contains(&ty),
                "{name}.{ident}: flatten は構造体にだけ使える"
            );
            extends.push(ty);
            continue;
        }
        for line in doc_lines(&field.attrs) {
            body.push_str(&format!("  /** {line} */\n"));
        }
        let mark = if optional { "?" } else { "" };
        body.push_str(&format!("  {}{mark}: {ty};\n", camel_case(&ident)));
    }
    let mut out = String::new();
    for line in doc_lines(&item.attrs) {
        out.push_str(&format!("/** {line} */\n"));
    }
    let extends = if extends.is_empty() {
        String::new()
    } else {
        format!(" extends {}", extends.join(", "))
    };
    out.push_str(&format!("export interface {name}{extends} {{\n{body}}}\n"));
    Ok(out)
}

fn ts_type(ty: &Type, known: &BTreeSet<String>) -> Result<String> {
    let Type::Path(path) = ty else {
        bail!("対応していない型の書き方");
    };
    let segment = path.path.segments.last().context("空の型")?;
    let name = segment.ident.to_string();
    let inner = |known: &BTreeSet<String>| -> Result<String> {
        let PathArguments::AngleBracketed(args) = &segment.arguments else {
            bail!("{name} に型引数がない");
        };
        match args.args.first() {
            Some(GenericArgument::Type(inner)) if args.args.len() == 1 => ts_type(inner, known),
            _ => Err(anyhow!("{name} の型引数を読めない")),
        }
    };
    Ok(match name.as_str() {
        "String" => "string".into(),
        "bool" => "boolean".into(),
        "i64" | "i32" | "u32" | "u64" | "usize" => "number".into(),
        "Option" => format!("{} | null", inner(known)?),
        "Vec" => {
            let item = inner(known)?;
            if item.contains(' ') {
                format!("({item})[]")
            } else {
                format!("{item}[]")
            }
        }
        other if known.contains(other) => other.into(),
        other => bail!("知らない型: {other}"),
    })
}

/// `#[serde(...)]` の中身を、空白を除いた「指定ごと」の文字列で返す。
fn serde_args(attrs: &[Attribute]) -> Vec<String> {
    let mut out = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        if let syn::Meta::List(list) = &attr.meta {
            let text: String = list
                .tokens
                .to_string()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            out.extend(split_top_level(&text));
        }
    }
    out
}

/// 引用符の中のカンマでは切らない。
fn split_top_level(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' if !quoted => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            syn::Meta::NameValue(meta) => match &meta.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(text),
                    ..
                }) => Some(text.value().trim().to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn camel_case(snake: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_fields_options_lists_and_flatten() {
        let ts = generate(
            r#"
            /// 一つの作品。
            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            pub struct Work {
                pub source_id: String,
                pub tags: Vec<String>,
                pub maybe: Vec<Option<i64>>,
                /// 無いこともある。
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub position: Option<i64>,
                #[serde(default)]
                pub has_cover: bool,
            }
            #[serde(rename_all = "camelCase")]
            pub struct Detail {
                #[serde(flatten)]
                pub summary: Work,
                pub members: Vec<Work>,
            }
            "#,
        )
        .unwrap();
        assert!(ts.contains("/** 一つの作品。 */\nexport interface Work {\n  sourceId: string;\n  tags: string[];\n  maybe: (number | null)[];\n  /** 無いこともある。 */\n  position?: number;\n  hasCover: boolean;\n}\n"));
        assert!(ts.contains("export interface Detail extends Work {\n  members: Work[];\n}\n"));
    }

    #[test]
    fn stops_on_anything_it_does_not_understand() {
        let unknown_type = "#[serde(rename_all = \"camelCase\")] pub struct A { pub at: PathBuf }";
        assert!(generate(unknown_type).is_err());
        let unknown_attr =
            "#[serde(rename_all = \"camelCase\")] pub struct A { #[serde(rename = \"x\")] pub a: String }";
        assert!(generate(unknown_attr).is_err());
        let no_rename = "pub struct A { pub source_id: String }";
        assert!(generate(no_rename).is_err());
    }
}
