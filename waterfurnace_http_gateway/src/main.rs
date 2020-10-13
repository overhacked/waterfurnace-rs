use eyre::Result;
use std::net::SocketAddr;
use structopt::StructOpt;
use tracing_subscriber;
use waterfurnace_http_gateway;

#[derive(Debug, StructOpt)]
#[structopt(name = "wf_gateway", about = "WaterFurnace Symphony gateway")]
struct Opt {
    #[structopt(short, long)]
    username: String,

    #[structopt(short, long)]
    password: String,

    /*
     * TODO:
     * clap::Arg.validate
     * -> net2::TcpBuilder 
     * -> tokio::net::TcpStream.from_std
     * -> and warp::Server.serve_incoming
     * to bind to multiple addresses (incl. localhost IPv4/6)
     */
    #[structopt(short, long, default_value = "127.0.0.1:3030")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Opt::from_args();

    // TODO: configurable listen/port
    waterfurnace_http_gateway::run(config.listen.clone(), &config.username, &config.password).await?;

    Ok(())
}
