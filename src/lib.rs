use core::{
    future::Future,
    task::{Context, Poll},
    pin::Pin,
};
use async_channel::{self as channel, Sender, Receiver};
use std::sync::atomic::{AtomicBool, Ordering};
use event_listener::{Event, EventListener};
use smol_timeout::{Timeout, TimeoutExt};
use std::time::Duration;

type Value = bool;

pub struct State {
    value: AtomicBool,
    done: Event,
    tx: Sender<Value>,
    rx: Receiver<Value>,
}

struct UpdateFuture {
    error: Option<async_channel::TrySendError<Value>>,
    done: Option<Timeout<EventListener>>,
}

impl UpdateFuture {
    fn new(done: EventListener) -> Self {
        Self {
            error: None,
            done: Some(done.timeout(Duration::from_millis(100))),
        }
    }

    fn error(senderr: async_channel::TrySendError<Value>) -> Self {
        Self {
            error: Some(senderr),
            done: None,
        }
    }
}

impl Future for UpdateFuture {
    type Output = Result<(), async_channel::TrySendError<Value>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>)
        -> Poll<Self::Output>
    {
        // Early return
        if let Some(error) = self.error {
            return Poll::Ready(Err(error));
        }

        // Are we there yet?
        if let Some(done) = &mut self.done {
            let done = Pin::new(done);
            if let Poll::Ready(_) = done.poll(cx) {
                return Poll::Ready(Ok(()));
            }
        }

        // Keep waiting
        return Poll::Pending;
    }
}

impl State {
    pub fn new() -> Self {
        let (tx, rx) = channel::bounded(1);

        Self{
            value: AtomicBool::new(false),
            done: Event::new(),
            tx, rx,
        }
    }

    pub fn has_updated(&self) -> bool {
        let value = self.value.load(Ordering::Relaxed);
        value
    }

    pub fn try_update(&self, value: Value)
        -> Result<(), async_channel::TrySendError<Value>>
    {
        self.tx.try_send(value)
    }

    pub fn update(&self, value: Value) -> impl Future<Output=Result<(), async_channel::TrySendError<Value>>> {
        let done = self.done.listen();
        if let Err(senderr) = self.tx.try_send(value) {
            println!("unable to update: {}", senderr);
            return UpdateFuture::error(senderr);
        }

        // maybe block until ready
        return UpdateFuture::new(done);
    }

    pub async fn run(&self) {
        while let Ok(new_value) = self.rx.recv().await {
            // Acquire write access
            self.value.store(new_value, Ordering::SeqCst);

            // mischief managed
            self.done.notify(usize::MAX);
        }
    }

    pub async fn run_once(&self) {
        if let Ok(new_value) = self.rx.recv().await {
            // Acquire write access
            self.value.store(new_value, Ordering::SeqCst);

            // mischief managed
            self.done.notify(usize::MAX);
        }
    }

    pub fn done(&self) -> EventListener {
        self.done.listen()
    }

    pub fn close(&self) {
        self.rx.close();
    }
}

#[cfg(test)]
mod tests {
    use async_std::task;
    use std::{
        sync::Arc,
        time::Duration,
    };
    use smol_timeout::TimeoutExt;
    use super::State;

    #[async_std::test]
    async fn no_update_without_worker() {
        let state = State::new();
        assert!(!state.has_updated());

        assert!(state
            .update(true)
            .await
            .is_ok()
        );
        assert!(!state.has_updated());
    }

    #[async_std::test]
    async fn update_from_background() {
        let state = Arc::new(State::new());

        // Spawn a worker - Updates can only happen
        // if this worker is running
        let worker = state.clone();
        task::spawn(async move {
            worker.run().await;
            println!("Worker finished");
        });

        assert!(!state.has_updated());

        // blocks until the worker has updated the state
        assert!(state
            .update(true)
            .timeout(Duration::from_millis(100))
            .await.is_some());
        assert!(state.has_updated());
    }

    #[async_std::test]
    async fn update_error_without_worker() {
        let state = State::new();

        state.close();

        assert!(state
            .update(true)
            .await
            .is_err()
        );
        assert!(!state.has_updated());
    }

    #[async_std::test]
    async fn update_error_with_worker() {
        let state = Arc::new(State::new());

        // Spawn a worker - it should not matter if the channel
        // is closed before this worker is able to complete work
        let worker = state.clone();
        task::spawn(async move {
            worker.run().await;
            println!("Worker finished");
        });

        state.close();

        assert!(state
            .update(true)
            .await
            .is_err()
        );
        assert!(!state.has_updated());
    }

    #[test] // This is not an asynchronous test
    fn try_update() {
        let state = State::new();
        let done = state.done();
        assert!(!state.has_updated());

        // synchronously update the value
        assert!(state.try_update(true).is_ok());
        assert!(!state.has_updated());

        // wait for asynchronous work
        task::block_on(state.run_once());

        assert!(done
            .wait_timeout(Duration::from_millis(100))
        );
        assert!(state.has_updated());
    }
}
