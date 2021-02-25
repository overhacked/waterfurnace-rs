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
#[structopt(about, version = env!("GIT_DESCRIBE"))]
struct Opt {
    #[structopt(short, long, env = "WATERFURNACE_USER")]
    /// Username to log into WaterFurnace Symphony
    username: String,

    #[structopt(short, long, env = "WATERFURNACE_PASSWORD", hide_env_values = true)]
    /// Password to log into WaterFurnace Symphony
    password: String,

    #[structopt(short, long, default_value = "localhost:3030", parse(try_from_str = to_socket_addrs), value_name = "{ IPv4 | '['IPv6']' | Hostname }:Port")]
    /// Address and port to listen on
    listen: Vec<Vec<SocketAddr>>,

    #[cfg(feature = "gelf")]
    #[structopt(long, value_name = "{ IPv4 | '['IPv6']' }:Port")]
    /// A GELF UDP socket to send trace events to
    gelf: Option<SocketAddr>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Opt::from_args();

    init_tracing(
        #[cfg(feature = "gelf")]
        config.gelf,
    );

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

fn init_tracing(
    #[cfg(feature = "gelf")]
    gelf_addr: Option<SocketAddr>,
) {
    use tracing_subscriber::{fmt, EnvFilter};
    use tracing_subscriber::prelude::*;
    #[cfg(feature = "gelf")]
    use tracing_gelf::Logger as Gelf;

    let fmt_layer = fmt::layer();
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("warn"))
        .unwrap();

    let registry = tracing_subscriber::registry();

    let registry = registry
        .with(filter_layer)
        .with(fmt_layer);

    #[cfg(feature = "gelf")]
    let registry = registry.with(
        match gelf_addr {
            Some(addr) => {
                let (layer, task) = Gelf::builder()
                    .buffer(2048)
                    .file_names(false)
                    .connect_udp(addr)
                    .unwrap();
                tokio::spawn(task);
                Some(layer)
            },
            _ => { None },
        }
    );

    registry.init();
}
