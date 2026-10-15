//! A shared API gate per provider also covers competing jobs and pagination.
use std::{sync::LazyLock, time::Duration};
use tokio::{sync::Mutex, time::Instant};

pub struct ApiGate(Mutex<Instant>);
impl ApiGate {
    fn new() -> Self {
        Self(Mutex::new(Instant::now()))
    }

    pub async fn wait(&self) {
        // Keep the lock while waiting: cancellation doesn't reserve future slots.
        let mut next = self.0.lock().await;
        tokio::time::sleep_until(*next).await;
        *next = Instant::now() + Duration::from_millis(800);
    }

    pub async fn defer(&self, delay: Duration) {
        let mut next = self.0.lock().await;
        *next = (*next).max(Instant::now() + delay);
    }
}

pub static PIXIV: LazyLock<ApiGate> = LazyLock::new(ApiGate::new);
pub static FANBOX: LazyLock<ApiGate> = LazyLock::new(ApiGate::new);

pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Duration {
    let seconds = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.parse::<u64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(value)
                    .ok()
                    .map(|until| {
                        (until.with_timezone(&chrono::Utc) - chrono::Utc::now())
                            .num_seconds()
                            .max(0) as u64
                    })
            })
        })
        .unwrap_or(60);
    Duration::from_secs(seconds.max(5))
}
