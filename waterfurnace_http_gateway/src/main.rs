mod handlers;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use structopt::StructOpt;
use tokio;
use tracing_subscriber;
use warp::{
    self,
    Filter,
    Rejection,
    Reply,
};
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

    let backend = Backend::start(config.username, config.password).await;

    warp::serve(gateways_route(&backend.client)).run(([127, 0, 0, 1], 3030)).await;
}

struct Backend {
    client: Arc<wf::Client>,
    connection: tokio::task::JoinHandle<wf::ConnectResult>,
}

fn with_client(client: Arc<wf::Client>) -> impl Filter<Extract = (Arc<wf::Client>,), Error = Infallible> + Clone {
    warp::any()
        .map(move || client.clone() )
}

impl Backend {
    async fn start(username: String, password: String) -> Self {
        let client = Arc::new(wf::Client::new());
        let connection = Arc::clone(&client);
        let connection_h = tokio::spawn(async move { connection.connect(username.clone(), password.clone()).await });
        tokio::time::delay_for(Duration::from_millis(500)).await; // Wait for the connection to actually happen

        Backend {
            client: client,
            connection: connection_h,
        }
    }
}

fn gateways_route(client: &Arc<wf::Client>) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    let client = Arc::clone(client);

    warp::path("gateways")
        .and(warp::get())
        .and(with_client(client))
        .and_then(handlers::gateways_handler)
}

