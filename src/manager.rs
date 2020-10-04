use futures::prelude::*;
use tokio::sync::Mutex;
use tokio::sync::Notify;

use crate::Session;
use crate::session::Message;
use crate::session::Result as SessionResult;
use crate::session::SessionError;
use crate::session::state;

use std::sync::Arc;
use std::time::Duration;


// Taken from setTimeout(1500000, ...) in Symphony JavaScript
const SESSION_TIMEOUT: Duration = Duration::from_millis(1500000);

type ManagedSession = Arc<Mutex<Option<Session<state::Connected>>>>;

pub struct SessionManager {
    session: ManagedSession,
    username: String,
    password: String,
    shutdown_handle: Option<future::AbortHandle>,
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
            session: Arc::new(Mutex::new(None)),
            username: username.to_string(),
            password: password.to_string(),
            shutdown_handle: None,
            closing_semaphore: Arc::new(Notify::new()),
        }
    }

    pub async fn login(&mut self)
        -> SessionResult<()>
    {
        // Idempotent no-op if already logged in
        match self.session.try_lock() {
            Ok(s) if s.is_some() => return Ok(()),
            Err(_) => return Ok(()),
            Ok(_) => {},
        }

        let mut session_lock = self.session.clone().lock_owned().await;
        let connected_session =
            Session::new()
            .login(&self.username, &self.password).await?
            .connect().await?;
        *session_lock = Some(connected_session);

        self.shutdown_handle = Some(self.spawn_renew_task(SESSION_TIMEOUT));

        Ok(())
    }

    pub async fn stream<T,R>(&self, mut tx: T, mut rx: R)
        where T: Stream<Item = String> + std::marker::Unpin, R: Sink<String> + std::marker::Unpin
    {
        let mut session_lock = self.session.clone().lock_owned().await;
        match &mut *session_lock {
            Some(session) => {
                let (mut ws_tx, mut ws_rx) = session.stream().split();
                let closing_semaphore = self.closing_semaphore.clone();
                loop {
                    tokio::select! {
                        Some(msg) = ws_rx.next() => rx.start_send_unpin(msg.unwrap().into_text().unwrap()).map_err(|_| SessionError::PipeError), // TODO: remove unwrap
                        Some(s)   =    tx.next() => ws_tx.start_send_unpin(s.into()).map_err(SessionError::WebSockets),
                        _ = closing_semaphore.notified() => break,
                    };
                }
            },
            None => {},
        }
    }

    pub async fn logout(&self)
        -> SessionResult<()>
    {
        self.closing_semaphore.notify();
        // Idempotent no-op if already logged out
        if self.session.lock().await.is_none() {
            return Ok(())
        }

        if let Some(ref h) = self.shutdown_handle {
            h.abort();
        }
        let mut session_lock = self.session.clone().lock_owned().await;
        let connected_session = session_lock.take().unwrap();
        connected_session.logout().await?;

        Ok(())
    }

    fn spawn_renew_task(&self, duration: Duration)
        -> future::AbortHandle
    {
        let session_handle = Arc::clone(&self.session);
        let username = self.username.clone();
        let password = self.password.clone();

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
            loop {
                tokio::time::delay_for(duration).await;
                let mut session_lock = session_handle.clone().lock_owned().await;
                let connected_session = session_lock.take().unwrap();
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
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        // Make sure to not leave Session renewing forever,
        // owned by the renew task's Arc
        if let Some(ref h) = self.shutdown_handle {
            h.abort();
        }
    }
}
