use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// --- 共通レスポンスラッパー ---
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FanboxResponse<T> {
    pub body: T,
}

// --- ユーザー情報 ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxUser {
    pub user_id: String,
    pub name: String,
    pub icon_url: Option<String>,
}

// --- クリエイタープロフィール詳細 ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxCreator {
    pub creator_id: String,
    pub description: String,
    pub cover_image_url: Option<String>,
    pub has_adult_content: bool,
    pub is_followed: bool,
    pub is_stopped: bool,
    pub is_supported: bool,
    pub user: FanboxUser,
    pub profile_links: Vec<String>,
    pub profile_items: serde_json::Value, // さまざまなプロフィールのアイテム（動的）
}

// --- 支援プラン ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxPlan {
    pub id: String,
    pub title: String,
    pub fee: u32,
    pub description: String,
    pub cover_image_url: Option<String>,
    pub creator_id: String,
    pub has_adult_content: bool,
}

// --- 投稿（Post） ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxPost {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub fee_required: u32,
    #[serde(default)]
    pub excerpt: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,

    // 🛠️【修正1】一覧APIでは "type" が返ってこないため Option に変更
    #[serde(rename = "type")]
    pub post_type: Option<String>,

    // 🛠️【修正2】一覧APIでは "cover" で返ってくることがあるため追加
    pub cover_image_url: Option<String>,
    pub cover: Option<serde_json::Value>,

    pub creator_id: String,
    #[serde(default)]
    pub has_adult_content: bool,
    #[serde(default)]
    pub is_restricted: bool,
    pub published_datetime: String,
    pub updated_datetime: String,
    #[serde(default)]
    pub comment_count: u32,
    #[serde(default)]
    pub like_count: u32,
    pub body: Option<serde_json::Value>, // 投稿本文（投稿タイプによって異なる）
    pub user: Option<FanboxUser>,
    pub next_post: Option<FanboxPostRef>,
    pub prev_post: Option<FanboxPostRef>,
}

// 関連する投稿の参照（Next/Prevなど）
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxPostRef {
    pub id: String,
    pub title: String,
    pub published_datetime: String,
}

// --- 投稿本文の解析用（Article型などの詳細パース用） ---

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxArticleBody {
    pub blocks: Vec<serde_json::Value>,
    pub image_map: Option<HashMap<String, FanboxArticleImage>>,
    pub file_map: Option<HashMap<String, FanboxArticleFile>>,
    pub url_embed_map: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxArticleImage {
    pub id: String,
    pub extension: String,
    pub width: u32,
    pub height: u32,
    pub original_url: String,
    pub thumbnail_url: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxArticleFile {
    pub id: String,
    pub name: String,
    pub extension: String,
    pub size: u64,
    pub url: String,
}

// --- ファイル型投稿の本文 ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxFileBody {
    pub text: String,
    pub files: Vec<FanboxArticleFile>,
}

// --- 画像型投稿の本文 ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxImageBody {
    pub text: String,
    pub images: Vec<FanboxArticleImage>,
}

// --- リスト表示 / ページネーション用 ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxPostList {
    #[serde(default, alias = "posts")]
    pub items: Vec<FanboxPost>,
    #[serde(default)]
    pub next_url: Option<String>,
}

// 🛠️【新規追加】APIの気まぐれなデータ構造（配列 or オブジェクト）を吸収する柔軟なEnum
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)] // JSONの形状に合わせて自動的にどちらかの型に割り振るSerdeマジック
pub enum FlexiblePostList {
    Object(FanboxPostList), // パターンB: {"items": [...], "nextUrl": "..."}
    Array(Vec<FanboxPost>), // パターンA: [{"id": "..."}, ...]
}

// 🛠️【新規追加】FlexiblePostList を包む全体レスポンスラッパー
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FlexibleResponse {
    pub body: FlexiblePostList,
}

// クリエイター投稿のページネーション情報
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FanboxPaginatedCreatorPosts {
    pub body: serde_json::Value, // 各ページのAPI URLリスト（極めて堅牢にパースするためにValueに変更）
}

#[cfg(test)]
mod tests {
    use super::FanboxPostList;

    #[test]
    fn post_list_accepts_current_posts_field() {
        let list: FanboxPostList = serde_json::from_value(serde_json::json!({
            "posts": [],
            "nextUrl": "https://api.fanbox.cc/post.listCreator?page=2"
        }))
        .expect("current FANBOX creator list shape should deserialize");

        assert!(list.items.is_empty());
        assert_eq!(
            list.next_url.as_deref(),
            Some("https://api.fanbox.cc/post.listCreator?page=2")
        );
    }
}
