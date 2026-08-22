pub mod fanbox;
pub mod pixiv;
pub mod webview;

/// 相手に送る意味のある Cookie の名前。
///
/// - セッション（`PHPSESSID` / `FANBOXSESSID`）— 名乗るための鍵そのもの
/// - `cf_clearance` — 内蔵ブラウザが自分で通り抜けた Cloudflare の通行証。
///   失うと、こちらでは解けない検問に当たったときに手が無くなる
const ESSENTIAL_COOKIE_NAMES: [&str; 3] = ["PHPSESSID", "FANBOXSESSID", "cf_clearance"];

/// 送る Cookie を、要るものだけに絞る。
///
/// ログイン窓が持ち帰る Cookie には、解析や広告の識別子まで混じっている
/// （実測で pixiv は 36 個 2,869 文字、FANBOX は 14 個 1,086 文字）。
/// **相手が要るのはセッションひとつで、実測でもそれだけで返事は変わらない。**
/// 残りは送る理由も、手元に残す理由もない。
///
/// 保存済みの古い値にも同じ規則を通すので、繋ぎ直さなくても余計なものは出ていかない。
pub fn essential_cookies(header: &str) -> String {
    header
        .split(';')
        .filter_map(|pair| {
            let pair = pair.trim();
            let name = pair.split('=').next()?.trim();
            ESSENTIAL_COOKIE_NAMES
                .contains(&name)
                .then(|| pair.to_string())
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::essential_cookies;

    /// 解析・広告の識別子は、送らないし持たない。
    #[test]
    fn only_the_keys_that_do_something_survive() {
        let captured = "_ga=GA1.2.x; PHPSESSID=123_abc; cto_bundle=junk; \
                        cf_clearance=pass; __gads=ad; p_ab_id=7";
        assert_eq!(
            essential_cookies(captured),
            "PHPSESSID=123_abc; cf_clearance=pass"
        );
    }

    /// FANBOX も同じ規則で通す。取得元ごとに別の作法を作らない。
    #[test]
    fn the_same_rule_serves_both_sources() {
        let captured = "FANBOXSESSID=abc; _gid=1; privacy_policy_agreement=1; cf_clearance=pass";
        assert_eq!(
            essential_cookies(captured),
            "FANBOXSESSID=abc; cf_clearance=pass"
        );
    }

    /// 名前が似ているだけのものを拾わない。
    #[test]
    fn a_name_that_merely_looks_like_a_session_is_not_one() {
        assert_eq!(essential_cookies("PHPSESSIDX=no; myPHPSESSID=no"), "");
    }

    /// 鍵がひとつも無ければ空。「何か送れた」ふりをしない。
    #[test]
    fn a_jar_without_keys_yields_nothing() {
        assert_eq!(essential_cookies("_ga=1; __cf_bm=2"), "");
        assert_eq!(essential_cookies(""), "");
    }
}
