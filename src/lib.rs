use async_channel::{self as channel, Sender, Receiver};
use std::sync::atomic::{AtomicBool, Ordering};
use event_listener::{Event, EventListener};
use smol_timeout::TimeoutExt;
use std::time::Duration;

pub struct State {
    value: AtomicBool,
    done: Event,
    tx: Sender<bool>,
    rx: Receiver<bool>,
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

    pub fn try_update(&self, value: bool)
        -> Result<(), async_channel::TrySendError<bool>>
    {
        self.tx.try_send(value)
    }

    pub async fn update(&self, value: bool) {
        let done = self.done.listen();
        if let Err(senderr) = self.tx.send(value).await {
            println!("unable to update: {}", senderr);
            return;
        }

        // maybe block until ready
        if let None = done
            .timeout(Duration::from_millis(100))
            .await {
            println!("waited until timeout");
        }
    }

    pub async fn run(&self) {
        if let Ok(new_value) = self.rx.recv().await {
            // Acquire write access
            self.value.store(new_value, Ordering::SeqCst);

            // mischief managed
            self.done.notify(usize::MAX);
        } else {
            println!("No value on channel");
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
    use std::sync::Arc;
    use super::State;

    #[async_std::test]
    async fn no_update_without_worker() {
        let state = State::new();
        assert!(!state.has_updated());

        state.update(true).await;
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
        state.update(true).await;
        assert!(state.has_updated());
    }

    #[async_std::test]
    async fn update_error() {
        let state = State::new();

        state.close();
        state.update(true).await;

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
        state.update(true).await;

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
        task::block_on(state.run());

        done.wait();
        assert!(state.has_updated());
    }
}
