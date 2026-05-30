use crate::fanbox_api::client::FanboxAPI;
pub use crate::fanbox_api::models::FanboxPost;
use std::error::Error;

/// 投稿の詳細を取得する（新しい fanbox_api クライアントを使用）
pub async fn get_post_detail(
    post_id: &str,
    cookie: &str,
    user_agent: &str,
) -> Result<FanboxPost, Box<dyn Error>> {
    log::info!("Fetching post detail for {} via FanboxAPI Client", post_id);
    let api = FanboxAPI::new(cookie.to_string(), user_agent.to_string());

    match api.get_post(post_id).await {
        Ok(post) => Ok(post),
        Err(e) => {
            log::error!("Failed to get Fanbox post: {:?}", e);
            Err(Box::new(e))
        }
    }
}
