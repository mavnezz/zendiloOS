//! Waiting for a turn: for a place to run, or for a lock another operation
//! holds. A request waits only while its client still waits for the answer:
//! not once it hung up, not past half the time it said it would wait (what
//! it asked for needs time left to run and be answered), and not once the
//! agent is stopping. A request that does not get its turn is answered so,
//! and never carried out later behind its client's back.
//!
//! Only waits before anything was changed go through here; a step in the
//! middle of an operation waits as long as it takes.

use std::future::Future;

use tokio::sync::watch;
use tokio::time::Instant;

use crate::operations::OperationError;

/// What a request's waits are bound by.
#[derive(Debug, Clone)]
pub(crate) struct Turn {
    /// Set once the client hung up.
    gone: watch::Receiver<bool>,
    /// Set once the agent is asked to stop.
    stopping: watch::Receiver<bool>,
    until: Option<Instant>,
}

tokio::task_local! {
    static TURN: Turn;
}

impl Turn {
    pub(crate) fn new(
        gone: watch::Receiver<bool>,
        stopping: watch::Receiver<bool>,
        until: Option<Instant>,
    ) -> Self {
        Self { gone, stopping, until }
    }

    /// Runs `work` with its waits bound by this turn.
    pub(crate) async fn scope<F: Future>(self, work: F) -> F::Output {
        TURN.scope(self, work).await
    }
}

/// Waits for `ready` as long as the request may. Outside a request (tests,
/// the agent's own tasks) as long as it takes.
pub(crate) async fn wait<F: Future>(ready: F) -> Result<F::Output, OperationError> {
    let Ok(Turn { mut gone, mut stopping, until }) = TURN.try_with(Clone::clone) else {
        return Ok(ready.await);
    };
    let late = async move {
        match until {
            Some(at) => tokio::time::sleep_until(at).await,
            None => std::future::pending().await,
        }
    };
    let not_now = |why: &str| Err(OperationError::NotNow(why.to_owned()));
    tokio::select! {
        biased;
        () = set(&mut gone) => not_now("the client gave up waiting"),
        () = set(&mut stopping) => not_now("the agent is stopping"),
        out = ready => Ok(out),
        () = late => not_now("another operation took too long"),
    }
}

/// `mutex`'s lock, as long as the request may wait for it.
pub(crate) async fn lock<T>(
    mutex: &tokio::sync::Mutex<T>,
) -> Result<tokio::sync::MutexGuard<'_, T>, OperationError> {
    wait(mutex.lock()).await
}

/// Sets `stop` once the request's client hangs up or the agent is asked
/// to stop, for work that looks at it as it goes (a search); never outside
/// a request. Watches until the guard is dropped.
pub(crate) fn flag_when_given_up(stop: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Option<Watching> {
    let Turn { mut gone, mut stopping, .. } = TURN.try_with(Clone::clone).ok()?;
    let task = tokio::spawn(async move {
        tokio::select! {
            () = set(&mut gone) => {}
            () = set(&mut stopping) => {}
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    Some(Watching(task))
}

/// Watches as long as it lives.
#[derive(Debug)]
pub(crate) struct Watching(tokio::task::JoinHandle<()>);

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Resolves once `flag` is set; never if its sender went without.
async fn set(flag: &mut watch::Receiver<bool>) {
    if flag.wait_for(|set| *set).await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn turn(until: Option<Instant>) -> (Turn, watch::Sender<bool>, watch::Sender<bool>) {
        let (gone_tx, gone) = watch::channel(false);
        let (stopping_tx, stopping) = watch::channel(false);
        (Turn::new(gone, stopping, until), gone_tx, stopping_tx)
    }

    #[tokio::test]
    async fn outside_a_request_a_wait_takes_as_long_as_it_takes() {
        let mutex = tokio::sync::Mutex::new(());
        assert!(lock(&mutex).await.is_ok());
    }

    #[tokio::test]
    async fn a_free_lock_is_taken_and_a_held_one_waited_for() {
        let mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let (turn, _gone, _stopping) = turn(None);
        let held = mutex.clone().lock_owned().await;
        let other = mutex.clone();
        let waiting = tokio::spawn(turn.scope(async move { lock(&other).await.map(drop) }));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        drop(held);
        assert!(waiting.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn a_client_that_hung_up_gets_no_turn() {
        let mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let (turn, gone, _stopping) = turn(None);
        let _held = mutex.clone().lock_owned().await;
        let other = mutex.clone();
        let waiting = tokio::spawn(turn.scope(async move { lock(&other).await.map(drop) }));
        gone.send(true).unwrap();
        let refused = waiting.await.unwrap().unwrap_err();
        assert!(refused.to_string().contains("gave up"), "{refused}");
        // Even with the lock free.
        let (turn, gone, _stopping) = self::turn(None);
        gone.send(true).unwrap();
        let free = tokio::sync::Mutex::new(());
        assert!(turn.scope(async { lock(&free).await.map(drop) }).await.is_err());
    }

    #[tokio::test]
    async fn a_watcher_gone_without_a_word_ends_no_wait() {
        let (gone_tx, gone) = watch::channel(false);
        let (stopping_tx, stopping) = watch::channel(false);
        drop((gone_tx, stopping_tx));
        let turn = Turn::new(gone, stopping, None);
        let free = tokio::sync::Mutex::new(());
        assert!(turn.scope(async { lock(&free).await.map(drop) }).await.is_ok());
    }

    #[tokio::test]
    async fn work_that_looks_hears_when_its_client_hangs_up() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        assert!(flag_when_given_up(Arc::new(AtomicBool::new(false))).is_none(), "outside a request");
        let (turn, gone, _stopping) = turn(None);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let watching = turn.scope(async move { flag_when_given_up(flag) }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!stop.load(Ordering::Relaxed));
        gone.send(true).unwrap();
        for _ in 0..100 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(stop.load(Ordering::Relaxed));
        drop(watching);

        let (turn, _gone, stopping) = self::turn(None);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let _watching = turn.scope(async move { flag_when_given_up(flag) }).await.unwrap();
        stopping.send(true).unwrap();
        for _ in 0..100 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(stop.load(Ordering::Relaxed), "the agent stops");
    }

    #[tokio::test]
    async fn nothing_new_starts_once_the_agent_stops() {
        let (turn, _gone, stopping) = turn(None);
        stopping.send(true).unwrap();
        let free = tokio::sync::Mutex::new(());
        let refused = turn.scope(async { lock(&free).await.map(drop) }).await.unwrap_err();
        assert!(refused.to_string().contains("stopping"), "{refused}");
    }

    #[tokio::test]
    async fn a_wait_ends_when_the_client_would_have_no_time_left() {
        let mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let (turn, _gone, _stopping) = turn(Some(Instant::now() + Duration::from_millis(50)));
        let _held = mutex.clone().lock_owned().await;
        let refused = turn.scope(async { lock(&mutex).await.map(drop) }).await.unwrap_err();
        assert!(refused.to_string().contains("too long"), "{refused}");
    }
}
