//! XHTML / XML 安全化ユーティリティ。
//!
//! EPUB の本文は XHTML — すなわち XML であり、ブラウザの HTML パーサのような
//! 寛容さを一切持たない。エスケープし損ねた `&` ひとつ、閉じ忘れた `<br>` ひとつで
//! 文書全体が解析不能になり、Send to Kindle のような取り込み側は EPUB ごと拒否する。
//!
//! ここでは外部由来（pixiv のキャプション、FANBOX の埋め込み HTML、利用者の編集）の
//! 文字列を、必ず整形式になる XHTML 断片へ正規化する。

use std::collections::HashSet;

// ============================================================
// 文字単位のエスケープ
// ============================================================

/// テキストノード用のエスケープ。
pub fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            ch if is_xml_char(ch) => out.push(ch),
            // XML 1.0 が許さない制御文字。残すと文書全体が解析不能になる。
            _ => {}
        }
    }
    out
}

/// 属性値用のエスケープ。引用符も落とす。
pub fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' | '\r' | '\t' => out.push(' '),
            ch if is_xml_char(ch) => out.push(ch),
            _ => {}
        }
    }
    out
}

/// テキストにも属性値にも使える、XML に必要なだけのエスケープ。
///
/// HTML 向けのエスケープは `/` まで落とすことがあるが、XML では意味を持たず、
/// href が `..&#x2f;style.css` のようになって読みにくいだけになる。
pub fn escape_xml(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            ch if is_xml_char(ch) => out.push(ch),
            _ => {}
        }
    }
    out
}

/// XML 1.0 が文書内に許す文字か。
pub fn is_xml_char(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\r')
        || matches!(ch, ' '..='\u{D7FF}')
        || matches!(ch, '\u{E000}'..='\u{FFFD}')
        || matches!(ch, '\u{10000}'..='\u{10FFFF}')
}

/// XML が許さない文字だけを落とす（マークアップには触れない）。
pub fn strip_invalid_chars(value: &str) -> String {
    value.chars().filter(|ch| is_xml_char(*ch)).collect()
}

/// タグを取り除いて素のテキストにする。`dc:description` のような場所で使う。
pub fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    for token in tokenize(html) {
        match token {
            Token::Text(ref text) => out.push_str(&decode_entities(text)),
            Token::StartTag { ref name, .. } if name == "br" || name == "p" => out.push('\n'),
            Token::EndTag { ref name } if name == "p" || name == "div" => out.push('\n'),
            _ => {}
        }
    }
    // 連続した空白をたたんで一行の要約に向く形にする。
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    strip_invalid_chars(&collapsed)
}

// ============================================================
// 実体参照
// ============================================================

/// XHTML が前提なく使える実体は 5 つだけ。それ以外は数値参照へ書き換える。
const NAMED_ENTITIES: &[(&str, char)] = &[
    ("nbsp", '\u{A0}'),
    ("iexcl", '¡'),
    ("cent", '¢'),
    ("pound", '£'),
    ("curren", '¤'),
    ("yen", '¥'),
    ("brvbar", '¦'),
    ("sect", '§'),
    ("uml", '¨'),
    ("copy", '©'),
    ("ordf", 'ª'),
    ("laquo", '«'),
    ("not", '¬'),
    ("shy", '\u{AD}'),
    ("reg", '®'),
    ("macr", '¯'),
    ("deg", '°'),
    ("plusmn", '±'),
    ("sup2", '²'),
    ("sup3", '³'),
    ("acute", '´'),
    ("micro", 'µ'),
    ("para", '¶'),
    ("middot", '·'),
    ("cedil", '¸'),
    ("sup1", '¹'),
    ("ordm", 'º'),
    ("raquo", '»'),
    ("frac14", '¼'),
    ("frac12", '½'),
    ("frac34", '¾'),
    ("iquest", '¿'),
    ("times", '×'),
    ("divide", '÷'),
    ("Agrave", 'À'),
    ("Aacute", 'Á'),
    ("Acirc", 'Â'),
    ("Atilde", 'Ã'),
    ("Auml", 'Ä'),
    ("Aring", 'Å'),
    ("AElig", 'Æ'),
    ("Ccedil", 'Ç'),
    ("Egrave", 'È'),
    ("Eacute", 'É'),
    ("Ecirc", 'Ê'),
    ("Euml", 'Ë'),
    ("Igrave", 'Ì'),
    ("Iacute", 'Í'),
    ("Icirc", 'Î'),
    ("Iuml", 'Ï'),
    ("Ntilde", 'Ñ'),
    ("Ograve", 'Ò'),
    ("Oacute", 'Ó'),
    ("Ocirc", 'Ô'),
    ("Otilde", 'Õ'),
    ("Ouml", 'Ö'),
    ("Oslash", 'Ø'),
    ("Ugrave", 'Ù'),
    ("Uacute", 'Ú'),
    ("Ucirc", 'Û'),
    ("Uuml", 'Ü'),
    ("Yacute", 'Ý'),
    ("szlig", 'ß'),
    ("agrave", 'à'),
    ("aacute", 'á'),
    ("acirc", 'â'),
    ("atilde", 'ã'),
    ("auml", 'ä'),
    ("aring", 'å'),
    ("aelig", 'æ'),
    ("ccedil", 'ç'),
    ("egrave", 'è'),
    ("eacute", 'é'),
    ("ecirc", 'ê'),
    ("euml", 'ë'),
    ("igrave", 'ì'),
    ("iacute", 'í'),
    ("icirc", 'î'),
    ("iuml", 'ï'),
    ("ntilde", 'ñ'),
    ("ograve", 'ò'),
    ("oacute", 'ó'),
    ("ocirc", 'ô'),
    ("otilde", 'õ'),
    ("ouml", 'ö'),
    ("oslash", 'ø'),
    ("ugrave", 'ù'),
    ("uacute", 'ú'),
    ("ucirc", 'û'),
    ("uuml", 'ü'),
    ("yacute", 'ý'),
    ("yuml", 'ÿ'),
    ("ensp", '\u{2002}'),
    ("emsp", '\u{2003}'),
    ("thinsp", '\u{2009}'),
    ("zwnj", '\u{200C}'),
    ("zwj", '\u{200D}'),
    ("lrm", '\u{200E}'),
    ("rlm", '\u{200F}'),
    ("ndash", '–'),
    ("mdash", '—'),
    ("lsquo", '‘'),
    ("rsquo", '’'),
    ("sbquo", '‚'),
    ("ldquo", '“'),
    ("rdquo", '”'),
    ("bdquo", '„'),
    ("dagger", '†'),
    ("Dagger", '‡'),
    ("bull", '•'),
    ("hellip", '…'),
    ("permil", '‰'),
    ("prime", '′'),
    ("Prime", '″'),
    ("lsaquo", '‹'),
    ("rsaquo", '›'),
    ("oline", '‾'),
    ("frasl", '⁄'),
    ("euro", '€'),
    ("trade", '™'),
    ("larr", '←'),
    ("uarr", '↑'),
    ("rarr", '→'),
    ("darr", '↓'),
    ("harr", '↔'),
    ("crarr", '↵'),
    ("infin", '∞'),
    ("ne", '≠'),
    ("le", '≤'),
    ("ge", '≥'),
    ("radic", '√'),
    ("sum", '∑'),
    ("minus", '−'),
    ("lowast", '∗'),
    ("prop", '∝'),
    ("ang", '∠'),
    ("and", '∧'),
    ("or", '∨'),
    ("cap", '∩'),
    ("cup", '∪'),
    ("int", '∫'),
    ("there4", '∴'),
    ("sim", '∼'),
    ("cong", '≅'),
    ("asymp", '≈'),
    ("equiv", '≡'),
    ("sub", '⊂'),
    ("sup", '⊃'),
    ("nsub", '⊄'),
    ("sube", '⊆'),
    ("supe", '⊇'),
    ("oplus", '⊕'),
    ("otimes", '⊗'),
    ("perp", '⊥'),
    ("sdot", '⋅'),
    ("lceil", '⌈'),
    ("rceil", '⌉'),
    ("lfloor", '⌊'),
    ("rfloor", '⌋'),
    ("loz", '◊'),
    ("spades", '♠'),
    ("clubs", '♣'),
    ("hearts", '♥'),
    ("diams", '♦'),
    ("alpha", 'α'),
    ("beta", 'β'),
    ("gamma", 'γ'),
    ("delta", 'δ'),
    ("epsilon", 'ε'),
    ("theta", 'θ'),
    ("lambda", 'λ'),
    ("mu", 'μ'),
    ("pi", 'π'),
    ("sigma", 'σ'),
    ("tau", 'τ'),
    ("phi", 'φ'),
    ("omega", 'ω'),
    ("Omega", 'Ω'),
    ("Delta", 'Δ'),
    ("Sigma", 'Σ'),
    ("Phi", 'Φ'),
    ("Pi", 'Π'),
    ("Lambda", 'Λ'),
    ("Gamma", 'Γ'),
];

/// `&…;` を解釈して文字に戻す。解釈できないものは `&` そのものとして扱う。
fn decode_entities(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'&' {
            let start = index;
            while index < bytes.len() && bytes[index] != b'&' {
                index += 1;
            }
            out.push_str(&text[start..index]);
            continue;
        }
        match read_entity(text, index) {
            Some((ch, next)) => {
                out.push(ch);
                index = next;
            }
            None => {
                out.push('&');
                index += 1;
            }
        }
    }
    out
}

/// `text[at]` が `&` のとき、そこから始まる実体参照を読む。
fn read_entity(text: &str, at: usize) -> Option<(char, usize)> {
    let rest = &text[at + 1..];
    let end = rest.find(';')?;
    if end == 0 || end > 32 {
        return None;
    }
    let body = &rest[..end];
    let next = at + 1 + end + 1;
    if let Some(digits) = body.strip_prefix('#') {
        let code = match digits.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => digits.parse::<u32>().ok()?,
        };
        let ch = char::from_u32(code)?;
        return is_xml_char(ch).then_some((ch, next));
    }
    match body {
        "amp" => Some(('&', next)),
        "lt" => Some(('<', next)),
        "gt" => Some(('>', next)),
        "quot" => Some(('"', next)),
        "apos" => Some(('\'', next)),
        _ => NAMED_ENTITIES
            .iter()
            .find(|(name, _)| *name == body)
            .map(|(_, ch)| (*ch, next)),
    }
}

/// テキスト断片を、XHTML がそのまま受け取れる形に整える。
///
/// 既存の実体参照は解釈したうえで書き直す。`&nbsp;` のような HTML 由来の名前は
/// XHTML では未定義で、そのまま通すと致命的な解析エラーになるため数値参照にする。
fn requote_text(text: &str) -> String {
    escape_text(&decode_entities(text))
}

fn requote_attr(value: &str) -> String {
    escape_attr(&decode_entities(value))
}

// ============================================================
// 字句解析
// ============================================================

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Text(String),
    Comment(String),
    StartTag {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    EndTag {
        name: String,
    },
}

fn tokenize(html: &str) -> Vec<Token> {
    let chars: Vec<char> = html.chars().collect();
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] != '<' {
            text.push(chars[index]);
            index += 1;
            continue;
        }
        // `<` の直後がタグ名でも `/` でも `!` でもないなら、ただの不等号。
        let next = chars.get(index + 1).copied();
        let starts_markup = matches!(next, Some(ch) if ch.is_ascii_alphabetic() || ch == '/' || ch == '!' || ch == '?');
        if !starts_markup {
            text.push('<');
            index += 1;
            continue;
        }
        if !text.is_empty() {
            tokens.push(Token::Text(std::mem::take(&mut text)));
        }
        if chars[index..].starts_with(&['<', '!', '-', '-']) {
            let (body, next_index) = read_until(&chars, index + 4, "-->");
            tokens.push(Token::Comment(body));
            index = next_index;
            continue;
        }
        if next == Some('!') || next == Some('?') {
            // DOCTYPE や処理命令は断片の中に置けない。読み飛ばす。
            let (_, next_index) = read_until(&chars, index + 2, ">");
            index = next_index;
            continue;
        }
        match read_tag(&chars, index) {
            Some((token, next_index)) => {
                tokens.push(token);
                index = next_index;
            }
            None => {
                text.push('<');
                index += 1;
            }
        }
    }
    if !text.is_empty() {
        tokens.push(Token::Text(text));
    }
    tokens
}

/// `terminator` までを読み、本体と次の位置を返す。見つからなければ末尾まで。
fn read_until(chars: &[char], from: usize, terminator: &str) -> (String, usize) {
    let needle: Vec<char> = terminator.chars().collect();
    let mut index = from;
    while index + needle.len() <= chars.len() {
        if chars[index..index + needle.len()] == needle[..] {
            return (chars[from..index].iter().collect(), index + needle.len());
        }
        index += 1;
    }
    (chars[from.min(chars.len())..].iter().collect(), chars.len())
}

fn read_tag(chars: &[char], at: usize) -> Option<(Token, usize)> {
    let mut index = at + 1;
    let closing = chars.get(index) == Some(&'/');
    if closing {
        index += 1;
    }
    let name_start = index;
    while index < chars.len() && (chars[index].is_ascii_alphanumeric() || chars[index] == '-') {
        index += 1;
    }
    if index == name_start {
        return None;
    }
    let name: String = chars[name_start..index]
        .iter()
        .collect::<String>()
        .to_ascii_lowercase();

    if closing {
        let (_, next) = read_until(chars, index, ">");
        return Some((Token::EndTag { name }, next));
    }

    let mut attrs = Vec::new();
    let mut self_closing = false;
    loop {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        match chars.get(index) {
            None => break,
            Some('>') => {
                index += 1;
                break;
            }
            Some('/') if chars.get(index + 1) == Some(&'>') => {
                self_closing = true;
                index += 2;
                break;
            }
            Some('/') => {
                index += 1;
                continue;
            }
            _ => {}
        }
        let key_start = index;
        while index < chars.len()
            && !chars[index].is_whitespace()
            && !matches!(chars[index], '=' | '>' | '/')
        {
            index += 1;
        }
        if index == key_start {
            index += 1;
            continue;
        }
        let key: String = chars[key_start..index]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase();
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        let mut value = String::new();
        if chars.get(index) == Some(&'=') {
            index += 1;
            while index < chars.len() && chars[index].is_whitespace() {
                index += 1;
            }
            match chars.get(index) {
                Some(quote @ ('"' | '\'')) => {
                    let quote = *quote;
                    index += 1;
                    let start = index;
                    while index < chars.len() && chars[index] != quote {
                        index += 1;
                    }
                    value = chars[start..index].iter().collect();
                    if index < chars.len() {
                        index += 1;
                    }
                }
                _ => {
                    let start = index;
                    while index < chars.len()
                        && !chars[index].is_whitespace()
                        && chars[index] != '>'
                    {
                        index += 1;
                    }
                    value = chars[start..index].iter().collect();
                }
            }
        }
        attrs.push((key, value));
    }
    Some((
        Token::StartTag {
            name,
            attrs,
            self_closing,
        },
        index,
    ))
}

// ============================================================
// 要素と属性の許可表
// ============================================================

/// 終了タグを持たない要素。XHTML では必ず `/>` で閉じる。
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// 中身ごと捨てる要素。EPUB のリーダーでは動かないか、有害。
const DROPPED_ELEMENTS: &[&str] = &[
    "script", "style", "iframe", "object", "embed", "form", "input", "button", "select",
    "textarea", "video", "audio", "canvas", "applet", "frame", "frameset", "noscript",
];

/// そのまま残せる要素。ここに無いものはタグだけ外して中身を残す。
const ALLOWED_ELEMENTS: &[&str] = &[
    "a",
    "abbr",
    "aside",
    "b",
    "bdi",
    "bdo",
    "blockquote",
    "br",
    "caption",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "pre",
    "q",
    "rp",
    "rt",
    "rtc",
    "ruby",
    "s",
    "samp",
    "section",
    "small",
    "span",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "u",
    "ul",
    "var",
    "wbr",
];

/// 段落の中には置けない要素。開いたままの `<p>` は自動的に閉じる。
const BLOCK_ELEMENTS: &[&str] = &[
    "address",
    "aside",
    "blockquote",
    "div",
    "dl",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];

/// どの要素にも置ける属性。
const GLOBAL_ATTRS: &[&str] = &[
    "class",
    "dir",
    "id",
    "lang",
    "title",
    "xml:lang",
    "epub:type",
];

fn allowed_attr(element: &str, attr: &str) -> bool {
    if GLOBAL_ATTRS.contains(&attr) {
        return true;
    }
    match element {
        "a" => matches!(attr, "href"),
        "img" => matches!(attr, "src" | "alt" | "width" | "height"),
        "td" | "th" => matches!(attr, "colspan" | "rowspan" | "headers" | "scope"),
        "col" | "colgroup" => attr == "span",
        "ol" => matches!(attr, "start" | "reversed" | "type"),
        "li" => attr == "value",
        "blockquote" | "q" => attr == "cite",
        "time" => attr == "datetime",
        "del" | "ins" => matches!(attr, "cite" | "datetime"),
        _ => false,
    }
}

// ============================================================
// 正規化
// ============================================================

/// 画像参照をどう扱うか。
pub enum ImageRef {
    /// この href に差し替える（すでにエスケープ前の生の値）。
    Keep(String),
    /// 参照先が無いので `<img>` ごと落とす。
    Drop,
}

/// 外部由来の HTML を整形式 XHTML 断片に正規化する。
pub fn sanitize_fragment(html: &str) -> String {
    sanitize_fragment_with(html, &mut |src| ImageRef::Keep(src.to_string()))
}

/// 画像参照の解決を差し込みながら正規化する。
///
/// EPUB では、本文が指すファイルが実在してマニフェストに載っていなければ検証に落ちる。
/// 解決できない `<img>` はここで落とすことで、壊れた参照を持つ EPUB を作らない。
pub fn sanitize_fragment_with(
    html: &str,
    resolve_image: &mut dyn FnMut(&str) -> ImageRef,
) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut open: Vec<String> = Vec::new();
    // 中身ごと捨てる要素の入れ子の深さ。0 より大きい間は何も書かない。
    let mut suppress = 0usize;
    let mut seen_ids: HashSet<String> = HashSet::new();

    for token in tokenize(html) {
        match token {
            Token::Text(text) => {
                if suppress == 0 {
                    out.push_str(&requote_text(&text));
                }
            }
            Token::Comment(body) => {
                if suppress == 0 {
                    // `--` を含むコメントは XML では不正。潰してから書く。
                    let body = strip_invalid_chars(&body).replace("--", "—");
                    out.push_str("<!--");
                    out.push_str(body.trim_end_matches('-'));
                    out.push_str("-->");
                }
            }
            Token::StartTag {
                name,
                attrs,
                self_closing,
            } => {
                if DROPPED_ELEMENTS.contains(&name.as_str()) {
                    // 空要素として書かれた `<embed/>` は入れ子を作らない。
                    if !self_closing && !VOID_ELEMENTS.contains(&name.as_str()) {
                        suppress += 1;
                    }
                    continue;
                }
                if suppress > 0 {
                    continue;
                }
                if !ALLOWED_ELEMENTS.contains(&name.as_str()) {
                    // 知らない要素はタグだけ外し、中身は文章として残す。
                    continue;
                }
                // 段落の中に段落やリストは置けない。開いている `<p>` を閉じる。
                if BLOCK_ELEMENTS.contains(&name.as_str()) {
                    while matches!(open.last().map(String::as_str), Some("p")) {
                        out.push_str("</p>");
                        open.pop();
                    }
                }
                if name == "li" {
                    while matches!(open.last().map(String::as_str), Some("li")) {
                        out.push_str("</li>");
                        open.pop();
                    }
                }

                let mut rendered_attrs = String::new();
                let mut drop_element = false;
                for (key, value) in &attrs {
                    if !allowed_attr(&name, key) {
                        continue;
                    }
                    let value = match key.as_str() {
                        "id" => {
                            let id = ncname(value);
                            // 同じ id が二つあれば検証エラー。後から来たほうを捨てる。
                            if id.is_empty() || !seen_ids.insert(id.clone()) {
                                continue;
                            }
                            id
                        }
                        "href" => match sanitize_uri(value) {
                            Some(uri) => uri,
                            None => continue,
                        },
                        "src" if name == "img" => match resolve_image(value) {
                            ImageRef::Keep(next) => next,
                            ImageRef::Drop => {
                                drop_element = true;
                                break;
                            }
                        },
                        _ => value.clone(),
                    };
                    rendered_attrs.push(' ');
                    rendered_attrs.push_str(key);
                    rendered_attrs.push_str("=\"");
                    rendered_attrs.push_str(&requote_attr(&value));
                    rendered_attrs.push('"');
                }
                if drop_element {
                    continue;
                }
                // 代替テキストのない画像はアクセシビリティ検証で落ちる。
                if name == "img" && !attrs.iter().any(|(key, _)| key == "alt") {
                    rendered_attrs.push_str(" alt=\"\"");
                }
                // `src` の無い `<img>` は参照先を持たない。書いても意味がない。
                if name == "img" && !rendered_attrs.contains(" src=\"") {
                    continue;
                }

                out.push('<');
                out.push_str(&name);
                out.push_str(&rendered_attrs);
                if VOID_ELEMENTS.contains(&name.as_str()) {
                    out.push_str(" />");
                } else if self_closing {
                    out.push_str("></");
                    out.push_str(&name);
                    out.push('>');
                } else {
                    out.push('>');
                    open.push(name);
                }
            }
            Token::EndTag { name } => {
                if DROPPED_ELEMENTS.contains(&name.as_str()) {
                    suppress = suppress.saturating_sub(1);
                    continue;
                }
                if suppress > 0 || VOID_ELEMENTS.contains(&name.as_str()) {
                    continue;
                }
                // 開いていない終了タグは捨てる。開いていれば、その内側を巻き戻す。
                let Some(depth) = open.iter().rposition(|item| *item == name) else {
                    continue;
                };
                while open.len() > depth {
                    let Some(item) = open.pop() else { break };
                    out.push_str("</");
                    out.push_str(&item);
                    out.push('>');
                }
            }
        }
    }
    while let Some(item) = open.pop() {
        out.push_str("</");
        out.push_str(&item);
        out.push('>');
    }
    out
}

/// リンク先として書き出してよい URI か判定し、書ける形に整える。
fn sanitize_uri(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lowered = trimmed.to_ascii_lowercase();
    // pixiv のキャプションにはアプリ内 URI が混ざる。EPUBCheck は未登録 scheme
    // として警告し、Kindle も開けないため、公開 Web URL へ置き換える。
    for (prefix, web) in [
        ("pixiv://users/", "https://www.pixiv.net/users/"),
        ("pixiv://illusts/", "https://www.pixiv.net/artworks/"),
        ("pixiv://artworks/", "https://www.pixiv.net/artworks/"),
        (
            "pixiv://novels/",
            "https://www.pixiv.net/novel/show.php?id=",
        ),
    ] {
        if lowered.starts_with(prefix) {
            return Some(format!("{}{}", web, &trimmed[prefix.len()..]));
        }
    }
    if trimmed.starts_with("//") {
        return Some(format!("https:{trimmed}"));
    }
    // 実行されうるものと、リーダーが解決できない埋め込みデータは持ち込まない。
    for scheme in ["javascript:", "data:", "vbscript:", "file:"] {
        if lowered.starts_with(scheme) {
            return None;
        }
    }
    // 登録されていない独自 scheme は EPUBCheck の警告になり、端末でも解決
    // できない。通常の相対 URL と、読書端末で扱える外部リンクだけを残す。
    if let Some(colon) = trimmed.find(':') {
        let boundary = trimmed.find(['/', '?', '#']).unwrap_or(trimmed.len());
        if colon < boundary
            && !["http:", "https:", "mailto:", "tel:"]
                .iter()
                .any(|scheme| lowered.starts_with(scheme))
        {
            return None;
        }
    }
    Some(trimmed.to_string())
}

// ============================================================
// EPUB 内のパスと識別子
// ============================================================

/// EPUB 内部のパスを URL として書ける形にする。
///
/// OPF の `href` も XHTML の `src` も IRI であり、空白や `#`、`?` を素で置くと
/// 別のものとして解釈される。日本語のファイル名も UTF-8 の %エンコードが要る。
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for segment in path.split('/') {
        if !out.is_empty() {
            out.push('/');
        }
        for byte in segment.as_bytes() {
            let ch = *byte as char;
            let unreserved = ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~');
            if unreserved {
                out.push(ch);
            } else {
                out.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    out
}

/// XML の ID として使える名前（NCName）に整える。
///
/// 先頭は文字か `_` でなければならず、数字始まりは不正。マニフェストの ID が
/// 一つでも不正なら EPUB 全体が検証に落ちる。
pub fn ncname(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 1);
    for ch in value.chars() {
        if ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let needs_prefix = out
        .chars()
        .next()
        .is_none_or(|ch| !(ch.is_alphabetic() || ch == '_'));
    if needs_prefix {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ampersands_and_angle_brackets_survive_as_xml() {
        assert_eq!(escape_text("A & B < C"), "A &amp; B &lt; C");
        assert_eq!(
            escape_attr("say \"hi\" & 'bye'"),
            "say &quot;hi&quot; &amp; &#39;bye&#39;"
        );
    }

    #[test]
    fn control_characters_illegal_in_xml_are_removed() {
        assert_eq!(escape_text("a\u{0}b\u{8}c\td"), "abc\td");
        assert_eq!(sanitize_fragment("<p>a\u{1}b</p>"), "<p>ab</p>");
    }

    #[test]
    fn html_entities_become_characters_xhtml_can_parse() {
        // `&nbsp;` は XHTML では未定義。そのまま通すと文書が解析不能になる。
        assert_eq!(
            sanitize_fragment("<p>a&nbsp;b&hellip;</p>"),
            "<p>a\u{A0}b…</p>"
        );
        assert_eq!(sanitize_fragment("<p>Q&A</p>"), "<p>Q&amp;A</p>");
        assert_eq!(sanitize_fragment("<p>&amp;lt;</p>"), "<p>&amp;lt;</p>");
        assert_eq!(sanitize_fragment("<p>&#x3042;</p>"), "<p>あ</p>");
    }

    #[test]
    fn unclosed_and_mismatched_tags_are_balanced() {
        assert_eq!(sanitize_fragment("<p>one<br>two"), "<p>one<br />two</p>");
        assert_eq!(sanitize_fragment("<b><i>x</b></i>"), "<b><i>x</i></b>");
        assert_eq!(sanitize_fragment("</p>stray"), "stray");
    }

    #[test]
    fn blocks_inside_a_paragraph_close_it_first() {
        // `<p>` の中の `<div>` は XHTML の内容モデル違反で検証エラーになる。
        assert_eq!(
            sanitize_fragment("<p>one<div>two</div>"),
            "<p>one</p><div>two</div>"
        );
        assert_eq!(
            sanitize_fragment("<ul><li>a<li>b</ul>"),
            "<ul><li>a</li><li>b</li></ul>"
        );
    }

    #[test]
    fn scripts_and_embeds_are_dropped_with_their_contents() {
        assert_eq!(sanitize_fragment("a<script>evil()</script>b"), "ab");
        assert_eq!(
            sanitize_fragment("<p>x<iframe src=\"https://e\"><b>y</b></iframe>z</p>"),
            "<p>xz</p>"
        );
        assert_eq!(sanitize_fragment("<font color=\"red\">kept</font>"), "kept");
    }

    #[test]
    fn dangerous_and_unknown_attributes_are_dropped() {
        assert_eq!(
            sanitize_fragment("<a href=\"javascript:x()\" onclick=\"x()\">t</a>"),
            "<a>t</a>"
        );
        assert_eq!(
            sanitize_fragment("<a href=\"https://a?b=1&c=2\">t</a>"),
            "<a href=\"https://a?b=1&amp;c=2\">t</a>"
        );
    }

    #[test]
    fn pixiv_app_links_become_portable_web_links() {
        assert_eq!(
            sanitize_fragment("<a href=\"pixiv://users/72817370\">作者</a>"),
            "<a href=\"https://www.pixiv.net/users/72817370\">作者</a>"
        );
        assert_eq!(
            sanitize_fragment("<a href=\"custom://thing\">表示名</a>"),
            "<a>表示名</a>"
        );
    }

    #[test]
    fn images_are_resolved_and_unresolvable_ones_removed() {
        let rendered = sanitize_fragment_with(
            "<p><img src=\"a.jpg\"><img src=\"b.jpg\"></p>",
            &mut |src| {
                if src == "a.jpg" {
                    ImageRef::Keep("../images/a.png".into())
                } else {
                    ImageRef::Drop
                }
            },
        );
        assert_eq!(rendered, "<p><img src=\"../images/a.png\" alt=\"\" /></p>");
    }

    #[test]
    fn duplicate_ids_are_not_written_twice() {
        assert_eq!(
            sanitize_fragment("<p id=\"1a\">x</p><p id=\"1a\">y</p>"),
            "<p id=\"_1a\">x</p><p>y</p>"
        );
    }

    #[test]
    fn paths_and_identifiers_are_written_in_forms_xml_accepts() {
        assert_eq!(
            encode_path("images/表紙 1.jpg"),
            "images/%E8%A1%A8%E7%B4%99%201.jpg"
        );
        assert_eq!(encode_path("text/page_001.xhtml"), "text/page_001.xhtml");
        assert_eq!(ncname("123456_p0"), "_123456_p0");
        assert_eq!(ncname("a b/c"), "a_b_c");
        assert_eq!(ncname(""), "_");
    }

    #[test]
    fn stripping_tags_leaves_readable_text() {
        assert_eq!(strip_tags("<p>a<br />b</p><p>c &amp; d</p>"), "a b c & d");
    }
}
