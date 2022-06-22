use std::future::Future;

pub async fn retry<T>(task: T) -> Result<T::Item, T::Error>
    where
        T: Task,
{
    let mut task = task;
    task.call().await
}

/// A unit of work to be retried
/// A implementation is provided for `FnMut() -> Future`
pub trait Task {
    type Item;
    type Error: std::fmt::Debug;
    type Fut: Future<Output=Result<Self::Item, Self::Error>>;

    fn call(&mut self) -> Self::Fut;
}

impl<F, Fut, I, E> Task for F
    where
        F: FnMut() -> Fut,
        Fut: Future<Output=Result<I, E>>,
        E: std::fmt::Debug,
{
    type Item = I;
    type Error = E;
    type Fut = Fut;

    fn call(&mut self) -> Self::Fut {
        self()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_implemented_for_closure() {
        let f = || async { Ok::<u32, u32>(10) };

        retry(f).await.unwrap();
    }
}
