use eyre::Result;
use structopt::StructOpt;
use tracing_subscriber;

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
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let opt = Opt::from_args();

    let session = wf::Session::new()
        .login(&opt.username, &opt.password).await?;

    session.logout().await?;

    Ok(())
}
