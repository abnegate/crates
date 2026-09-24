use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use tokio::task::JoinError;
use tokio::task::JoinHandle;

/// A spawned task that is aborted when its handle is dropped.
///
/// Dropping a [`JoinHandle`] detaches the task rather than stopping it, so a
/// run its caller gave up on left the tool it was waiting for running: a
/// command's whole process group, a half-done write, a remote call. Aborting
/// drops the tool's future, and with it whatever that future owns.
pub(super) struct Task<Value> {
    handle: JoinHandle<Value>,
}

impl<Value: Send + 'static> Task<Value> {
    pub(super) fn spawn(future: impl Future<Output = Value> + Send + 'static) -> Self {
        Self {
            handle: tokio::spawn(future),
        }
    }
}

impl<Value> Future for Task<Value> {
    type Output = Result<Value, JoinError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.handle).poll(context)
    }
}

impl<Value> Drop for Task<Value> {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
