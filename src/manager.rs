use crossbeam::atomic::AtomicCell;
use futures::prelude::*;
use tokio::sync::{
    mpsc,
    Mutex,
    Notify,
    RwLock,
};
use tracing::debug;

use crate::Session;
use crate::session::Message;
use crate::session::Result as SessionResult;
use crate::session::SessionError;
use crate::session::state::{self, LoggedIn};

use std::sync::Arc;
use std::time::Duration;


// Taken from setTimeout(1500000, ...) in Symphony JavaScript
const SESSION_TIMEOUT: Duration = Duration::from_millis(1500000);

type ManagedSession = Arc<RwLock<Option<Session<state::Connected>>>>;

pub struct SessionManager {
    session: ManagedSession,
    username: String,
    password: String,
    shutdown_handle: AtomicCell<Option<future::AbortHandle>>,
    closing_semaphore: Arc<Notify>,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionManager ")
            .field("session", &self.session)
            .field("username", &self.username)
            .field("password", &"********")
            .finish()
    }
}

impl SessionManager {
    pub fn new(username: &str, password: &str)
        -> SessionManager
    {
        SessionManager {
            session: Arc::new(RwLock::new(None)),
            username: username.to_string(),
            password: password.to_string(),
            shutdown_handle: AtomicCell::new(None),
            closing_semaphore: Arc::new(Notify::new()),
        }
    }

    pub async fn login(&self)
        -> SessionResult<()>
    {
        let mut session_lock = self.session.write().await;

        // Idempotent no-op if already logged in
        if let Some(_) = *session_lock {
            return Ok(());
        }

        let connected_session =
            Session::new()
            .login(&self.username, &self.password).await?
            .connect().await?;
        *session_lock = Some(connected_session);

        self.shutdown_handle.store(Some(self.spawn_renew_task(SESSION_TIMEOUT)));

        Ok(())
    }

    pub async fn stream(&self, mut tx: mpsc::UnboundedReceiver<String>, rx: mpsc::UnboundedSender<String>) -> SessionResult<()>
    {
        let session_lock = self.session.read().await;

        match &*session_lock {
            Some(session) => {
                let mut stream = session.stream()?;
                let closing_semaphore = self.closing_semaphore.clone();
                loop {
                    {tokio::select! {
                        Some(msg) = stream.next() => {
                            debug!("Received message: {:?}", msg);
                            match msg.unwrap() {
                                Message::Text(s) => rx.send(s).map_err(|_| SessionError::Pipe), // TODO: remove unwrap
                                _ => continue,
                            }
                        },
                        Some(s)   =    tx.next() => {
                            debug!("Sending message: {:?}", s);
                            stream.start_send_unpin(s.into()).map_err(SessionError::WebSockets)
                        },
                        _ = closing_semaphore.notified() => break,
                    }}?;
                }
            },
            None => {},
        }
        Ok(())
    }

    pub fn close(&self)
        -> SessionResult<()>
    {
        self.closing_semaphore.notify();

        if let Some(h) = self.shutdown_handle.take() {
            h.abort();
        }

        Ok(())
    }

    pub async fn logout(&self)
        -> SessionResult<()>
    {
        let mut session_lock = self.session.write().await;

        // Idempotent no-op if already logged out
        if let None = *session_lock {
            return Ok(())
        }

        let connected_session = (*session_lock).take().unwrap();
        connected_session.logout().await?;

        Ok(())
    }

    fn spawn_renew_task(&self, duration: Duration)
        -> future::AbortHandle
    {
        let session_handle = Arc::clone(&self.session);
        let username = self.username.clone();
        let password = self.password.clone();
        let closing_semaphore = self.closing_semaphore.clone();

        // Allowed here to remove warning about unreachable
        // Ok(...) at end of async block
        #[allow(unreachable_code)]
        let (timeout_fut, shutdown_handle) = future::abortable(async move {
            // Scope these moved values outside
            // the loop so they aren't dropped
            // at the end of each loop
            let username = username;
            let password = password;
            let session_handle = session_handle;
            let closing_semaphore = closing_semaphore;
            loop {
                tokio::time::delay_for(duration).await;
                closing_semaphore.notify();
                let mut session_lock = session_handle.write().await;
                let connected_session = (*session_lock).take().unwrap();
                connected_session.logout().await?;
                let connected_session =
                    Session::new()
                    .login(&username, &password).await?
                    .connect().await?;
                *session_lock = Some(connected_session);
            }
            // Explicit return type required to use ? operator in async block
            // See https://rust-lang.github.io/async-book/07_workarounds/02_err_in_async_blocks.html
            Ok::<(), SessionError>(())
        });
        tokio::spawn(timeout_fut);
        shutdown_handle
    }

    pub async fn get_token(&self) -> SessionResult<String> {
        let session_lock = self.session.read().await;
        match &*session_lock {
            Some(session) => Ok(session.get_token().to_string()),
            None => Err(SessionError::InvalidCredentials("No session token".to_string())), // TODO: better error
        }
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        // Make sure to not leave Session renewing forever,
        // owned by the renew task's Arc
        if let Some(h) = self.shutdown_handle.take() { // TODO: handle error
            h.abort();
        }
    }
}
