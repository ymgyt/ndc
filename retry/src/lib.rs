use std::cmp::min;
use std::future::Future;
use std::time::Duration;

use rand::{distributions::OpenClosed01, thread_rng, Rng};
use wasm_timer::Delay;

pub async fn retry<T>(task: T) -> Result<T::Item, T::Error>
    where
        T: Task,
{
    retry_if(task, Always).await
}

pub async fn retry_if<T, C>(task: T, condition: C) -> Result<T::Item, T::Error>
    where
        T: Task,
        C: Condition<T::Error>,
{
    RetryPolicy::default().retry_if(task, condition).await
}

#[derive(Clone, Copy)]
enum Backoff {
    Fixed,
    Exponential { exponent: f64 },
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff::Exponential {
            exponent: Backoff::DEFAULT_EXPONENT,
        }
    }
}

impl Backoff {
    const DEFAULT_EXPONENT: f64 = 2.0;

    fn iter(self, policy: &RetryPolicy) -> BackoffIter {
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
    let jitter: f64 = thread_rng().sample(OpenClosed01);
    let secs = (duration.as_secs() as f64) * jitter;
    let nanos = (duration.subsec_nanos() as f64) * jitter;
    let millis = (secs * 1_000_f64) + (nanos / 1_000_000_f64);
    Duration::from_millis(millis as u64)
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
    fn backoffs(&self) -> impl Iterator<Item=Duration> {
        self.backoff.iter(self)
    }

    pub fn exponential(delay: Duration) -> Self {
        Self {
            backoff: Backoff::Exponential {
                exponent: Backoff::DEFAULT_EXPONENT,
            },
            delay,
            ..Self::default()
        }
    }

    pub fn fixed(delay: Duration) -> Self {
        Self {
            backoff: Backoff::Fixed,
            delay,
            ..Self::default()
        }
    }

    pub fn with_backoff_exponent(mut self, exp: f64) -> Self {
        if let Backoff::Exponential { ref mut exponent } = self.backoff {
            *exponent = exp
        }
        self
    }

    pub fn with_jitter(mut self, jitter: bool) -> Self {
        self.jitter = jitter;
        self
    }

    pub fn with_max_delay(mut self, max: Duration) -> Self {
        self.max_delay = Some(max);
        self
    }

    pub fn with_max_retries(mut self, max: usize) -> Self {
        self.max_retries = max;
        self
    }

    pub async fn retry<T>(&self, task: T) -> Result<T::Item, T::Error>
        where
            T: Task,
    {
        self::retry_if(task, Always).await
    }

    pub async fn retry_if<T, C>(&self, task: T, condition: C) -> Result<T::Item, T::Error>
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
            };
        }
    }

    pub async fn collect<T, C, S>(
        &self,
        task: T,
        condition: C,
        start_value: S,
    ) -> Result<Vec<T::Item>, T::Error>
        where
            T: TaskWithParameter<S>,
            C: SuccessCondition<T::Item, S>,
    {
        let mut backoffs = self.backoffs();
        let mut condition = condition;
        let mut task = task;
        let mut results = vec![];
        let mut input = start_value;

        loop {
            return match task.call(input).await {
                Ok(item) => {
                    let maybe_new_input = condition.retry_with(&item);
                    results.push(item);

                    if let Some(new_input) = maybe_new_input {
                        if let Some(delay) = backoffs.next() {
                            tracing::trace!(
                                "task succeeded and condition is met. will run again in {delay:?}"
                            );

                            let _ = Delay::new(delay).await;
                            input = new_input;
                            continue;
                        }
                    }

                    // condition indicate to stop collect or max retries exceeded.
                    Ok(results)
                }
                Err(err) => Err(err),
            };
        }
    }

    pub async fn collect_and_retry<T, C, D, S>(
        &self,
        task: T,
        success_condition: C,
        error_condition: D,
        start_value: S,
    ) -> Result<Vec<T::Item>, T::Error>
        where
            T: TaskWithParameter<S>,
            C: SuccessCondition<T::Item, S>,
            D: Condition<T::Error>,
            S: Clone,
    {
        let mut success_backoffs = self.backoffs();
        let mut error_backoffs = self.backoffs();
        let mut success_condition = success_condition;
        let mut error_condition = error_condition;
        let mut task = task;
        let mut results = vec![];
        let mut input = start_value.clone();
        let mut last_result = start_value;

        loop {
            return match task.call(input).await {
                Ok(item) => {
                    let maybe_new_input = success_condition.retry_with(&item);
                    results.push(item);

                    if let Some(new_input) = maybe_new_input {
                        if let Some(delay) = success_backoffs.next() {
                            tracing::trace!(
                                "task succeeded and condition is met. will run again in {delay:?}"
                            );
                            let _ = Delay::new(delay).await;
                            input = new_input.clone();
                            last_result = new_input;
                            continue;
                        }
                    }

                    Ok(results)
                }
                Err(err) => {
                    if error_condition.is_retryable(&err) {
                        if let Some(delay) = error_backoffs.next() {
                            tracing::trace!(
                                "task failed with error {err:?}. will retry again in {delay:?}"
                            );
                            let _ = Delay::new(delay).await;
                            input = last_result.clone();
                            continue;
                        }
                    }
                    Err(err)
                }
            };
        }
    }
}

/// A type to determine if a failed Future should be retried
/// A implementation is provided for `Fn(&Err) -> bool` allowing you
/// to use a simple closure or fn handles
pub trait Condition<E> {
    fn is_retryable(&mut self, error: &E) -> bool;
}

struct Always;

impl<E> Condition<E> for Always {
    fn is_retryable(&mut self, _: &E) -> bool {
        true
    }
}

impl<F, E> Condition<E> for F
    where
        F: FnMut(&E) -> bool,
{
    fn is_retryable(&mut self, error: &E) -> bool {
        self(error)
    }
}

/// A type to determine if a successful Future should be retried
/// A implementation is provided for `Fn(&Result) -> Option<S>`, where S
/// represents the next input value.
pub trait SuccessCondition<R, S> {
    fn retry_with(&mut self, result: &R) -> Option<S>;
}

impl<F, R, S> SuccessCondition<R, S> for F
    where
        F: Fn(&R) -> Option<S>,
{
    fn retry_with(&mut self, result: &R) -> Option<S> {
        self(result)
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

/// A unit of work to be retried, that accepts a parameter
/// A implementation is provided for `FnMut() -> Future`
pub trait TaskWithParameter<P> {
    type Item;
    type Error: std::fmt::Debug;
    type Fut: Future<Output=Result<Self::Item, Self::Error>>;
    fn call(&mut self, parameter: P) -> Self::Fut;
}

impl<F, P, Fut, I, E> TaskWithParameter<P> for F
    where
        F: FnMut(P) -> Fut,
        Fut: Future<Output=Result<I, E>>,
        E: std::fmt::Debug,
{
    type Item = I;
    type Error = E;
    type Fut = Fut;
    fn call(&mut self, parameter: P) -> Self::Fut {
        self(parameter)
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
    fn jitter_adds_variance_to_durations() {
        assert_ne!(jitter(Duration::from_secs(1)), Duration::from_secs(1))
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

    #[test]
    fn always_is_always_retryable() {
        assert!(Always.is_retryable(&()))
    }

    #[test]
    fn closures_impl_condition() {
        fn test(_: impl Condition<()>) {}
        fn foo(_err: &()) -> bool {
            true
        }
        test(foo);
        test(|_err: &()| true);
    }

    #[test]
    fn closures_impl_task() {
        fn test(_: impl Task) {}
        async fn foo() -> Result<u32, ()> {
            Ok(34)
        }
        test(foo);
        test(|| async { Ok::<u32, ()>(34) });
    }

    #[test]
    fn closures_impl_task_with_param() {
        fn test<P>(_: impl TaskWithParameter<P>) {}
        async fn foo(p: u32) -> Result<u32, ()> {
            Ok(p + 1)
        }
        test(foo);
        test(|p: u32| async move { Ok::<u32, ()>(p + 1) })
    }

    #[tokio::test]
    async fn task_implemented_for_closure() {
        let f = || async { Ok::<u32, u32>(10) };

        retry(f).await.unwrap();
    }

    #[test]
    fn retried_futures_are_send_when_tasks_are_send() {
        fn test(_: impl Send) {}
        test(RetryPolicy::default().retry(|| async { Ok::<u32, ()>(34) }))
    }

    #[tokio::test]
    async fn collect_retries_when_condition_is_met() -> Result<(), Box<dyn Error>> {
        let result = RetryPolicy::fixed(Duration::from_millis(1))
            .collect(
                |input: u32| async move { Ok::<u32, ()>(input + 1) },
                |result: &u32| if *result < 2 { Some(*result) } else { None },
                0_u32,
            )
            .await;
        assert_eq!(result, Ok(vec![1, 2]));
        Ok(())
    }

    #[tokio::test]
    async fn collect_does_not_retry_when_condition_is_not_met() -> Result<(), Box<dyn Error>> {
        let result = RetryPolicy::fixed(Duration::from_millis(1))
            .collect(
                |input: u32| async move { Ok::<u32, ()>(input + 1) },
                |result: &u32| if *result < 1 { Some(*result) } else { None },
                0_u32,
            )
            .await;
        assert_eq!(result, Ok(vec![1]));
        Ok(())
    }

    #[tokio::test]
    async fn collect_and_retry_retries_when_success_condition_is_met() -> Result<(), Box<dyn Error>>
    {
        let result = RetryPolicy::fixed(Duration::from_millis(1))
            .collect_and_retry(
                |input: u32| async move { Ok::<u32, u32>(input + 1) },
                |result: &u32| if *result < 2 { Some(*result) } else { None },
                |err: &u32| *err > 1,
                0 as u32,
            )
            .await;
        assert_eq!(result, Ok(vec![1, 2]));
        Ok(())
    }

    #[tokio::test]
    async fn collect_and_retry_does_not_retry_when_success_condition_is_not_met() -> Result<(), Box<dyn Error>> {
        let result = RetryPolicy::fixed(Duration::from_millis(1))
            .collect_and_retry(
                |input: u32| async move { Ok::<u32, u32>(input + 1) },
                |result: &u32| if *result < 1 { Some(*result) } else { None },
                |err: &u32| *err > 1,
                0 as u32,
            )
            .await;
        assert_eq!(result, Ok(vec![1]));
        Ok(())
    }

    #[tokio::test]
    async fn collect_and_retry_retries_when_error_condition_is_met() -> Result<(), Box<dyn Error>> {
        let mut task_ran = 0;
        let _ = RetryPolicy::fixed(Duration::from_millis(1))
            .collect_and_retry(
                |_input: u32| {
                    task_ran += 1;
                    async move { Err::<u32, u32>(0) }
                },
                |result: &u32| if *result < 2 { Some(*result) } else { None },
                |err: &u32| *err == 0,
                0 as u32,
            )
            .await;
        // Default for retry policy is 5, so we end up with the task being
        // retries 5 times and being run 6 times.
        assert_eq!(task_ran, 6);
        Ok(())
    }

    #[tokio::test]
    async fn collect_and_retry_does_not_retry_when_error_condition_is_not_met() -> Result<(), Box<dyn Error>> {
        let result = RetryPolicy::fixed(Duration::from_millis(1))
            .collect_and_retry(
                |input: u32| async move { Err::<u32, u32>(input + 1) },
                |result: &u32| if *result < 1 { Some(*result) } else { None },
                |err: &u32| *err > 1,
                0 as u32,
            )
            .await;
        assert_eq!(result, Err(1));
        Ok(())
    }

    #[tokio::test]
    async fn ok_futures_yield_ok() -> Result<(), Box<dyn Error>> {
        let result = RetryPolicy::default()
            .retry(|| async { Ok::<u32, ()>(34) })
            .await;
        assert_eq!(result, Ok(34));
        Ok(())
    }
}
