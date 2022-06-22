use std::future::Future;

pub async fn retry<T>(task: T) -> Result<T::Item, T::Error>
    where
        T: Task,
{
    retry_if(task, Always).await
}

pub async fn retry_if<T,C>(
    task: T,
    condition: C,
)  -> Result<T::Item, T::Error>
where
    T: Task,
    C: Condition<T::Error>,

{
    // RetryPolicy::default().retry_if(task,condition).await
    todo!()
}

/// A type to determine if a failed Future should be retried
/// A implementation is provided for `Fn(&Err) -> bool` allowing yout
/// to use a simple closure or fn handles
pub trait Condition<E> {
   fn is_retryable(
       &mut self,
       error: &E,
   ) -> bool;
}

struct Always;

impl<E> Condition<E> for Always {
    fn is_retryable(&mut self, _: &E) -> bool {
        true
    }
}

impl<F,E> Condition<E> for F
where F: FnMut(&E) -> bool,
{
    fn is_retryable(&mut self, error: &E) -> bool {
        self(error)
    }
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
