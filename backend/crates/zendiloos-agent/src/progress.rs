//! Steps a long operation reports while it runs ([`Response::Progress`]),
//! sent to its client before the answer: creating a pool takes minutes,
//! and a person waiting for it should see what happens.
//!
//! [`Response::Progress`]: zendiloos_agent_proto::Response::Progress

use std::future::Future;

use tokio::sync::mpsc::UnboundedSender;

/// A step: a word the interface names (`creating_pool`), and how far in
/// percent where that can be told.
pub(crate) type Step = (&'static str, Option<u8>);

tokio::task_local! {
    static STEPS: UnboundedSender<Step>;
}

/// Runs `work` with its steps going to `steps`.
pub(crate) async fn scope<F: Future>(steps: UnboundedSender<Step>, work: F) -> F::Output {
    STEPS.scope(steps, work).await
}

/// Reports `step`; outside a request (tests, the agent's own tasks) it goes
/// nowhere.
pub(crate) fn step(step: &'static str, percent: Option<u8>) {
    let _ = STEPS.try_with(|steps| steps.send((step, percent)));
}

/// `done` of `total`, in percent.
pub(crate) fn percent(done: usize, total: usize) -> Option<u8> {
    (total > 0).then(|| u8::try_from(done.min(total) * 100 / total).unwrap_or(100))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn steps_go_to_the_requests_client_and_nowhere_else() {
        step("outside", None);
        let (sender, mut steps) = tokio::sync::mpsc::unbounded_channel();
        scope(sender, async { step("creating_pool", percent(1, 4)) }).await;
        assert_eq!(steps.recv().await, Some(("creating_pool", Some(25))));
        assert!(steps.try_recv().is_err());
        assert_eq!(percent(3, 0), None);
        assert_eq!(percent(5, 4), Some(100));
    }
}
