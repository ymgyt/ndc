use std::cmp::min;
use std::future::Future;
use std::time::Duration;

use wasm_timer::Delay;

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
    RetryPolicy::default().retry_if(task,condition).await
}

#[derive(Clone,Copy)]
enum Backoff {
    Fixed,
    Exponential { exponent: f64 },
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff::Exponential { exponent: Backoff::DEFAULT_EXPONENT }
    }
}

impl Backoff {
    const DEFAULT_EXPONENT: f64 = 2.0;

    fn iter(
        self,
        policy: &RetryPolicy,
    ) -> BackoffIter {
       BackoffIter {
           backoff: self,
           current: 1.0,
           #[cfg(feature = "rand")]
           jitter: policy.jitter,
           delay: policy.delay,
           max_delay: policy.max_delay,
           max_retries: policy.max_retries,
       }
    }
}

struct BackoffIter {
    backoff: Backoff,
    current: f64,
    #[cfg(feature = "rand")]
    jitter: bool,
    delay: Duration,
    max_delay: Option<Duration>,
    max_retries: usize,
}

impl Iterator for BackoffIter {
    type Item = Duration;
    fn next(&mut self) -> Option<Self::Item> {
        if self.max_retries > 0 {
            let factor = match self.backoff {
                Backoff::Fixed => self.current,
                Backoff::Exponential { exponent } => {
                    let factor = self.current;
                    let next_factor = self.current * exponent;
                    self.current = next_factor;
                    factor
                }
            };

            let mut delay = self.delay.mul_f64(factor);
            #[cfg(feature = "rand")]
                {
                    if self.jitter {
                        delay = jitter(delay);
                    }
                }
            if let Some(max_delay) = self.max_delay {
                delay = min(delay, max_delay);
            }
            self.max_retries -= 1;

            return Some(delay);
        }
        None
    }
}

#[cfg(feature = "rand")]
fn jitter(duration: Duration) -> Duration {
    // TODO:
    duration
}

/// A template for configuring retry behavior
#[derive(Clone)]
pub struct RetryPolicy {
   backoff: Backoff,
    #[cfg(feature = "rand")]
    jitter: bool,
    delay: Duration,
    max_delay: Option<Duration>,
    max_retries: usize,
}

impl Default for RetryPolicy {
    fn default() -> Self {
       Self {
           backoff: Backoff::default(),
           delay: Duration::from_secs(1),
           #[cfg(feature = "rand")]
           jitter: false,
           max_delay: None,
           max_retries: 5,
       }
    }
}

impl RetryPolicy {
    fn backoffs(&self) -> impl Iterator<Item = Duration> {
        self.backoff.iter(self)
    }

    pub fn exponential(delay: Duration) -> Self {
        Self {
            backoff: Backoff::Exponential { exponent: Backoff::DEFAULT_EXPONENT },
            delay,
            ..Self::default()
        }
    }

    pub async fn retry<T>(
        &self,
        task: T,
    ) -> Result<T::Item, T::Error>
    where
        T: Task,
    {
        self::retry_if(task, Always).await
    }

    pub async fn retry_if<T,C>(
        &self,
        task: T,
        condition: C,
    ) -> Result<T::Item, T::Error>
    where
        T: Task,
        C: Condition<T::Error>,
    {
        let mut backoffs = self.backoffs();
        let mut task = task;
        let mut condition = condition;
        loop {
            return match task.call().await {
                Ok(result) => Ok(result),
                Err(err) => {
                    if condition.is_retryable(&err) {
                        // Backoff has two responsibilities.
                        //   * Control whether to retry or not.
                        //     backoff iter take care of max_retry policy.
                        //   * If it does, control the duration of the delay.
                        if let Some(delay) = backoffs.next() {
                            tracing::trace!(
                               "task failed with error {err:?}. will try again in  {delay:?}"
                           );
                            let _ = Delay::new(delay).await;
                            continue;
                        }
                    }
                    Err(err)
                }
            }
        }
    }
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
    use approx::assert_relative_eq;
    use std::error::Error;

    #[test]
    fn retry_policy_is_send() {
        fn test(_: impl Send) {}
        test(RetryPolicy::default())
    }

    #[test]
    fn backoff_default() {
        if let Backoff::Exponential { exponent } = Backoff::default() {
           assert_relative_eq!(exponent, 2.0);
        } else {
            panic!("Default backoff expected to be exponential");
        }
    }

    #[test]
    fn exponential_backoff() {
        let mut iter = RetryPolicy::exponential(Duration::from_secs(1)).backoffs();
        assert_relative_eq!(iter.next().unwrap().as_secs_f64(), 1.0);
        assert_relative_eq!(iter.next().unwrap().as_secs_f64(), 2.0);
        assert_relative_eq!(iter.next().unwrap().as_secs_f64(), 4.0);
        assert_relative_eq!(iter.next().unwrap().as_secs_f64(), 8.0);
        assert_relative_eq!(iter.next().unwrap().as_secs_f64(), 16.0);
    }

    #[tokio::test]
    async fn task_implemented_for_closure() {
        let f = || async { Ok::<u32, u32>(10) };

        retry(f).await.unwrap();
    }

    #[tokio::test]
    async fn ok_futures_yield_ok() -> Result<(), Box<dyn Error>> {
        let result = RetryPolicy::default()
            .retry(|| async { Ok::<u32,()>(34)})
            .await;
        assert_eq!(result, Ok(34));
        Ok(())
    }
}
