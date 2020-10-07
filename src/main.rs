use eyre::Result;
use structopt::StructOpt;
use tracing_subscriber;

use waterfurnace_symphony as wf;

use std::sync::Arc;
use std::time::Duration;

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

    let client = Arc::new(wf::Client::new());

    let client2 = Arc::clone(&client);
    tokio::spawn(async move { client2.connect(opt.username.clone(), opt.password.clone()).await });

    for _ in 1..18 {
        let _data = client.gateway_read("001EC02B2D8E").await?;

        let run_time = Duration::from_secs(10);
        tokio::time::delay_for(run_time).await;
    }

    Ok(())
}
