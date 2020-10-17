pub(crate) mod cached_client;
mod handlers;
mod routes;

use backoff::{ExponentialBackoff, backoff::Backoff};
use futures::future;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{
        AtomicBool,
        Ordering,
    },
};
use thiserror::Error;
use tracing::{
    trace,
    info,
    warn,
    error,
};
use tracing_futures::Instrument as _;
use warp;
use waterfurnace_symphony as wf;

pub async fn run(addr: impl Into<SocketAddr> + 'static, username: &str, password: &str) -> Result<()>
{
    let client = wf::Client::new();
    run_with_client(client, addr, username, password).await
}

pub async fn run_incoming<I>(incoming: I, username: &str, password: &str) -> Result<()>
where
    I: futures::stream::TryStream + Send + 'static,
    I::Ok: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static + Unpin,
    I::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let client = wf::Client::new();
    run_incoming_with_client_graceful_shutdown(client, incoming, future::pending(), username, password).await
}

pub async fn run_incoming_graceful_shutdown<I>(incoming: I, shutdown: impl Future<Output = ()> + Send + 'static, username: &str, password: &str) -> Result<()>
where
    I: futures::stream::TryStream + Send + 'static,
    I::Ok: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static + Unpin,
    I::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let client = wf::Client::new();
    run_incoming_with_client_graceful_shutdown(client, incoming, shutdown, username, password).await
}

pub async fn run_with_client(client: wf::Client, addr: impl Into<SocketAddr> + 'static, username: &str, password: &str) -> Result<()>
{
    let listener = tokio::net::TcpListener::bind(addr.into()).await?;
    run_incoming_with_client_graceful_shutdown(client, listener, future::pending(), username, password).await
}

#[tracing::instrument(skip(client, incoming, shutdown),fields(password = "********"))]
pub async fn run_incoming_with_client_graceful_shutdown<I>(client: wf::Client, incoming: I, shutdown: impl Future<Output = ()> + Send + 'static, username: &str, password: &str) -> Result<()>
where
    I: futures::stream::TryStream + Send + 'static,
    I::Ok: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static + Unpin,
    I::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let client = Arc::new(client);
    let mut connect_h = spawn_connection(client.clone(), &username, &password);

    let mut backoff = ExponentialBackoff::default();
    backoff.max_elapsed_time = None;

    let ready = Arc::new(AtomicBool::new(true));
    let api = routes::all(&client, Arc::clone(&ready));

    let serve_h = tokio::spawn(async move {
        // Has to be wrapped in Ok() to match return
        // type of spawn_connection
        Ok::<(), Box<dyn std::error::Error + Send + Sync + 'static>>(
            warp::serve(api)
            .serve_incoming_with_graceful_shutdown(incoming, shutdown).await
        )
    });

    tokio::pin!(serve_h);

    loop {
        tokio::select! {
            Ok(r) = connect_h => {
                match r {
                    Ok(_) => { /* requested client shutdown */
                        return Err(ServerError::ClientGone);
                    },
                    Err(client_err) => { /* client error, retry */
                        // Set the server to an unready state, so it
                        // can start serving errors to HTTP clients
                        ready.store(false, Ordering::Release);

                        // Get the next backoff interval
                        let next_backoff = match backoff.next_backoff() {
                            Some(duration) => duration,
                            None => return Err(ServerError::RetryGiveUp),
                        };

                        warn!(err = ?client_err, "Symphony client error, retrying after {:?}", next_backoff);
                        // Re-spawn the connection
                        connect_h = spawn_connection(client.clone(), &username, &password);

                        // Wait for the client to be in a ready state,
                        // at MOST as long as the next backoff interval,
                        // and reset the backoff timer once the client is ready.
                        //
                        // Any errors from Client::connect() will be picked up
                        // by future::try_select(...) in the next iteration of the loop,
                        // so the retry loop will continue until backoff.next_backoff()
                        // returns None (currently never, until a config flag is added)
                        if let Ok(_) = client.wait_ready_timeout(next_backoff).await {
                            info!("Client ready again, enabling gateway routes");
                            // Tell the server to start serving requests normally
                            ready.store(true, Ordering::Release);

                            // Reset the backoff timer so the next failure has
                            // a short retry interval, again
                            backoff.reset();
                        }
                    },
                }
            },
            Ok(r) = &mut serve_h => {
                match r {
                    Ok(_) => { /* requested server shutdown */
                        info!("Shutting down...");
                        client.logout().await?;
                        return Ok(());
                    },
                    Err(_) => { /* serve panic, fail */
                        return Err(ServerError::ServerPanic);
                    },
                }
            },
            else => panic!("Joining connect_h or serve_h failed"),
        };
    }
}

#[tracing::instrument(skip(client, username, password), level = "debug")]
fn spawn_connection(client: Arc<wf::Client>, username: &str, password: &str) -> tokio::task::JoinHandle<wf::ConnectResult> {
    let connect_username = username.to_string();
    let connect_password = password.to_string();
    tokio::spawn(async move {
        trace!("calling client.connect()");
        let connect_result = client.connect(connect_username, connect_password).await;
        trace!("client.connect() returned");
        connect_result
    }.in_current_span())
}

type Result<T> = std::result::Result<T, ServerError>;

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("Symphony connection error: {0}")]
    ClientError(#[from] wf::ClientError),
    #[error("Client unexpectedly shut down")]
    ClientGone,
    #[error("Server thread panicked")]
    ServerPanic,
    #[error("Max retries exceeded")]
    RetryGiveUp,
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}
