//! 詳細設定。取得の間隔・やり直しの回数・履歴の保持数など、利用者が変えてよい細かい値。
//!
//! 値はアプリのフォルダーの `advanced-settings.json` に置く。読むのはほとんどが Rust の
//! 処理なので、画面ではなくここが持つ。画面だけが使う値（読む速さ・処理の記録の数）も
//! 同じ所に置き、**範囲を一か所にだけ書く。** 画面は [`specs`] から範囲を受け取る。
//!
//! 範囲には二つの意味がある。取得元との通信の間隔は、下限が**アカウントを守る線**で、
//! 今の値の半分より速くはできない（縮めた人が取得元に断られ、止められる）。そのほかは
//! 意味のある値の幅にすぎない。どちらも、範囲外を保存しようとしたら丸めずに断る。
//! 手で書き換えたファイルに範囲外の値があれば、その項目だけ既定で読む。

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

const SETTINGS_FILE: &str = "advanced-settings.json";

/// 値の単位。画面の見せ方と、断るときの言い方が変わる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unit {
    /// 保存はミリ秒、見せるのは秒（小数一桁）。
    Milliseconds,
    Seconds,
    Minutes,
    Days,
    Count,
    CharsPerMinute,
}

/// 項目の群。画面のカードの分け方。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Group {
    Source,
    History,
    Server,
    Assist,
    Reading,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub unit: Unit,
    pub default: i64,
    pub min: i64,
    pub max: i64,
}

macro_rules! settings {
    ($($variant:ident = $key:literal, $label:literal, $group:ident, $unit:ident, $default:expr, $min:expr, $max:expr;)*) => {
        /// 詳細設定の項目。
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Setting { $($variant),* }

        const SPECS: &[SettingSpec] = &[$(SettingSpec {
            key: $key,
            label: $label,
            group: Group::$group,
            unit: Unit::$unit,
            default: $default,
            min: $min,
            max: $max,
        }),*];
    };
}

settings! {
    // 取得元との通信。間隔の下限は今の値の半分。
    RequestGapMs = "requestGapMs", "取得元へ問い合わせる間隔", Source, Milliseconds, 800, 400, 60_000;
    UpdateCheckDelayMs = "updateCheckDelayMs", "更新の確認で、作品や作者の間にあける時間", Source, Milliseconds, 2_000, 1_000, 60_000;
    NewWorksDelayMs = "newWorksDelayMs", "新作だけの確認で、作者の間にあける時間", Source, Milliseconds, 1_500, 750, 60_000;
    RateLimitRetries = "rateLimitRetries", "取得制限に当たった作品をやり直す回数", Source, Count, 3, 0, 5;
    ConsecutiveErrorLimit = "consecutiveErrorLimit", "失敗が続いたら更新の確認を止める回数", Source, Count, 3, 1, 10;
    DeepCheckIntervalDays = "deepCheckIntervalDays", "本文まで突き合わせる間隔", Source, Days, 14, 7, 90;
    SourceTimeoutSecs = "sourceTimeoutSecs", "取得元の応答を待つ時間", Source, Seconds, 30, 15, 120;
    AssetDownloadConcurrency = "assetDownloadConcurrency", "画像を同時に取る数", Source, Count, 2, 1, 2;
    // 欠けたまま確定した最初の保存も 1 回に数える（library/repair_attempts.rs）。1 は取り直さない。
    RepairAttempts = "repairAttempts", "欠けた作品を取りに行く回数", Source, Count, 2, 1, 5;
    // 履歴
    KeepUpdateJobs = "keepUpdateJobs", "更新の確認の履歴を残す件数", History, Count, 50, 10, 500;
    KeepUpdateJobDays = "keepUpdateJobDays", "更新の確認の履歴を残す日数", History, Days, 30, 7, 365;
    OperationJobsKept = "operationJobsKept", "処理の記録を残す件数", History, Count, 100, 20, 500;
    // サーバーへの自動送信
    AutoSendQuietMinutes = "autoSendQuietMinutes", "棚の変更が落ち着いてから送るまで", Server, Minutes, 5, 1, 60;
    AutoSendRetryMinutes = "autoSendRetryMinutes", "送れなかったときにやり直すまで", Server, Minutes, 15, 5, 120;
    // AI
    AssistTimeoutSecs = "assistTimeoutSecs", "AI の応答を待つ時間", Assist, Seconds, 180, 60, 1_800;
    // 読む
    ReadingCharsPerMinute = "readingCharsPerMinute", "読む速さ", Reading, CharsPerMinute, 550, 200, 2_000;
}

const COUNT: usize = SPECS.len();

/// 全項目の値。並びは [`SPECS`] と同じ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Values([i64; COUNT]);

impl Default for Values {
    fn default() -> Self {
        let mut values = [0; COUNT];
        for (slot, spec) in values.iter_mut().zip(SPECS) {
            *slot = spec.default;
        }
        Self(values)
    }
}

impl Values {
    pub fn get(&self, setting: Setting) -> i64 {
        self.0[setting as usize]
    }

    /// 画面から来た値を確かめる。知らない名前、範囲の外は断る。書いていない項目は既定。
    pub fn validate(input: &BTreeMap<String, i64>) -> Result<Self, String> {
        let mut values = Self::default();
        for (key, value) in input {
            let Some(index) = SPECS.iter().position(|spec| spec.key == key) else {
                return Err(format!("知らない詳細設定です: {key}"));
            };
            let spec = &SPECS[index];
            if !(spec.min..=spec.max).contains(value) {
                return Err(format!(
                    "「{}」は {}〜{} の範囲で決めてください（{} は使えません）",
                    spec.label,
                    describe(spec.unit, spec.min),
                    describe(spec.unit, spec.max),
                    describe(spec.unit, *value),
                ));
            }
            values.0[index] = *value;
        }
        Ok(values)
    }

    /// ファイルから読む。壊れていれば全部を既定に、範囲外の項目はその項目だけ既定にする。
    pub fn parse(text: &str) -> Self {
        let mut values = Self::default();
        let Ok(raw) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(text) else {
            log::warn!("詳細設定のファイルを読めません。既定の値で動きます");
            return values;
        };
        for (index, spec) in SPECS.iter().enumerate() {
            match raw.get(spec.key).map(serde_json::Value::as_i64) {
                None => {}
                Some(Some(value)) if (spec.min..=spec.max).contains(&value) => {
                    values.0[index] = value;
                }
                Some(_) => log::warn!(
                    "詳細設定の「{}」が範囲の外なので、既定の値で動きます",
                    spec.label
                ),
            }
        }
        values
    }

    /// 名前ごとの値。ファイルと画面へ渡す形。
    pub fn to_map(self) -> BTreeMap<&'static str, i64> {
        SPECS.iter().map(|spec| spec.key).zip(self.0).collect()
    }
}

fn describe(unit: Unit, value: i64) -> String {
    match unit {
        Unit::Milliseconds => {
            let seconds = value as f64 / 1_000.0;
            if value % 1_000 == 0 {
                format!("{seconds:.0} 秒")
            } else {
                format!("{seconds} 秒")
            }
        }
        Unit::Seconds => format!("{value} 秒"),
        Unit::Minutes => format!("{value} 分"),
        Unit::Days => format!("{value} 日"),
        Unit::Count => format!("{value}"),
        Unit::CharsPerMinute => format!("{value} 字/分"),
    }
}

static PATH: OnceLock<PathBuf> = OnceLock::new();
static CURRENT: RwLock<Option<Values>> = RwLock::new(None);

/// 起動のときに一度呼ぶ。無ければ既定で動く。
pub fn load(app_data: &Path) {
    let path = app_data.join(SETTINGS_FILE);
    let values = std::fs::read_to_string(&path)
        .map(|text| Values::parse(&text))
        .unwrap_or_default();
    let _ = PATH.set(path);
    set_current(values);
}

fn set_current(values: Values) {
    let mut current = CURRENT
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *current = Some(values);
}

/// 今の値。読み込む前（試験など）は既定。
pub fn current() -> Values {
    CURRENT
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .unwrap_or_default()
}

pub fn specs() -> &'static [SettingSpec] {
    SPECS
}

/// 確かめて、書いて、効かせる。書けなければ効かせない（次の起動で戻ってしまうため）。
pub fn save(input: &BTreeMap<String, i64>) -> Result<Values, String> {
    let values = Values::validate(input)?;
    let path = PATH
        .get()
        .ok_or_else(|| "アプリのフォルダーが決まっていません".to_string())?;
    let text = serde_json::to_string_pretty(&values.to_map()).map_err(|e| e.to_string())?;
    // 途中で落ちても半端な設定を残さない。
    let stage = path.with_extension("json.part");
    std::fs::write(&stage, text).map_err(|e| e.to_string())?;
    std::fs::rename(&stage, path).map_err(|e| e.to_string())?;
    set_current(values);
    Ok(values)
}

fn get(setting: Setting) -> i64 {
    current().get(setting)
}

fn non_negative(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

// 使う側の読み口。単位の換算はここで済ませる。

/// 取得元（pixiv・FANBOX それぞれ）へ続けて問い合わせるときの最小の間隔。
pub fn request_gap() -> Duration {
    Duration::from_millis(non_negative(get(Setting::RequestGapMs)))
}

pub fn update_check_delay_ms() -> u64 {
    non_negative(get(Setting::UpdateCheckDelayMs))
}

pub fn new_works_delay_ms() -> u64 {
    non_negative(get(Setting::NewWorksDelayMs))
}

pub fn rate_limit_retries() -> u32 {
    get(Setting::RateLimitRetries) as u32
}

/// 欠けたまま保存した作品を、同じ版・同じ欠けで取りに行く回数の上限（最初の保存を含む）。
pub fn repair_attempts() -> u32 {
    get(Setting::RepairAttempts) as u32
}

pub fn consecutive_error_limit() -> u32 {
    get(Setting::ConsecutiveErrorLimit) as u32
}

pub fn deep_check_interval_days() -> i64 {
    get(Setting::DeepCheckIntervalDays)
}

/// 取得元の API と画像の配信元の応答を待つ時間。
pub fn source_timeout() -> Duration {
    Duration::from_secs(non_negative(get(Setting::SourceTimeoutSecs)))
}

/// pixiv のウェブ版の要求は API より軽いので、上の 3 分の 2 にする（既定で 20 秒）。
pub fn source_web_timeout() -> Duration {
    Duration::from_secs(non_negative(get(Setting::SourceTimeoutSecs)) * 2 / 3)
}

pub fn asset_download_concurrency() -> usize {
    get(Setting::AssetDownloadConcurrency) as usize
}

pub fn keep_update_jobs() -> i64 {
    get(Setting::KeepUpdateJobs)
}

pub fn keep_update_job_days() -> i64 {
    get(Setting::KeepUpdateJobDays)
}

pub fn auto_send_quiet() -> Duration {
    Duration::from_secs(non_negative(get(Setting::AutoSendQuietMinutes)) * 60)
}

pub fn auto_send_retry() -> Duration {
    Duration::from_secs(non_negative(get(Setting::AutoSendRetryMinutes)) * 60)
}

/// AI の応答を待つ時間。
///
/// 長編の部分抽出では、モデルの初回ロードと十分な思考時間も含めて待つ。
/// 60秒では、GPUへ載せた直後や長いチャンクで正常な応答まで打ち切っていた。
pub fn assist_timeout() -> Duration {
    Duration::from_secs(non_negative(get(Setting::AssistTimeoutSecs)))
}

/// 全文を送る長い問い合わせ。上の値を短くしても 300 秒は待つ。
///
/// 6万トークンの本文でも、読み込み（約1,100 トークン/秒）と答え（約300
/// トークン）で1分少々。以前は思考を含めて900秒待っていたが、GPU のメモリが
/// 溢れた推論サーバーは数十トークン/秒まで落ちて止まり、利用者は15分待った
/// あげくに失敗を見ることになった。だから既定は 300 秒で、それより長く待つのは
/// 利用者が詳細設定で延ばしたときだけ。
pub fn assist_long_timeout() -> Duration {
    assist_timeout().max(Duration::from_secs(300))
}

#[cfg(test)]
#[path = "advanced_settings_tests.rs"]
mod tests;
