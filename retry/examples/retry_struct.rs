#![allow(dead_code)]

use std::future::{Future, ready, Ready};

use retry::RetryPolicy;

#[derive(Debug)]
enum Error {
    Retryable,
    Fatal,
}

struct MyTask {
    count: u32,
}

impl retry::Task for MyTask {
    type Item = ();
    type Error = Error;
    type Fut = Ready<Result<Self::Item, Self::Error>>;

    fn call(&mut self) -> Self::Fut {
        self.count += 1;

        tracing::info!("MyTask call() {}", self.count);

        if self.count < 3 {
            ready(Err(Error::Retryable))
        } else {
            ready(Err(Error::Fatal))
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_init::init();

    let policy = RetryPolicy::default();
    let my_task = MyTask { count: 0 };

    let result = policy.retry_if(
        my_task,
        |err: &Error| match err {
            Error::Retryable => true,
            Error::Fatal => false,
        },
    ).await;

    tracing::info!("{result:?}");
}
