mod support;

use futures::{
    self,
    Future
};
use support::*;
use support::server::Server;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use tracing_subscriber;

use waterfurnace_symphony::{
    Client,
    ClientError,
};

type ConnectReturnType = Result<(), std::boxed::Box<dyn Error + Send + Sync>>;

async fn get_connected_client(server: &Server)
    -> (Arc<Client>, tokio::task::JoinHandle<ConnectReturnType>,)
{

    let client = Arc::new(Client::new_for(
        &server.get_login_uri(),
        &server.get_config_uri()
    ));

    let client2 = Arc::clone(&client);
    let connect_h = tokio::spawn(async move { client2.connect("test_user".to_string(), "bad7a55".to_string()).await });
    
    tokio::time::delay_for(Duration::from_millis(500)).await; // Wait for the connection to actually happen

    (client, connect_h,)
}

async fn check_client_operations<O>(client: Arc<Client>, connect_h: tokio::task::JoinHandle<ConnectReturnType>, ops: O)
    where O: Future<Output = Result<(), ClientError>>
{
    let (connect_result, ops_result) = tokio::join! {
        async {
            connect_h.await.unwrap()
        },
        async {
            ops.await?;
            if client.is_ready() {
                client.logout().await?;
            }
            Ok::<(), ClientError>(())
        },
    };

    if let Err(e) = connect_result {
        println!("{}", e);
        panic!("client.connect() returned error");
    }

    if let Err(e) = ops_result {
        println!("{}", e);
        panic!("client operations returned error");
    }

}

#[tokio::test]
async fn client_connect() {
    let _ = tracing_subscriber::fmt::try_init();

    let server = server::http();

    let (client, connect_h) = get_connected_client(&server).await;

    check_client_operations(client, connect_h, async {
        Ok::<(), ClientError>(())
    }).await;
}

#[tokio::test]
async fn client_read() {
    let _ = tracing_subscriber::fmt::try_init();

    let server = server::http();

    let (client, connect_h) = get_connected_client(&server).await;
    let client2 = Arc::clone(&client);

    check_client_operations(client2, connect_h, async {
        let gateways = client.get_gateways().await.expect("No gateways found");
        let some_gateway = gateways.keys().collect::<Vec<&String>>()[0];
        let read_response = client.gateway_read(some_gateway).await?;
        assert!(read_response.awl_id == *some_gateway);
        Ok::<(), ClientError>(())
    }).await;

}
