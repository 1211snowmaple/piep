//! Clients live for one worker, never in a process-wide credential cache.
//! The task-local scope includes pagination, detail/text and profile requests.
use std::{cell::RefCell, future::Future, sync::Arc};

use crate::{
    fanbox_api::{client::FanboxAPI, error::FanboxError},
    pixiv_api::{aapi::AppPixivAPI, error::PixivError},
};

#[derive(Default)]
struct Clients {
    pixiv: Option<(String, Arc<AppPixivAPI>)>,
    fanbox: Option<(String, String, Arc<FanboxAPI>)>,
    fees: Option<Result<std::collections::HashMap<String, u64>, String>>,
}

tokio::task_local! {
    static CLIENTS: RefCell<Clients>;
}

pub async fn scope<F: Future>(future: F) -> F::Output {
    CLIENTS
        .scope(RefCell::new(Clients::default()), future)
        .await
}

pub fn pixiv(token: String) -> Result<Arc<AppPixivAPI>, PixivError> {
    CLIENTS
        .try_with(|slot| {
            let mut clients = slot.borrow_mut();
            if let Some((key, api)) = &clients.pixiv {
                if key == &token {
                    return Ok(api.clone());
                }
            }
            let api = Arc::new(AppPixivAPI::new_from_refresh_token(token.clone())?);
            clients.pixiv = Some((token.clone(), api.clone()));
            Ok(api)
        })
        .unwrap_or_else(|_| AppPixivAPI::new_from_refresh_token(token).map(Arc::new))
}

pub fn fanbox(cookie: String, user_agent: String) -> Result<Arc<FanboxAPI>, FanboxError> {
    CLIENTS
        .try_with(|slot| {
            let mut clients = slot.borrow_mut();
            if let Some((key, ua, api)) = &clients.fanbox {
                if key == &cookie && ua == &user_agent {
                    return Ok(api.clone());
                }
            }
            let api = Arc::new(FanboxAPI::new(cookie.clone(), user_agent.clone())?);
            clients.fanbox = Some((cookie.clone(), user_agent.clone(), api.clone()));
            clients.fees = None;
            Ok(api)
        })
        .unwrap_or_else(|_| FanboxAPI::new(cookie, user_agent).map(Arc::new))
}

pub async fn supporting_fees(
    cookie: String,
    ua: String,
) -> Result<std::collections::HashMap<String, u64>, String> {
    let api = fanbox(cookie, ua).map_err(|e| e.to_string())?;
    if let Some(cached) = CLIENTS
        .try_with(|slot| slot.borrow().fees.clone())
        .ok()
        .flatten()
    {
        return cached;
    }
    let result = api.supporting_fees().await.map_err(|e| e.to_string());
    let _ = CLIENTS.try_with(|slot| slot.borrow_mut().fees = Some(result.clone()));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clients_are_reused_only_within_the_same_job_and_credentials() {
        let first = scope(async {
            let first = pixiv("a".into()).unwrap();
            assert!(Arc::ptr_eq(&first, &pixiv("a".into()).unwrap()));
            assert!(!Arc::ptr_eq(&first, &pixiv("b".into()).unwrap()));
            let fan = fanbox("session".into(), "ua".into()).unwrap();
            assert!(Arc::ptr_eq(
                &fan,
                &fanbox("session".into(), "ua".into()).unwrap()
            ));
            assert!(!Arc::ptr_eq(
                &fan,
                &fanbox("session".into(), "new ua".into()).unwrap()
            ));
            first
        })
        .await;
        scope(async {
            assert!(!Arc::ptr_eq(&first, &pixiv("a".into()).unwrap()));
        })
        .await;
    }
}
