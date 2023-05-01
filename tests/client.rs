use futures::{
    self,
    Future
};
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use mock_symphony::{
    self,
    Chaos,
    FailProbability,
};
use waterfurnace_symphony::{
    Client,
    ClientError,
};

type ConnectReturnType = Result<(), std::boxed::Box<dyn Error + Send + Sync>>;

async fn get_connected_client(server: &mock_symphony::Server)
    -> (Arc<Client>, tokio::task::JoinHandle<ConnectReturnType>,)
{

    let client = Arc::new(Client::new_for(
        &server.get_login_uri(),
        &server.get_config_uri()
    ));

    let client2 = Arc::clone(&client);
    let connect_h = tokio::spawn(async move { client2.connect("test_user".to_string(), "bad7a55".to_string()).await });
    
    tokio::time::sleep(Duration::from_millis(500)).await; // Wait for the connection to actually happen

    (client, connect_h,)
}

async fn check_client_operations<O>(client: Arc<Client>, connect_h: tokio::task::JoinHandle<ConnectReturnType>, ops: O)
    where O: Future<Output = Result<(), ClientError>>
{
    let (connect_result, ops_result) = tokio::join!(
    async {
        connect_h.await
    },
    async {
        info!("Running test operations");
        let ops_result = ops.await;
        info!("Test operations succeeded");
        if client.is_ready() {
            info!("Logging out client");
            client.logout().await?;
        }
        ops_result
    });

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

    let server = mock_symphony::http();

    let (client, connect_h) = get_connected_client(&server).await;

    check_client_operations(client, connect_h, async {
        Ok::<(), ClientError>(())
    }).await;
}

#[tokio::test]
async fn client_read() {
    let _ = tracing_subscriber::fmt::try_init();

    let server = mock_symphony::http();

    let (client, connect_h) = get_connected_client(&server).await;
    let client2 = Arc::clone(&client);

    check_client_operations(client2, connect_h, async {
        let locations = client.get_locations().await.expect("No gateways found");
        let some_gateway = &locations[0].gateways[0].awl_id;
        let read_response = client.gateway_read(some_gateway).await?;
        assert!(read_response.awl_id == *some_gateway);
        Ok::<(), ClientError>(())
    }).await;

}

#[tokio::test]
#[ignore = "slow to run every time"]
async fn slow_server() {
    let _ = tracing_subscriber::fmt::try_init();

    let server = mock_symphony::http_chaos(Chaos {
        failure: FailProbability::never(),
        delay_min: Duration::from_millis(1000),
        delay_max: Duration::from_millis(5000),
    }, None);

    let (client, connect_h) = get_connected_client(&server).await;
    let client2 = Arc::clone(&client);

    check_client_operations(client2, connect_h, async {
        let locations = client.get_locations().await.expect("No gateways found");
        let some_gateway = &locations[0].gateways[0].awl_id;
        let read_response = client.gateway_read(some_gateway).await?;
        assert!(read_response.awl_id == *some_gateway);
        Ok::<(), ClientError>(())
    }).await;
}

