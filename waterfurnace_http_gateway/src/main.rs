pub(crate) mod cached_client;
mod handlers;
mod routes;

use backoff::{ExponentialBackoff, backoff::Backoff};
use futures::future::{self, Either};
use std::sync::{
    Arc,
    atomic::{
        AtomicBool,
        Ordering,
    },
};
use structopt::StructOpt;
use tracing::{
    error,
    info,
};
use tracing_subscriber;
use warp;
use waterfurnace_symphony as wf;

#[derive(Debug, StructOpt)]
#[structopt(name = "wf_gateway", about = "WaterFurnace Symphony gateway")]
struct Opt {
    #[structopt(short, long)]
    username: String,

    #[structopt(short, long)]
    password: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Opt::from_args();

    let client = Arc::new(wf::Client::new());
    let mut connect_h = spawn_connection(client.clone(), &config.username, &config.password);

    let mut backoff = ExponentialBackoff::default();
    let ready = Arc::new(AtomicBool::new(true));
    let api = routes::all(&client, Arc::clone(&ready));

    // TODO: configurable listen/port
    let mut serve_h = tokio::spawn(
        warp::serve(api)
        .bind(([127, 0, 0, 1], 3030))
    );

    loop {
        let select_result = future::try_select(
            connect_h,
            serve_h
        );
        // TODO: under which conditions does Client terminate Ok(()) or Err(...)?
        match select_result.await {
            Ok(Either::Left((_, _))) => { /* requested client shutdown */
                info!("Client connection closed, exiting...");
                break;
            },
            Ok(Either::Right((_, _))) => { /* requested server shutdown */
                info!("Shutting down...");
                break;
            },
            Err(Either::Left((client_err, serve,))) => { /* client error, retry */
                // Set the server to an unready state, so it
                // can start serving errors to HTTP clients
                ready.store(false, Ordering::Release);

                // Put the serve task handle back, so we can
                // keep serving requests after every loop iteration
                serve_h = serve;

                // Get the next backoff interval
                let next_backoff = match backoff.next_backoff() {
                    Some(duration) => duration,
                    None => break,
                };

                error!("waterfurnace_symphony::ClientError: {:?}", client_err);
                // Re-spawn the connection
                connect_h = spawn_connection(client.clone(), &config.username, &config.password);

                // Wait for the client to be in a ready state,
                // at MOST as long as the next backoff interval,
                // and reset the backoff timer once the client is ready.
                //
                // Any errors from Client::connect() will be picked up
                // by future::try_select(...) in the next iteration of the loop,
                // so the retry loop will continue until backoff.next_backoff()
                // returns None (currently never, until a config flag is added)
                if let Ok(_) = client.wait_ready_timeout(next_backoff).await {
                    // Tell the server to start serving requests normally
                    ready.store(true, Ordering::Release);

                    // Reset the backoff timer so the next failure has
                    // a short retry interval, again
                    backoff.reset();
                }
            },
            Err(Either::Right((_, _))) => { /* serve panic, fail */
                error!("Server thread panicked, exiting!");
                break;
            },
        }
    }
}

fn spawn_connection(client: Arc<wf::Client>, username: &str, password: &str) -> tokio::task::JoinHandle<wf::ConnectResult> {
    let connect_username = username.to_string();
    let connect_password = password.to_string();
    tokio::spawn(async move {
        client.connect(connect_username, connect_password).await
    })
}
