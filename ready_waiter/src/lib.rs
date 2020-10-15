use std::fmt;
use std::time::Duration;
use tokio;
use tokio::sync::watch::{self, Sender, Receiver};
use tokio::time::{timeout as tokio_timeout, Elapsed};
use tracing::{self, trace, debug};
use tracing_futures;

#[derive(Debug)]
pub struct Waiter {
    ready_tx: Sender<bool>,
    ready_rx: Receiver<bool>,
}

impl fmt::Display for Waiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ready_str = if self.is_ready() { "Ready" } else { "Not Ready" };
        write!(f, "{}", ready_str)
    }
}

impl Waiter {
    pub fn new(is_ready: bool) -> Self {
        let (tx, rx) = watch::channel(is_ready);
        Waiter {
            ready_tx: tx,
            ready_rx: rx,
        }
    }

    #[tracing::instrument(skip(self))]
    pub fn set(&self, is_ready: bool) {
        self.ready_tx.broadcast(is_ready).unwrap();
        trace!("set value");
    }

    pub fn set_ready(&self) {
        self.set(true)
    }

    pub fn set_unready(&self) {
        self.set(false)
    }

    pub fn is_ready(&self) -> bool {
        *self.ready_rx.borrow()
    }

    #[tracing::instrument(skip(self))]
    pub async fn wait(&self, is_ready: bool) {
        debug!("waiting");
        let mut rx = self.ready_rx.clone();
        loop {
            match rx.recv().await {
                None => panic!("self.ready_tx should not be dropped"),
                Some(v) if v == is_ready => {
                    debug!("done waiting");
                    break
                },
                Some(v) => {
                    trace!("saw {:?}", v);
                    continue
                },
            }
        }
    }

    pub async fn wait_timeout(&self, timeout: Duration, is_ready: bool)
        -> Result<(), Elapsed>
    {
        tokio_timeout(timeout, self.wait(is_ready)).await?;
        Ok(())
    }

    pub async fn wait_ready(&self) {
        self.wait(true).await
    }

    pub async fn wait_unready(&self) {
        self.wait(false).await
    }

    pub async fn wait_ready_timeout(&self, timeout: Duration)
        -> Result<(), Elapsed>
    {
        self.wait_timeout(timeout, true).await?;
        Ok(())
    }

    pub async fn wait_unready_timeout(&self, timeout: Duration)
        -> Result<(), Elapsed>
    {
        self.wait_timeout(timeout, false).await?;
        Ok(())
    }
}
