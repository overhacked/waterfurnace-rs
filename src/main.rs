use eyre::Result;
use structopt::StructOpt;
use tokio::sync::Notify;
use tracing::info;
use tracing_subscriber;

use waterfurnace_symphony as wf;

use std::sync::Arc;

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

    let client = Arc::new(wf::Client::new(opt.username.clone(), opt.password.clone()));

    let client2 = Arc::clone(&client);
    tokio::spawn(async move { client2.connect().await });

    /*
    let run_time = std::time::Duration::from_secs(2);
    tokio::time::delay_for(run_time).await;
    */

    client.ready().await;
    client.login().await?;
    /*
    let data = client.send(wf::Command::Read { // TODO: implement rlist (including max_zone population)
        awl_id: "001EC02B2D8E".to_string(),
        zone: 0,
    }).await?;

    println!("{:#?}", data.await?);
    */


    let run_time = std::time::Duration::from_secs(10);
    tokio::time::delay_for(run_time).await;

    Ok(())
}
