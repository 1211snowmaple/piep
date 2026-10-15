use super::*;

fn order(title: &str) -> Option<i64> {
    title_stem_and_order(title).1
}

fn stem(title: &str) -> String {
    title_stem_and_order(title).0
}

/// 設計提案 6-3 が挙げる話数表記と、3 の例 C をそのまま通す。
#[test]
fn episode_numbers_cover_the_documented_notations() {
    assert_eq!(order("第12話"), Some(120));
    assert_eq!(order("12話"), Some(120));
    assert_eq!(order("#12"), Some(120));
    assert_eq!(order("その12"), Some(120));
    assert_eq!(order("十二"), Some(120));
    assert_eq!(order("XII"), Some(120));

    // 例 C: 表記が三者三様でも同じ連番として並ぶ。
    assert_eq!(order("航路 01"), Some(10));
    assert_eq!(order("航路 #2"), Some(20));
    assert_eq!(order("航路 第三夜"), Some(30));
    assert_eq!(stem("航路 01"), stem("航路 #2"));
    assert_eq!(stem("航路 #2"), stem("航路 第三夜"));
}

#[test]
fn front_and_back_markers_order_within_one_episode() {
    assert!(order("月下の約束 前編") < order("月下の約束 後編"));
    assert_eq!(order("【FANBOX】月下の約束（後編）"), Some(3));
    assert_eq!(stem("【FANBOX】月下の約束（後編）"), "fanbox 月下の約束");
    // 素の 上/中/下 も単独トークンなら位置として読む。
    assert_eq!(order("旅路 上"), Some(1));
    assert_eq!(order("旅路 中"), Some(2));
    assert_eq!(order("旅路 下"), Some(3));
    // 話数があるときは枝番として合成される。
    assert_eq!(order("第3話 後編"), Some(33));
}

#[test]
fn special_positions_sit_outside_the_numbered_run() {
    let prologue = order("ぷろろーぐ").expect("prologue");
    let first = order("第1話").expect("first");
    let finale = order("最終話").expect("finale");
    let epilogue = order("エピローグ").expect("epilogue");
    let side = order("番外編").expect("side story");
    assert!(prologue < first);
    assert!(first < finale);
    assert!(finale < epilogue);
    assert!(epilogue < side);
}

/// 語の一部を位置と読み違えないこと。ここが緩むと無関係な作品が混ざる。
#[test]
fn ordinary_words_are_not_mistaken_for_positions() {
    // `月下の約束` の `下` は語中なので後編ではない。
    assert_eq!(order("月下の約束"), None);
    // `finalize` は完結編ではない。
    assert_eq!(order("finalize"), None);
    // ローマ数字に使える文字だけの英単語も話数にしない。
    assert_eq!(order("civil"), None);
    // 西暦は単独トークンでも話数として拾わない。
    assert_eq!(order("記録 2026"), None);
}
