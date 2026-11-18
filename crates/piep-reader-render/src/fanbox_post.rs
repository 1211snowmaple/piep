//! FANBOX の投稿を、どの包み方で保存されていても取り出す。

use serde_json::Value;

pub const WRAPPER_KEYS: [&str; 4] = ["post", "data", "result", "body"];
pub const MAX_WRAPPER_DEPTH: usize = 8;

pub fn looks_like_post(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.contains_key("id")
        && (object.contains_key("title")
            || object.contains_key("creatorId")
            || object.contains_key("publishedDatetime")
            || object.contains_key("type"))
}

/// Return the actual FANBOX post regardless of which API/archive wrapper surrounds it.
///
/// FANBOX has returned both `{ body: POST }` and `{ body: { post: POST } }` over
/// time. Imported archives can additionally contain frontend-style `data` or
/// `result` wrappers. Keeping this in one place prevents readers, search, EPUB and
/// the downloader from disagreeing about the same saved JSON.
pub fn post_ref(mut value: &Value) -> Option<&Value> {
    for _ in 0..=MAX_WRAPPER_DEPTH {
        if looks_like_post(value) {
            return Some(value);
        }
        let object = value.as_object()?;
        value = WRAPPER_KEYS
            .iter()
            .find_map(|key| object.get(*key).filter(|nested| nested.is_object()))?;
    }
    None
}

pub fn post_or_self(value: &Value) -> &Value {
    post_ref(value).unwrap_or(value)
}
