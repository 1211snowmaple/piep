//! モデルを一回の問い合わせではなく、手順として使うための足回り。
//!
//! 一往復で「全部入りの指示」を投げると、小さな失敗（形の崩れ、存在しない
//! 番号、言い過ぎ）がそのまま利用者に届く。ここでは次の三つを部品にする。
//!
//! - **会話**: 本文などの長い材料を先頭に固定し、仕事ごとの指示を後ろへ足す。
//!   先頭が同じ要求はサーバーの prompt cache が効き、二問目から本文を読み直さない。
//! - **形の検査と一度だけの直し**: 返ってきた JSON をアプリが検査し、使えない
//!   理由を添えて同じ会話で出し直させる。直しは一度まで。
//! - **道具**: 読み取り専用の道具をモデルに渡し、必要な事実を自分で引かせる。
//!   手順数と時間に上限を置き、上限に来たら手元の材料で答えさせる。

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::Deserialize;

use super::{
    checked_request_body_with_limit, client, describe_http_failure, extract_json,
    normalized_base_url, read_response_limited, request_limiter, validate_engine, AssistEngine,
};

/// 全文を一度に読ませる要求の上限。ローカルの推論サーバーは文脈長で
/// 自分から断るので、こちらは誤設定で無制限に膨らまない程度に抑える。
pub(super) const LOCAL_LONG_REQUEST_BYTES: usize = 4 * 1024 * 1024;
/// 全文の読み込み（prefill）と思考を含めて待つ。6万字・6万トークン級の
/// 本文を読み、さらに考えさせると180秒では足りないことがある。
pub(super) const LONG_REQUEST_TIMEOUT: Duration = Duration::from_secs(900);

/// どれだけ考えさせるか。
///
/// 判定（根拠の照合・タグの採否）は考えさせ、整形や番号の選択は考えさせない。
/// 一律に考えさせると遅く、一律に止めると判断を誤る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Effort {
    Off,
    Low,
    Medium,
}

impl Effort {
    fn wire(self) -> &'static str {
        match self {
            Effort::Off => "none",
            Effort::Low => "low",
            Effort::Medium => "medium",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CallOptions {
    pub effort: Effort,
    /// 回答と思考を合わせた出力の上限。
    pub max_tokens: u32,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: Option<u32>,
    /// None はサーバーの既定。Strata は既定で presence_penalty を掛ける。
    pub presence_penalty: Option<f32>,
    pub timeout: Duration,
    pub max_request_bytes: usize,
    /// 考える量の上限（トークン）。Strata は上限で思考を閉じて答えに移る。
    /// `reasoning_effort` は訓練上の目安にすぎず、low でも毎回4千トークン
    /// 以上考えた（Qwen3.6, 実作品）。上限を置かないと一回が90秒を超える。
    pub reasoning_budget: Option<u32>,
    /// 先頭の何メッセージが、後の問いと共有する材料か。Strata はその境目の
    /// 状態を固定して残し、同じ本文への二問目で本文を読み直さない。
    /// Qwen3.6 は再帰層を持つため、固定しないと途中までの状態を再利用できない。
    pub shared_prefix_messages: Option<usize>,
}

impl CallOptions {
    /// Qwen3.6 の推奨値（思考あり 0.6/0.95/20、思考なし 0.7/0.8/20）に合わせる。
    /// 形の決まった仕事でも温度0にしない。思考型は温度0で同じ言い回しを
    /// 繰り返しやすい。
    pub fn for_effort(effort: Effort, max_tokens: u32) -> Self {
        let (temperature, top_p) = if effort == Effort::Off {
            (0.7, 0.8)
        } else {
            (0.6, 0.95)
        };
        Self {
            effort,
            max_tokens,
            temperature,
            top_p,
            top_k: Some(20),
            presence_penalty: None,
            timeout: LONG_REQUEST_TIMEOUT,
            max_request_bytes: LOCAL_LONG_REQUEST_BYTES,
            reasoning_budget: None,
            shared_prefix_messages: None,
        }
    }

    /// 温度0。形の決まった仕事は揺らがない方が検査しやすい。思考の上限と
    /// 併せて使う（思考型は温度0で同じ言い回しを繰り返すことがある）。
    ///
    /// Strata の MTP 先読みは、温度0で、かつ出現済みの語に罰を掛けない要求で
    /// だけ働く。Strata は既定で presence_penalty を掛けるので、明示して外す
    /// （実測: 先読み 0/0 → 24/24、56 → 100 tok/s）。
    pub fn greedy(mut self) -> Self {
        self.temperature = 0.0;
        self.presence_penalty = Some(0.0);
        self
    }

    pub fn thinking_budget(mut self, tokens: u32) -> Self {
        self.reasoning_budget = Some(tokens);
        self
    }

    pub fn sharing_first(mut self, messages: usize) -> Self {
        self.shared_prefix_messages = Some(messages);
        self
    }
}

/// モデルに渡す道具の宣言。
#[derive(Debug, Clone)]
pub(super) struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,
}

impl Tool {
    fn wire(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            }
        })
    }
}

pub(super) type ToolFuture = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;

/// 道具の実行者。データベースを読むのは呼び出し側（commands）なので、
/// ここは名前と引数を渡して文字列の結果を受け取るだけにする。
pub(super) trait ToolRunner: Send + Sync {
    fn run(&self, name: &str, arguments: serde_json::Value) -> ToolFuture;
}

/// モデルが何を調べたかの記録。生成物の出どころとして残す。
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolTrace {
    pub name: String,
    pub arguments: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Completion {
    pub content: String,
    pub tool_calls: Vec<WireToolCall>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(super) struct WireToolCall {
    pub id: String,
    #[serde(rename = "type", default = "function_type")]
    pub kind: String,
    pub function: WireFunctionCall,
}

fn function_type() -> String {
    "function".to_string()
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(super) struct WireFunctionCall {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Deserialize)]
struct WireResponse {
    choices: Vec<WireChoice>,
}

#[derive(Deserialize)]
struct WireChoice {
    message: WireMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<WireToolCall>>,
}

pub(super) fn system(text: &str) -> serde_json::Value {
    serde_json::json!({ "role": "system", "content": text })
}

pub(super) fn user(text: &str) -> serde_json::Value {
    serde_json::json!({ "role": "user", "content": text })
}

pub(super) fn assistant(text: &str) -> serde_json::Value {
    serde_json::json!({ "role": "assistant", "content": text })
}

/// 一回だけ送る。会話の組み立てと再試行は呼び出し側が決める。
pub(super) async fn complete(
    engine: &AssistEngine,
    messages: &[serde_json::Value],
    response_format: Option<&serde_json::Value>,
    tools: Option<&[Tool]>,
    forced_tool: Option<&str>,
    options: CallOptions,
) -> Result<Completion, String> {
    validate_engine(engine)?;
    let _permit = tokio::time::timeout(
        Duration::from_secs(30),
        request_limiter(engine, None).acquire_owned(),
    )
    .await
    .map_err(|_| "別のAI処理が続いています。完了後にもう一度実行してください".to_string())?
    .map_err(|_| "推論リクエストの実行枠を確保できません".to_string())?;
    let base_url = normalized_base_url(&engine.base_url)?;
    let mut body = serde_json::json!({
        "model": engine.model,
        "messages": messages,
        "temperature": options.temperature,
        "top_p": options.top_p,
        "max_tokens": options.max_tokens,
        "reasoning_effort": options.effort.wire(),
    });
    if let Some(top_k) = options.top_k {
        body["top_k"] = serde_json::json!(top_k);
    }
    if let Some(penalty) = options.presence_penalty {
        body["presence_penalty"] = serde_json::json!(penalty);
    }
    if let Some(budget) = options
        .reasoning_budget
        .filter(|_| options.effort != Effort::Off)
    {
        body["reasoning_budget_tokens"] = serde_json::json!(budget);
    }
    if let Some(shared) = options
        .shared_prefix_messages
        .filter(|shared| *shared > 0 && *shared < messages.len())
    {
        body["strata_prefix"] = serde_json::json!({ "messages": shared });
    }
    if options.effort == Effort::Off {
        // reasoning_effort を読まないサーバー（llama.cpp 系）向けにも切る。
        body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": false });
    }
    if let Some(format) = response_format {
        body["response_format"] = format.clone();
    }
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        body["tools"] = serde_json::Value::Array(tools.iter().map(Tool::wire).collect());
        if let Some(name) = forced_tool.filter(|name| tools.iter().any(|tool| tool.name == *name)) {
            body["tool_choice"] =
                serde_json::json!({ "type": "function", "function": { "name": name } });
        }
    }
    let (mut status, mut bytes) = post(&base_url, &body, options).await?;
    if !status.is_success() {
        let rejected = rejected_optional_fields(status.as_u16(), &bytes);
        if !rejected.is_empty() {
            // 速さのための項目は他のサーバーに無い。断られたら外して一度だけ送り直す。
            if let Some(object) = body.as_object_mut() {
                for key in rejected {
                    object.remove(key);
                }
            }
            (status, bytes) = post(&base_url, &body, options).await?;
        }
    }
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes);
        if is_structured_output_failure(status.as_u16(), &detail) {
            return Err(STRUCTURED_OUTPUT_FAILED.to_string());
        }
        return Err(describe_http_failure(status.as_u16(), &detail));
    }
    let parsed: WireResponse = serde_json::from_slice(&bytes)
        .map_err(|error| format!("モデルの応答を読めません: {error}"))?;
    let choice = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| "モデルが何も返しませんでした".to_string())?;
    Ok(Completion {
        content: choice.message.content.unwrap_or_default(),
        tool_calls: choice.message.tool_calls.unwrap_or_default(),
        finish_reason: choice.finish_reason,
    })
}

async fn post(
    base_url: &str,
    body: &serde_json::Value,
    options: CallOptions,
) -> Result<(reqwest::StatusCode, Vec<u8>), String> {
    let request = checked_request_body_with_limit(body, options.max_request_bytes)?;
    let response = client(base_url)?
        .post(super::endpoint(base_url, "chat/completions"))
        .timeout(options.timeout)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(request)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                format!(
                    "モデルが{}秒以内に答えませんでした。長い本文では文脈長と速度を確認してください",
                    options.timeout.as_secs()
                )
            } else {
                format!("モデルに接続できません: {error}")
            }
        })?;
    let status = response.status();
    let bytes = read_response_limited(response).await?;
    Ok((status, bytes))
}

/// 外しても答えの意味が変わらない項目。tool_choice 以外は OpenAI 標準に無く、
/// tool_choice も道具の名指しを断るサーバーがある。
const OPTIONAL_FIELDS: [&str; 6] = [
    "strata_prefix",
    "reasoning_budget_tokens",
    "top_k",
    "chat_template_kwargs",
    "reasoning_effort",
    "tool_choice",
];

fn rejected_optional_fields(status: u16, body: &[u8]) -> Vec<&'static str> {
    if !matches!(status, 400 | 422) {
        return Vec::new();
    }
    let detail = String::from_utf8_lossy(body).to_ascii_lowercase();
    if !["unknown", "unsupported", "unrecognized", "extra", "invalid"]
        .iter()
        .any(|marker| detail.contains(marker))
    {
        return Vec::new();
    }
    OPTIONAL_FIELDS
        .into_iter()
        .filter(|field| detail.contains(field))
        .collect()
}

/// Strata はスキーマをサーバーで検査し、合わない応答を 502 で返す。
/// サーバーの故障と区別して、形の直しとして扱う。
const STRUCTURED_OUTPUT_FAILED: &str = "structured_output_failed";

fn is_structured_output_failure(status: u16, detail: &str) -> bool {
    status == 502 && detail.contains(STRUCTURED_OUTPUT_FAILED)
}

/// 決まった形の答えを一つもらう。
///
/// `check` は JSON をアプリの規則で検査する（番号が実在するか、字数に収まるか）。
/// 通らなければ、**理由を添えて同じ会話で一度だけ**出し直させる。黙って
/// 捨てたり切り詰めたりすると、モデルの判断がアプリの都合で変わる。
pub(super) async fn structured<T>(
    engine: &AssistEngine,
    messages: &[serde_json::Value],
    schema: &serde_json::Value,
    options: CallOptions,
    check: impl Fn(serde_json::Value) -> Result<T, String>,
) -> Result<T, String> {
    let mut conversation = messages.to_vec();
    let mut last_error = String::new();
    for attempt in 0..2 {
        let answer = match complete(engine, &conversation, Some(schema), None, None, options).await
        {
            Ok(answer) => answer,
            Err(error) if error == STRUCTURED_OUTPUT_FAILED && attempt == 0 => {
                last_error = "決められた形の JSON になっていませんでした".to_string();
                conversation.push(user(&repair_request(&last_error)));
                continue;
            }
            Err(error) if error == STRUCTURED_OUTPUT_FAILED => {
                return Err("モデルが決められた形で答えられませんでした".to_string())
            }
            Err(error) => return Err(error),
        };
        if answer.content.trim().is_empty()
            && answer
                .finish_reason
                .as_deref()
                .is_some_and(|reason| reason.eq_ignore_ascii_case("length"))
        {
            return Err(format!(
                "モデルが出力上限（{} トークン）を考えるだけで使い切りました",
                options.max_tokens
            ));
        }
        let checked = extract_json(&answer.content)
            .and_then(|json| {
                serde_json::from_str::<serde_json::Value>(json)
                    .map_err(|error| format!("JSON として読めません: {error}"))
            })
            .and_then(&check);
        match checked {
            Ok(value) => return Ok(value),
            Err(error) => {
                last_error = error;
                conversation.push(assistant(&answer.content));
                conversation.push(user(&repair_request(&last_error)));
            }
        }
    }
    Err(format!(
        "モデルの答えを検査に通せませんでした: {last_error}"
    ))
}

fn repair_request(reason: &str) -> String {
    format!(
        "直前の答えは次の理由で使えません: {reason}\n\
         指示と形はそのままに、理由を直した答えを出し直してください。"
    )
}

/// 道具を使わせながら調べさせる。答えの形は決めない（Strata は道具と
/// response_format の併用を断る）。調べ終えたら、呼び出し側が同じ会話の
/// 後ろへ形の決まった指示を足して `structured` で答えさせる。
///
/// 会話は呼び出し側へ返す。調べた中身を、最後の答えの材料として残すため。
pub(super) async fn investigate(
    engine: &AssistEngine,
    conversation: &mut Vec<serde_json::Value>,
    tools: &[Tool],
    runner: &dyn ToolRunner,
    first_tool: Option<&str>,
    options: CallOptions,
    max_steps: usize,
) -> Result<Vec<ToolTrace>, String> {
    let mut traces = Vec::new();
    for step in 0..max_steps {
        // 一手目だけ道具を名指しする。任せると、候補一覧の語を推して確かめる
        // だけで探さなかった（棚の実データ10件で find_tags は0回、3件は何も調べなかった）。
        let forced = first_tool.filter(|_| step == 0);
        let answer = complete(engine, conversation, None, Some(tools), forced, options).await?;
        if answer.tool_calls.is_empty() {
            if !answer.content.trim().is_empty() {
                conversation.push(assistant(&answer.content));
            }
            return Ok(traces);
        }
        conversation.push(serde_json::json!({
            "role": "assistant",
            "content": answer.content,
            "tool_calls": answer.tool_calls,
        }));
        for call in &answer.tool_calls {
            let declared = tools.iter().any(|tool| tool.name == call.function.name);
            let arguments = serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                .unwrap_or(serde_json::Value::Null);
            let result = if !declared {
                Err(format!("{} という道具はありません", call.function.name))
            } else if !arguments.is_object() {
                Err("引数が JSON のオブジェクトではありません".to_string())
            } else {
                runner.run(&call.function.name, arguments).await
            };
            traces.push(ToolTrace {
                name: call.function.name.clone(),
                arguments: clamp_chars(&call.function.arguments, 300),
                ok: result.is_ok(),
            });
            let content = match result {
                Ok(text) => clamp_chars(&text, MAX_TOOL_RESULT_CHARS),
                Err(error) => format!("エラー: {error}"),
            };
            conversation.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": content,
            }));
        }
    }
    // 上限まで調べても終わらない場合は、そこまでの材料で答えさせる。
    conversation.push(user(
        "調べるのはここまでにしてください。ここまでに分かったことだけで答えます。",
    ));
    Ok(traces)
}

/// 一つの道具の結果が文脈を食い尽くさないように。
const MAX_TOOL_RESULT_CHARS: usize = 6_000;

fn clamp_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut clamped = text.chars().take(max_chars).collect::<String>();
    clamped.push_str("…（以下省略）");
    clamped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_output_failure_is_told_apart_from_server_faults() {
        assert!(is_structured_output_failure(
            502,
            r#"{"error":{"code":"structured_output_failed"}}"#
        ));
        assert!(!is_structured_output_failure(502, "bad gateway"));
        assert!(!is_structured_output_failure(
            400,
            "structured_output_failed"
        ));
    }

    #[test]
    fn thinking_and_answer_sampling_follow_effort() {
        let off = CallOptions::for_effort(Effort::Off, 100);
        assert_eq!((off.temperature, off.top_p), (0.7, 0.8));
        let on = CallOptions::for_effort(Effort::Medium, 100);
        assert_eq!((on.temperature, on.top_p), (0.6, 0.95));
        assert_eq!(Effort::Off.wire(), "none");
        // 先読みを止める罰は、温度0の要求でだけ外す。それ以外はサーバーの既定に任せる。
        assert_eq!(on.presence_penalty, None);
        let greedy = on.greedy();
        assert_eq!(
            (greedy.temperature, greedy.presence_penalty),
            (0.0, Some(0.0))
        );
    }

    #[test]
    fn only_fields_the_server_named_are_dropped() {
        assert_eq!(
            rejected_optional_fields(
                400,
                br#"{"error":"Unrecognized request argument: strata_prefix"}"#
            ),
            vec!["strata_prefix"]
        );
        assert!(rejected_optional_fields(400, b"context length exceeded").is_empty());
        assert!(rejected_optional_fields(500, b"unknown strata_prefix").is_empty());
    }

    #[test]
    fn tool_results_are_bounded() {
        let long = "あ".repeat(MAX_TOOL_RESULT_CHARS + 10);
        let clamped = clamp_chars(&long, MAX_TOOL_RESULT_CHARS);
        assert!(clamped.ends_with("…（以下省略）"));
        assert_eq!(clamp_chars("短い", 10), "短い");
    }
}
