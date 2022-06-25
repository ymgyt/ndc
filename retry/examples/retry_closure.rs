#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use retry::RetryPolicy;

#[derive(Debug)]
enum Error {
    Retryable,
    Fatal,
}

#[tokio::main]
async fn main() {
    tracing_init::init();

    let policy = RetryPolicy::exponential(Duration::from_secs(1))
        .with_jitter(false)
        .with_backoff_exponent(2.)
        .with_max_delay(Duration::from_secs(60))
        .with_max_retries(4);

    let retry_count = Arc::new(AtomicU64::new(0));

    let result = policy.retry_if(
         ||  {
            let retry_count = Arc::clone(&retry_count);
            async move {
                retry_count.fetch_add(1, Ordering::SeqCst);

                tracing::info!(?retry_count, "task run!");

                Err::<(), Error>(Error::Retryable)
            }
        },
        |err: &Error| match err {
            Error::Retryable => true,
            Error::Fatal => false,
        },
    ).await;

    tracing::info!("{result:?} {retry_count:?}");
}
