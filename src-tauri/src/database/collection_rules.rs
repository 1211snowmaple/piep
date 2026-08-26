//! 束ねるための、題名の読み方。
//!
//! 候補生成・命名・版違いの畳み込みが、同じ語彙を別々に持たないよう一箇所に
//! 集めてある。検索側の正規化（`search::normalize_search_text`）とは目的が違う。
//! あちらは**照合の鍵**を作るのでカタカナをひらがなへ倒し記号を落とすが、
//! ここが作るのは**人に見せる文字列**と、それとは別の照合鍵である。
//!
//! この区別を失ったことが「はいすぺいけめん女子の綾城さん」という名前の
//! 出どころだった。検索の鍵をそのまま画面へ出していた。

use std::sync::OnceLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// 告知・活動報告のたぐいを示す語。
///
/// 作品として保存されてはいるが、読む単位ではない。束に混ざると
/// 「2023年5月進捗のご報告」が5件で1つの連載に見えてしまう。
const ADMINISTRATIVE_TITLE: &str = r"お知らせ|告知|活動報告|活動記録|雑談|近況|予定|スケジュール|プラン|支援サイト|ご挨拶|アンケート|募集|目次|公開方針|進捗|展望|ご報告|作家様紹介|まとめ記事";

/// 公式シリーズ名が、作者の管理用ラベルであることを示す語。
///
/// pixiv のシリーズは作品のまとまりとは限らない。「有償依頼」151作、
/// 「リクエストシリーズ」21作のように、受注区分がそのまま入っている。
/// 束の名前には使えない。
const ADMINISTRATIVE_SERIES: &str =
    r"有償依頼|リクエスト|支援|サンプル|依頼|投げ銭|お礼|寄稿|まとめ|その他";

/// 同じ作品の別版を示す語。続きではない。
const EDITION_MARKER: &str = r"先行公開|おまけ付き|全文|サンプル|ｻﾝﾌﾟﾙ|体験版|支援者限定|支援者|フル版|完全版|前後編合体|再掲";

/// 版の照合で落として良い括弧の中身。
///
/// 取得元の名前・字数・版の呼び名だけ。実データの「【FANBOXサンプル】」のように
/// 複数が続けて入るので、下で並びとして組み立てる。
const EDITION_BRACKET_TOKEN: &str = r"FANBOX|ファンボックス|pixiv|ピクシブ|支援サイト|先行公開|おまけ付き|全文|サンプル|ｻﾝﾌﾟﾙ|体験版|支援者限定|支援者|フル版|完全版|再掲|本文全体[^】〕）)\]]{0,12}|[0-9,．.万千]+\s*(?:文字|字)";

/// 話数・巻数を表す形。
///
/// 現行の `title_stem_and_order` に無かった `周目`・丸数字・`part`・`ep`・
/// 先頭の `03：` 形をここで拾う。実データに「魔法少女、繰り返す: １２周目」や
/// 「①性杯戦争に巻き込まれた〜」があり、どちらも順序として読めていなかった。
const ORDINAL: &str = r"(?:第\s*[0-9]{1,3}|[0-9]{1,3}\s*(?:話|章|回|編|部|夜|巻|周目|節|幕)|#\s*[0-9]{1,3}|その\s*[0-9]{1,3}|part\s*[0-9]{1,3}|ep\.?\s*[0-9]{1,3}|[\u{2460}-\u{2473}]|前編|中編|後編|前篇|中篇|後篇|完結編|最終話|最終回|番外編|外伝|プロローグ|エピローグ)";

/// 括弧で囲われた注記。表示用の語幹を作るときだけ落とす。
const ANY_BRACKET: &str = r"[【〔（(\[][^】〕）)\]]{0,20}[】〕）)\]]";

fn cached(slot: &'static OnceLock<Regex>, pattern: &'static str) -> &'static Regex {
    slot.get_or_init(|| Regex::new(pattern).expect("collection rule regex"))
}

fn administrative_title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached(&RE, ADMINISTRATIVE_TITLE)
}

fn administrative_series_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached(&RE, ADMINISTRATIVE_SERIES)
}

fn edition_marker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached(&RE, EDITION_MARKER)
}

/// **前編・後編は絶対に落とさない。** 落とすと前編と後編が「同じ作品の別版」に
/// なり、いちばん壊してはいけない読む順が消える。だから括弧の中身が版の呼び名
/// だけで埋まっているときにしか、その括弧を落とさない。
fn edition_bracket_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let token = format!("(?:{EDITION_BRACKET_TOKEN})");
        Regex::new(&format!(
            r"(?i)[【〔（(\[]\s*{token}(?:\s*[・/／,，、+＋]?\s*{token})*\s*[】〕）)\]]"
        ))
        .expect("edition bracket regex")
    })
}

fn ordinal_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!("(?i){ORDINAL}")).expect("ordinal regex"))
}

fn any_bracket_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached(&RE, ANY_BRACKET)
}

fn leading_number_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached(&RE, r"^\s*[0-9]{1,3}\s*[：:．.、_\-]?\s*")
}

/// 全角と半角を揃えるだけの正規化。かなは倒さず、記号も残す。
fn width_normalized(input: &str) -> String {
    input.nfkc().collect()
}

/// 束ねる対象にしない作品か。
///
/// 題名が告知型であるか、本文がほとんど無い記事であれば真。作品一覧から
/// 消すわけではなく、**候補生成が拾わない**というだけの判断である。
pub fn is_administrative_post(title: &str, content_type: &str, text_length: i64) -> bool {
    if administrative_title_re().is_match(&width_normalized(title)) {
        return true;
    }
    // FANBOX の記事は本文が短いものほど告知である確率が高い。小説（novel）は
    // 短くても作品なので、種別を見てから字数を見る。
    content_type == "article" && text_length < 1_200
}

/// 公式シリーズ名を束の名前に使ってよいか。
pub fn is_administrative_series_label(title: &str) -> bool {
    let trimmed = title.trim();
    trimmed.is_empty() || administrative_series_re().is_match(&width_normalized(trimmed))
}

/// 版の呼び名を含む題名か。
pub fn has_edition_marker(title: &str) -> bool {
    edition_marker_re().is_match(&width_normalized(title))
}

/// 人に見せられる語幹。
///
/// 括弧の注記と話数を落とし、前後の区切り記号を整えるだけ。カタカナも記号も
/// そのまま残るので、そのまま束の名前にできる。
pub fn display_title_stem(title: &str) -> String {
    // 丸数字は幅を揃えると裸の数字に化けて、話数だと分からなくなる。先に落とす。
    let without_circled = title
        .chars()
        .map(|c| {
            if ('\u{2460}'..='\u{2473}').contains(&c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>();
    // 幅だけ先に揃える。「１２周目」の全角数字は、揃えないと話数として読めない。
    // かなは倒さないので、揃えたあとの文字列はそのまま画面に出せる。
    let normalized = width_normalized(&without_circled);
    let without_brackets = any_bracket_re().replace_all(&normalized, " ");
    let without_ordinal = ordinal_re().replace_all(&without_brackets, " ");
    // 「03：ドリームワールド〜」の先頭連番。助数詞も括弧も伴わないので、
    // 位置でしか話数と分からない。
    let leading = leading_number_re().replace(&without_ordinal, "");
    let without_ordinal = leading;
    // 連番だけが違う題名を束ねるので、裸の数字も落とす。ただし作品名の一部で
    // ある数字（「Fate/stay night」の類）は括弧にも助数詞にも囲まれないため、
    // 前後が空白か区切りのときだけ落とす。
    let mut out = String::with_capacity(without_ordinal.len());
    let mut previous_space = true;
    for ch in without_ordinal.chars() {
        if ch.is_whitespace() || ch == '\u{3000}' {
            if !previous_space {
                out.push(' ');
                previous_space = true;
            }
        } else {
            out.push(ch);
            previous_space = false;
        }
    }
    out.trim()
        .trim_matches(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '・' | ','
                        | '、'
                        | '。'
                        | '!'
                        | '?'
                        | '！'
                        | '？'
                        | '~'
                        | '〜'
                        | '-'
                        | '–'
                        | '—'
                        | '…'
                        | ':'
                        | '：'
                        | '/'
                        | '／'
                        | '＆'
                        | '&'
                        | '_'
                        | '＿'
                )
        })
        .to_string()
}

/// 題名から話数を読む。
///
/// 順に試して最初に当たったものを返す。「前編＋中編」のような合本は前の方の
/// 番号になる。合本を後ろへ送ると、分冊と合本が同じ束にあるとき読み始めが
/// 合本の後になってしまう。
pub fn episode_order(title: &str) -> Option<i64> {
    // 丸数字は題名の途中に置かれることが多く、他のどの形とも衝突しない。
    for ch in title.chars() {
        if ('\u{2460}'..='\u{2473}').contains(&ch) {
            return Some(ch as i64 - 0x245F);
        }
    }
    let normalized = width_normalized(title);
    let numbered = [
        r"^\s*[【〔\[]?\s*([0-9]{1,3})\s*[：:．.、_\-）)】〕\]]",
        r"第\s*([0-9]{1,3})",
        r"([0-9]{1,3})\s*(?:話|章|回|編|部|夜|巻|周目)",
        r"#\s*([0-9]{1,3})",
        r"その\s*([0-9]{1,3})",
        r"(?i)part\s*([0-9]{1,3})",
        r"(?i)ep\.?\s*([0-9]{1,3})",
    ];
    for pattern in numbered {
        let regex = Regex::new(pattern).expect("episode pattern");
        if let Some(capture) = regex.captures(&normalized) {
            if let Some(value) = capture.get(1).and_then(|m| m.as_str().parse::<i64>().ok()) {
                return Some(value);
            }
        }
    }
    // 番号を持たない区切り語。プロローグを 0、番外を末尾に置く。
    for (word, order) in [
        ("プロローグ", 0),
        ("前編", 1),
        ("前篇", 1),
        ("中編", 2),
        ("中篇", 2),
        ("後編", 3),
        ("後篇", 3),
        ("完結編", 4),
        ("最終話", 98),
        ("最終回", 98),
        ("エピローグ", 99),
        ("番外編", 100),
        ("外伝", 100),
    ] {
        if title.contains(word) {
            return Some(order);
        }
    }
    None
}

/// 版どうしを突き合わせる鍵。
///
/// 版の呼び名だけを落とし、話数の語は残す。`display_title_stem` と違って
/// 照合に使うので、かなを倒し記号を落として揺れを吸収する。
pub fn edition_match_key(title: &str) -> String {
    let without_bracket = edition_bracket_re().replace_all(title, "");
    let without_marker = edition_marker_re().replace_all(&without_bracket, "");
    crate::database::search::normalize_search_text(&without_marker)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// 束ねるための照合鍵。話数と括弧を落としたうえで揺れを吸収する。
pub fn family_match_key(title: &str) -> String {
    crate::database::search::normalize_search_text(&display_title_stem(title))
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// 題名に話数の語があるか。題名族を束と認めてよいかの判断に使う。
pub fn has_ordinal_marker(title: &str) -> bool {
    ordinal_re().is_match(&width_normalized(title))
}

/// 合本が含む話数の範囲。
///
/// 「【前編＋中編】…」は前編と後編の**あいだ**にある別の作品ではなく、
/// 二つをまとめた版である。束の中で並べると、同じ本文が二度出てくる。
///
/// 返すのは `(始まり, 終わり)`。単独の話しか読めなければ `None`。
pub fn composite_episode_range(title: &str) -> Option<(i64, i64)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let regex = RE.get_or_init(|| {
        // 「前編＋中編」「前後編合体」「前編・中編・後編」のような並び。
        Regex::new(r"(前編|前篇|中編|中篇|後編|後篇|完結編)\s*[＋+・、,&＆]\s*(前編|前篇|中編|中篇|後編|後篇|完結編)")
            .expect("composite episode regex")
    });
    let normalized = width_normalized(title);
    if normalized.contains("前後編合体") || normalized.contains("前後編まとめ") {
        return Some((1, 3));
    }
    let capture = regex.captures(&normalized)?;
    let order = |word: &str| match word {
        "前編" | "前篇" => 1,
        "中編" | "中篇" => 2,
        "後編" | "後篇" => 3,
        _ => 4,
    };
    let first = order(capture.get(1)?.as_str());
    let last = order(capture.get(2)?.as_str());
    // 三つ以上並ぶこともある。最後に出てくるものまでを範囲に含める。
    let last = regex
        .captures_iter(&normalized)
        .filter_map(|value| value.get(2).map(|m| order(m.as_str())))
        .max()
        .unwrap_or(last);
    Some((first.min(last), first.max(last)))
}

/// 束を言い表せないタグ。
///
/// 棚のほとんどに付いているので、共有していても何も説明しない。
/// 「R-18 の作品をまとめました」は、まとまりの説明になっていない。
pub const UNINFORMATIVE_TAGS: &[&str] = &[
    "R-18",
    "R18",
    "R-18G",
    "オリジナル",
    "一次創作",
    "二次創作",
    "小説",
    "エロ小説",
    "官能小説",
    "AI挿絵",
    "長編",
    "短編",
];

/// 作者の管理用ラベルとして付いているタグ。
///
/// 「リクエスト品」「skeb依頼」は受注の区分であって、物語の共通点ではない。
/// 公式シリーズ名で同じことが起きるのと同じ理由で、束の起点にも名前にもしない。
const ADMINISTRATIVE_TAG: &str =
    r"リクエスト|依頼|支援|サンプル|skeb|スケブ|有償|お礼|宣伝|告知|進捗";

fn administrative_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!("(?i){ADMINISTRATIVE_TAG}")).expect("admin tag regex"))
}

/// 束の名前や説明に使えるタグか。
pub fn is_informative_tag(tag: &str) -> bool {
    let trimmed = tag.trim();
    !trimmed.is_empty()
        && !UNINFORMATIVE_TAGS.iter().any(|value| value == &trimmed)
        && !administrative_tag_re().is_match(&width_normalized(trimmed))
}

/// 名前として短すぎず長すぎないか整える。
///
/// 束の名前は棚のカードに出るので、行に収まる長さで切る。切ったことが
/// 分かるように末尾へ `…` を置く。
pub fn clamp_name(name: &str, max_chars: usize) -> String {
    let trimmed = name.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = trimmed.chars().take(max_chars).collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_stem_keeps_katakana_and_marks() {
        // 現行の実装はここで「はいすぺいけめん女子の綾城さん」を返していた。
        let stem = display_title_stem(
            "『同人女の感情』ハイスペイケメン女子の綾城さんにキモデブが溺愛されて界隈の「姫」になる話#3",
        );
        assert!(
            stem.contains("ハイスペイケメン"),
            "カタカナが倒れている: {stem}"
        );
        assert!(stem.contains('『'), "記号が落ちている: {stem}");
        assert!(!stem.contains("#3"), "連番が残っている: {stem}");
    }

    #[test]
    fn display_stem_drops_bracket_notes_and_episode_words() {
        assert_eq!(
            display_title_stem("【前編】一介の男子高校を自らの手中に収める話"),
            "一介の男子高校を自らの手中に収める話"
        );
        assert_eq!(
            display_title_stem("魔法少女、繰り返す: １２周目"),
            "魔法少女、繰り返す"
        );
    }

    #[test]
    fn episode_order_reads_the_vocabulary_the_old_rule_missed() {
        assert_eq!(episode_order("④性杯戦争に巻き込まれたあなたが"), Some(4));
        assert_eq!(episode_order("魔法少女、繰り返す: １２周目"), Some(12));
        assert_eq!(
            episode_order("03：ドリームワールド〜多重クロスオーバー〜（前編）"),
            Some(3)
        );
        assert_eq!(episode_order("冒険者ギルドの受付嬢助手の話#4"), Some(4));
        assert_eq!(episode_order("Part 2 何かの話"), Some(2));
        assert_eq!(episode_order("【後編】鮎川オリビアの話"), Some(3));
        assert_eq!(episode_order("順序のない題名"), None);
    }

    #[test]
    fn episode_order_prefers_the_earlier_half_of_a_combined_volume() {
        // 合本は前の番号で並ぶ。後ろへ送ると読み始めが合本の後になる。
        assert_eq!(episode_order("【前編＋中編】一介の男子高校の話"), Some(1));
    }

    #[test]
    fn administrative_posts_are_kept_out_of_bundles() {
        assert!(is_administrative_post(
            "2023年5月進捗のご報告と6月の展望",
            "article",
            8_000
        ));
        assert!(is_administrative_post(
            "【重要なお知らせ】小説作品の公開方針を変更いたします",
            "novel",
            2_395
        ));
        // 短い記事は告知として扱うが、短い小説は作品のまま。
        assert!(is_administrative_post("何かの記事", "article", 400));
        assert!(!is_administrative_post("短い掌編", "novel", 400));
        assert!(!is_administrative_post(
            "催眠アプリで人生を染められる話",
            "novel",
            20_000
        ));
    }

    #[test]
    fn composite_volumes_are_recognised_as_covering_a_range() {
        assert_eq!(
            composite_episode_range("【前編＋中編】一介の男子高校の話"),
            Some((1, 2))
        );
        assert_eq!(
            composite_episode_range("【後編＋完結編】一介の男子高校の話"),
            Some((3, 4))
        );
        assert_eq!(
            composite_episode_range("【前後編合体】結束バンドの話"),
            Some((1, 3))
        );
        // 単独の話は合本ではない。ここを取り違えると、前編が消える。
        assert_eq!(composite_episode_range("【前編】一介の男子高校の話"), None);
        assert_eq!(composite_episode_range("順序のない題名"), None);
    }

    #[test]
    fn administrative_tags_do_not_describe_a_bundle() {
        // 受注の区分は物語の共通点ではない。実データで「リクエスト品」が
        // 36作の束の名前になっていた。
        assert!(!is_informative_tag("リクエスト品"));
        assert!(!is_informative_tag("skeb依頼"));
        assert!(!is_informative_tag("有償依頼"));
        assert!(!is_informative_tag("R-18"));
        assert!(is_informative_tag("黛冬優子"));
        assert!(is_informative_tag("催眠"));
        assert!(is_informative_tag("ぼっち・ざ・ろっく!"));
    }

    #[test]
    fn administrative_series_labels_are_not_names() {
        assert!(is_administrative_series_label("有償依頼"));
        assert!(is_administrative_series_label("リクエストシリーズ"));
        assert!(is_administrative_series_label("支援サイトサンプル"));
        assert!(!is_administrative_series_label("好き好き大好き黛冬優子"));
        assert!(!is_administrative_series_label("爛歪のMARIONETTA"));
    }

    #[test]
    fn edition_key_folds_editions_but_never_folds_reading_order() {
        let plain = edition_match_key("性杯戦争〜アーチャー陣営〜");
        assert_eq!(
            edition_match_key("【FANBOXサンプル】性杯戦争〜アーチャー陣営〜"),
            plain
        );
        assert_eq!(
            edition_match_key("【全文】性杯戦争〜アーチャー陣営〜"),
            plain
        );
        // ここが肝心。前編と後編が同じ鍵になってはいけない。
        assert_ne!(
            edition_match_key("【前編】一介の男子高校の話"),
            edition_match_key("【後編】一介の男子高校の話")
        );
    }

    #[test]
    fn family_key_groups_the_numbered_siblings() {
        let a = family_match_key("①性杯戦争に巻き込まれたあなたが二人をハメ潰す話");
        let b = family_match_key("⑦性杯戦争に巻き込まれたあなたが二人をハメ潰す話");
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }
}
