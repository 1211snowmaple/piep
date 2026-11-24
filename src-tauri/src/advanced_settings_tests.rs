use super::*;

fn input(pairs: &[(&str, i64)]) -> BTreeMap<String, i64> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), *value))
        .collect()
}

#[test]
fn defaults_are_the_values_the_code_used_before() {
    let values = Values::default();
    assert_eq!(values.get(Setting::RequestGapMs), 800);
    assert_eq!(values.get(Setting::UpdateCheckDelayMs), 2_000);
    assert_eq!(values.get(Setting::NewWorksDelayMs), 1_500);
    assert_eq!(values.get(Setting::RateLimitRetries), 3);
    assert_eq!(values.get(Setting::ConsecutiveErrorLimit), 3);
    assert_eq!(values.get(Setting::DeepCheckIntervalDays), 14);
    assert_eq!(values.get(Setting::SourceTimeoutSecs), 30);
    assert_eq!(values.get(Setting::AssetDownloadConcurrency), 2);
    assert_eq!(values.get(Setting::KeepUpdateJobs), 50);
    assert_eq!(values.get(Setting::KeepUpdateJobDays), 30);
    assert_eq!(values.get(Setting::OperationJobsKept), 100);
    assert_eq!(values.get(Setting::AutoSendQuietMinutes), 5);
    assert_eq!(values.get(Setting::AutoSendRetryMinutes), 15);
    assert_eq!(values.get(Setting::AssistTimeoutSecs), 180);
    assert_eq!(values.get(Setting::ReadingCharsPerMinute), 550);
}

#[test]
fn every_default_sits_inside_its_range() {
    for spec in specs() {
        assert!(
            spec.min <= spec.default && spec.default <= spec.max,
            "{}",
            spec.key
        );
    }
}

/// 取得元へ当たる間隔は、今の値の半分より速くできない。アカウントを守る線。
#[test]
fn source_intervals_cannot_go_below_half_of_today() {
    for spec in specs().iter().filter(|spec| {
        spec.group == Group::Source && matches!(spec.unit, Unit::Milliseconds | Unit::Days)
    }) {
        assert_eq!(spec.min * 2, spec.default, "{}", spec.key);
    }
    // 同時に取る数は今より増やせない。
    let concurrency = specs()
        .iter()
        .find(|spec| spec.key == "assetDownloadConcurrency")
        .unwrap();
    assert_eq!(concurrency.max, concurrency.default);
}

#[test]
fn validate_accepts_the_edges_of_the_range() {
    let values =
        Values::validate(&input(&[("requestGapMs", 400), ("rateLimitRetries", 5)])).unwrap();
    assert_eq!(values.get(Setting::RequestGapMs), 400);
    assert_eq!(values.get(Setting::RateLimitRetries), 5);
    // 書いていない項目は既定。
    assert_eq!(values.get(Setting::UpdateCheckDelayMs), 2_000);
}

#[test]
fn validate_refuses_values_outside_the_range_instead_of_clamping() {
    let error = Values::validate(&input(&[("requestGapMs", 399)])).unwrap_err();
    assert!(error.contains("取得元へ問い合わせる間隔"), "{error}");
    assert!(error.contains("0.4 秒〜60 秒"), "{error}");
    assert!(Values::validate(&input(&[("newWorksDelayMs", 749)])).is_err());
    assert!(Values::validate(&input(&[("updateCheckDelayMs", 60_001)])).is_err());
    assert!(Values::validate(&input(&[("rateLimitRetries", -1)])).is_err());
    assert!(Values::validate(&input(&[("rateLimitRetries", 6)])).is_err());
    assert!(Values::validate(&input(&[("consecutiveErrorLimit", 0)])).is_err());
    assert!(Values::validate(&input(&[("deepCheckIntervalDays", 6)])).is_err());
    assert!(Values::validate(&input(&[("assetDownloadConcurrency", 3)])).is_err());
    assert!(Values::validate(&input(&[("sourceTimeoutSecs", 14)])).is_err());
}

#[test]
fn validate_refuses_unknown_names() {
    let error = Values::validate(&input(&[("requestGap", 800)])).unwrap_err();
    assert!(error.contains("requestGap"), "{error}");
}

#[test]
fn a_hand_edited_file_falls_back_per_item() {
    let values = Values::parse(
        r#"{"requestGapMs": 100, "updateCheckDelayMs": 5000, "rateLimitRetries": "3"}"#,
    );
    // 範囲外・型違いはその項目だけ既定。
    assert_eq!(values.get(Setting::RequestGapMs), 800);
    assert_eq!(values.get(Setting::RateLimitRetries), 3);
    assert_eq!(values.get(Setting::UpdateCheckDelayMs), 5_000);
    assert_eq!(Values::parse("{ broken"), Values::default());
}

#[test]
fn derived_durations_follow_the_value() {
    let values = Values::validate(&input(&[("sourceTimeoutSecs", 60)])).unwrap();
    assert_eq!(values.get(Setting::SourceTimeoutSecs) * 2 / 3, 40);
    // 既定ではウェブ版は 20 秒のまま。
    assert_eq!(
        Values::default().get(Setting::SourceTimeoutSecs) * 2 / 3,
        20
    );
}

/// 保存すると効き、断られた保存は何も変えない。ファイルにも残り、次の起動で読める。
///
/// 全体の値を書き換えるので、ほかの試験が読まない AI の待ち時間だけを触る。
#[test]
fn save_applies_and_persists_and_a_refused_save_changes_nothing() {
    let dir = std::env::temp_dir().join(format!("piep-advanced-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    load(&dir);
    assert_eq!(assist_timeout(), Duration::from_secs(180));
    assert_eq!(assist_long_timeout(), Duration::from_secs(300));

    save(&input(&[("assistTimeoutSecs", 600)])).unwrap();
    assert_eq!(assist_timeout(), Duration::from_secs(600));
    assert_eq!(assist_long_timeout(), Duration::from_secs(600));
    let written = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
    assert_eq!(Values::parse(&written).get(Setting::AssistTimeoutSecs), 600);

    assert!(save(&input(&[("assistTimeoutSecs", 59)])).is_err());
    assert_eq!(assist_timeout(), Duration::from_secs(600));
    assert_eq!(
        Values::parse(&std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap())
            .get(Setting::AssistTimeoutSecs),
        600
    );

    save(&BTreeMap::new()).unwrap();
    assert_eq!(assist_timeout(), Duration::from_secs(180));
    let _ = std::fs::remove_dir_all(&dir);
}
