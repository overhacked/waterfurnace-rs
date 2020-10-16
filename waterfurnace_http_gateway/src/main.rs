use eyre::Result;
use std::sync::Arc;
use std::net::SocketAddr;
use std::net::ToSocketAddrs as _;
use structopt::StructOpt;
use tokio::stream::StreamExt as _;
use tracing_subscriber;
use waterfurnace_http_gateway;

fn to_socket_addrs(src: &str) -> Result<Vec<SocketAddr>>
{
    Ok(src.to_socket_addrs()?.collect())
}

#[derive(Debug, StructOpt)]
#[structopt(name = "wf_gateway", about = "WaterFurnace Symphony gateway")]
struct Opt {
    #[structopt(short, long)]
    username: String,

    #[structopt(short, long)]
    password: String,

    #[structopt(short, long, default_value = "localhost:3030", parse(try_from_str = to_socket_addrs))]
    listen: Vec<Vec<SocketAddr>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Opt::from_args();

    let mut stream_map = tokio::stream::StreamMap::new();
    for addr in config.listen.iter().flatten() {
        stream_map.insert(addr.to_string(), tokio::net::TcpListener::bind(addr).await?);
    }
    let listen_addrs: Vec<String> = stream_map.keys().cloned().collect();
    println!("Listening on {}", listen_addrs.join(", "));
    let listeners = stream_map.map(|(_, s)| s);

    let shutdown_tx = Arc::new(tokio::sync::Notify::new());
    let shutdown_rx = shutdown_tx.clone();
    ctrlc::set_handler(move || {
        shutdown_tx.notify();
    }).expect("Error setting Ctrl-C handler");

    waterfurnace_http_gateway::run_incoming_graceful_shutdown(
        listeners,
        async move { shutdown_rx.notified().await; },
        &config.username,
        &config.password
    ).await?;
    Ok(())
}
