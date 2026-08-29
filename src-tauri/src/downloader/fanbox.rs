use crate::fanbox_api::client::FanboxAPI;
use std::error::Error;

/// 投稿の詳細を取得する（新しい fanbox_api クライアントを使用）
pub async fn get_post_detail(
    post_id: &str,
    cookie: &str,
    user_agent: &str,
) -> Result<serde_json::Value, Box<dyn Error>> {
    log::info!("Fetching post detail for {} via FanboxAPI Client", post_id);
    let api = FanboxAPI::new(cookie.to_string(), user_agent.to_string())?;

    match api.get_post_value(post_id).await {
        Ok(post) => Ok(post),
        Err(e) => {
            // 文面のほうを出す。`{:?}` は応答の本文まで書き出すが、
            // `FanboxError` の文面は**それを混ぜないように書いてある**。
            // 記録だけ設計を裏切っていた。
            log::error!("Failed to get Fanbox post: {e}");
            Err(Box::new(e))
        }
    }
}
