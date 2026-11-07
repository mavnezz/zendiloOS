//! What a long operation reports while it runs, sent to its client before
//! the answer: the step it is at ([`Response::Progress`]), since creating
//! a pool takes minutes and a person waiting for it should see what
//! happens; and the lines a program it runs writes ([`Response::Output`]),
//! such as Docker Compose's while a project is deployed.
//!
//! [`Response::Progress`]: zendiloos_agent_proto::Response::Progress
//! [`Response::Output`]: zendiloos_agent_proto::Response::Output

use std::future::Future;

use tokio::sync::mpsc::UnboundedSender;

/// What an operation reports on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Note {
    /// A word the interface names (`creating_pool`), and how far in percent
    /// where that can be told.
    Step(&'static str, Option<u8>),
    /// A line a program wrote.
    Line(String),
}

tokio::task_local! {
    static NOTES: UnboundedSender<Note>;
}

/// Runs `work` with what it reports going to `notes`.
pub(crate) async fn scope<F: Future>(notes: UnboundedSender<Note>, work: F) -> F::Output {
    NOTES.scope(notes, work).await
}

/// Reports `step`; outside a request (tests, the agent's own tasks) it goes
/// nowhere.
pub(crate) fn step(step: &'static str, percent: Option<u8>) {
    let _ = NOTES.try_with(|notes| notes.send(Note::Step(step, percent)));
}

/// Reports a line a program wrote; outside a request it goes nowhere.
pub(crate) fn line(line: &str) {
    let _ = NOTES.try_with(|notes| notes.send(Note::Line(line.to_owned())));
}

/// `done` of `total`, in percent.
pub(crate) fn percent(done: usize, total: usize) -> Option<u8> {
    (total > 0).then(|| u8::try_from(done.min(total) * 100 / total).unwrap_or(100))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn what_is_reported_goes_to_the_requests_client_and_nowhere_else() {
        step("outside", None);
        line("outside");
        let (sender, mut notes) = tokio::sync::mpsc::unbounded_channel();
        scope(sender, async {
            step("creating_pool", percent(1, 4));
            line("Container web Started");
        })
        .await;
        assert_eq!(notes.recv().await, Some(Note::Step("creating_pool", Some(25))));
        assert_eq!(notes.recv().await, Some(Note::Line("Container web Started".into())));
        assert!(notes.try_recv().is_err());
        assert_eq!(percent(3, 0), None);
        assert_eq!(percent(5, 4), Some(100));
    }
}
