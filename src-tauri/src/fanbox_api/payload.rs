use serde_json::Value;

const WRAPPER_KEYS: [&str; 4] = ["post", "data", "result", "body"];
const MAX_WRAPPER_DEPTH: usize = 8;

fn looks_like_post(value: &Value) -> bool {
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
pub(crate) fn post_ref(mut value: &Value) -> Option<&Value> {
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

pub(crate) fn post_or_self(value: &Value) -> &Value {
    post_ref(value).unwrap_or(value)
}

pub(crate) fn post_mut_or_self(value: &mut Value) -> &mut Value {
    if post_ref(value).is_none() {
        return value;
    }
    fn descend(value: &mut Value, depth: usize) -> &mut Value {
        if depth >= MAX_WRAPPER_DEPTH || looks_like_post(value) {
            return value;
        }
        let key = value.as_object().and_then(|object| {
            WRAPPER_KEYS
                .iter()
                .copied()
                .find(|key| object.get(*key).is_some_and(Value::is_object))
        });
        match key {
            Some(key) => {
                let nested = value
                    .as_object_mut()
                    .and_then(|object| object.get_mut(key))
                    .expect("wrapper key was checked above");
                descend(nested, depth + 1)
            }
            None => value,
        }
    }
    descend(value, 0)
}

pub(crate) fn into_post(mut value: Value) -> Option<Value> {
    for _ in 0..=MAX_WRAPPER_DEPTH {
        if looks_like_post(&value) {
            return Some(value);
        }
        let key = value.as_object().and_then(|object| {
            WRAPPER_KEYS
                .iter()
                .copied()
                .find(|key| object.get(*key).is_some_and(Value::is_object))
        })?;
        value = value.as_object_mut()?.remove(key)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn post() -> Value {
        json!({
            "id": "5106430",
            "title": "アンケート結果",
            "creatorId": "ponkan",
            "body": { "text": "本文" }
        })
    }

    #[test]
    fn accepts_direct_legacy_and_current_post_shapes() {
        for value in [
            post(),
            json!({ "body": post() }),
            json!({ "body": { "post": post() } }),
            json!({ "data": { "result": { "body": { "post": post() } } } }),
        ] {
            assert_eq!(
                post_ref(&value).and_then(|p| p["id"].as_str()),
                Some("5106430")
            );
        }
    }

    #[test]
    fn does_not_mistake_a_post_body_for_a_post() {
        let value = json!({ "body": { "text": "本文" } });
        assert!(post_ref(&value).is_none());
        assert_eq!(post_or_self(&value), &value);
    }

    #[test]
    fn mutable_and_owned_helpers_reach_the_same_post() {
        let mut wrapped = json!({ "body": { "post": post() } });
        post_mut_or_self(&mut wrapped)["localCoverPath"] = json!("./cover.jpg");
        assert_eq!(post_ref(&wrapped).unwrap()["localCoverPath"], "./cover.jpg");

        let owned = into_post(wrapped).unwrap();
        assert_eq!(owned["id"], "5106430");
        assert_eq!(owned["localCoverPath"], "./cover.jpg");
    }
}
