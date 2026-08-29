//! Rust 側の走査。
//!
//! `syn` で構文木として読む。正規表現にしないのは、コマンドの署名が
//! `Result<Vec<DownloadEntry>, String>` のように入れ子の総称を含み、
//! 括弧の対応を数えないと必ず取りこぼすからである。
//!
//! ただし `generate_handler!` の中身とイベント名だけは構文木にならない
//! （前者はマクロのトークン列、後者は文字列リテラル）ので、そこだけ
//! テキストとして読む。

use crate::model::{Arg, Command, EventEmit, Ret};
use proc_macro2::Span;
use quote::ToTokens;
use std::path::Path;
use syn::{Attribute, Expr, ExprLit, FnArg, Item, Lit, Meta, Pat, PathArguments, ReturnType, Type};

/// Tauri が自分で差し込む引数の型。フロントは渡さないので契約には出さない。
const INJECTED: &[&str] = &[
    "AppHandle",
    "State",
    "Window",
    "WebviewWindow",
    "WebviewWindowBuilder",
    "Channel",
    "Request",
];

/// `commands/` 以下のすべての `#[tauri::command]` を集める。
pub fn scan_commands(commands_dir: &Path, repo_root: &Path) -> anyhow::Result<Vec<Command>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(commands_dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let module = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        if module == "mod" {
            continue;
        }
        let text = std::fs::read_to_string(path)?;
        let file =
            syn::parse_file(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let rel = relative(path, repo_root);
        for item in &file.items {
            let Item::Fn(f) = item else { continue };
            if !has_command_attr(&f.attrs) {
                continue;
            }
            out.push(Command {
                name: f.sig.ident.to_string(),
                module: module.clone(),
                file: rel.clone(),
                line: line_of(f.sig.ident.span()),
                doc: doc_of(&f.attrs),
                is_async: f.sig.asyncness.is_some(),
                args: f.sig.inputs.iter().filter_map(arg_of).collect(),
                returns: ret_of(&f.sig.output),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// `#[tauri::command]` か `#[command]` が付いているか。
fn has_command_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        let segs: Vec<String> = a
            .path()
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        segs == ["tauri", "command"] || segs == ["command"]
    })
}

/// `///` を一つの文字列にまとめる。何も書かれていなければ `None`。
fn doc_of(attrs: &[Attribute]) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for a in attrs {
        if !a.path().is_ident("doc") {
            continue;
        }
        let Meta::NameValue(nv) = &a.meta else {
            continue;
        };
        let Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) = &nv.value
        else {
            continue;
        };
        // `/// text` は `" text"` として届く。行頭の空白を一つだけ落とす。
        let v = s.value();
        lines.push(v.strip_prefix(' ').unwrap_or(&v).to_string());
    }
    let joined = lines.join("\n").trim().to_string();
    (!joined.is_empty()).then_some(joined)
}

/// 引数ひとつを読む。`self` は取らない（コマンドは自由関数のみ）。
fn arg_of(input: &FnArg) -> Option<Arg> {
    let FnArg::Typed(pt) = input else { return None };
    let Pat::Ident(ident) = &*pt.pat else {
        return None;
    };
    let rust_name = ident.ident.to_string();
    let ty = tidy_type(&pt.ty.to_token_stream().to_string());
    Some(Arg {
        js_name: to_camel(&rust_name),
        injected: is_injected(&ty),
        rust_name,
        ty,
    })
}

/// Tauri が注入する型かどうか。参照や寿命を剥がしてから先頭の名前を見る。
fn is_injected(ty: &str) -> bool {
    let bare = ty.trim_start_matches('&').trim();
    let bare = bare.strip_prefix("mut ").unwrap_or(bare);
    // `tauri::State<'_, T>` と `State<'_, T>` の両方に当てる。
    let last = bare.rsplit("::").next().unwrap_or(bare);
    let head: String = last
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    INJECTED.contains(&head.as_str())
}

/// 戻り値を読み、`Result<T, E>` なら成功側と失敗側へ分ける。
fn ret_of(output: &ReturnType) -> Ret {
    let ReturnType::Type(_, ty) = output else {
        return Ret {
            raw: "()".into(),
            ok: None,
            err: None,
        };
    };
    let raw = tidy_type(&ty.to_token_stream().to_string());
    let (ok, err) = split_result(ty);
    Ret { raw, ok, err }
}

/// `Result<T, E>` の中身を取り出す。`Result` でなければ `(None, None)`。
fn split_result(ty: &Type) -> (Option<String>, Option<String>) {
    let Type::Path(p) = ty else {
        return (None, None);
    };
    let Some(last) = p.path.segments.last() else {
        return (None, None);
    };
    if last.ident != "Result" {
        return (None, None);
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return (None, None);
    };
    let mut types = args.args.iter().filter_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(tidy_type(&t.to_token_stream().to_string())),
        _ => None,
    });
    (types.next(), types.next())
}

/// トークン列を人が読める型名へ戻す。
///
/// `to_token_stream().to_string()` は `Vec < i64 >` のように必ず空白を挟む。
/// 表示にしか使わないので、記号の周りの空白を畳むだけでよい。
fn tidy_type(s: &str) -> String {
    let mut out = s.to_string();
    for (from, to) in [
        (" < ", "<"),
        ("< ", "<"),
        (" <", "<"),
        (" > ", ">"),
        (" >", ">"),
        ("> ", ">"),
        (" :: ", "::"),
        (":: ", "::"),
        (" ::", "::"),
        (" ,", ","),
        ("& ", "&"),
        ("( ", "("),
        (" )", ")"),
    ] {
        out = out.replace(from, to);
    }
    // `>` を畳んだ副作用で `Vec<i64>,String` のように詰まるので戻す。
    out = out.replace(",", ", ").replace(",  ", ", ");
    out.trim().to_string()
}

/// snake_case を Tauri 2 が JS 側へ渡すときの camelCase にする。
fn to_camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper = false;
    for c in s.chars() {
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

/// `generate_handler!` に登録された名前を読む。
///
/// マクロの中身は構文木にならないのでテキストとして読む。`[` から
/// 対応する `]` までを切り出し、`commands::モジュール::名前` の最後を取る。
pub fn scan_registered(lib_rs: &Path) -> anyhow::Result<Vec<String>> {
    let text = std::fs::read_to_string(lib_rs)?;
    let Some(start) = text.find("generate_handler!") else {
        anyhow::bail!("generate_handler! が {} に見つからない", lib_rs.display());
    };
    let Some(open) = text[start..].find('[') else {
        anyhow::bail!("generate_handler! の [ が見つからない");
    };
    let open = start + open;
    let mut depth = 0usize;
    let mut close = None;
    for (i, c) in text[open..].char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.ok_or_else(|| anyhow::anyhow!("generate_handler! の ] が見つからない"))?;
    let body = &text[open + 1..close];

    // 区分コメント (`// 認証 (commands::auth)`) を先に落とす。カンマで割ってから
    // 「// で始まる区間」を捨てると、コメントの直後に続く本物の登録まで
    // 一緒に消える。実際それで 9 件を取りこぼしていた。
    let body: String = body
        .lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut names: Vec<String> = body
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.rsplit("::").next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// `.emit("名前"` の送出箇所を集める。
///
/// ファイル全体に対して当てる。行ごとに見てはいけない。この規模の呼び出しは
///
/// ```text
/// let _ = app.emit(
///     "search-index-progress",
///     payload,
/// );
/// ```
///
/// のように改行して書かれており、行単位だと `.emit(` と名前が別の行に落ちて
/// 一件も拾えない。実際それで 7 種類を取りこぼしていた。
pub fn scan_events(src_dir: &Path, repo_root: &Path) -> anyhow::Result<Vec<EventEmit>> {
    let re = regex::Regex::new(r#"\.emit(?:_all)?\s*\(\s*"([^"]+)""#)?;
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(src_dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(path)?;
        let rel = relative(path, repo_root);
        for c in re.captures_iter(&text) {
            let at = c.get(0).unwrap().start();
            out.push(EventEmit {
                name: c[1].to_string(),
                file: rel.clone(),
                // バイト位置から行番号を数える。改行を跨いで一致するため、
                // 一致開始位置までの行数がそのまま送出の行になる。
                line: text[..at].lines().count(),
            });
        }
    }
    out.sort_by(|a, b| (&a.name, &a.file, a.line).cmp(&(&b.name, &b.file, b.line)));
    Ok(out)
}

fn line_of(span: Span) -> usize {
    span.start().line
}

/// リポジトリ根からの相対パスを、区切りを `/` に揃えて返す。
pub fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
