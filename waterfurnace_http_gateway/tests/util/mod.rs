use mock_symphony::{
    self,
    Chaos,
};
use std::time::Duration;
use tokio;
use url::Url;
use waterfurnace_http_gateway::*;

type RunGatewayResult = (Url, mock_symphony::Server,);

#[allow(dead_code)]
pub async fn run_gateway_with_mock() -> RunGatewayResult {
	_run_chaos_gateway_with_mock_on_port(None, None).await
}

#[allow(dead_code)]
pub async fn run_chaos_gateway_with_mock(chaos: Chaos) -> RunGatewayResult {
	_run_chaos_gateway_with_mock_on_port(Some(chaos), None).await
}

#[allow(dead_code)]
pub async fn run_gateway_with_mock_on_port(port: u16) -> RunGatewayResult {
	_run_chaos_gateway_with_mock_on_port(None, Some(port)).await
}

#[allow(dead_code)]
pub async fn run_chaos_gateway_with_mock_on_port(chaos: Chaos, port: u16) -> RunGatewayResult {
	_run_chaos_gateway_with_mock_on_port(Some(chaos), Some(port)).await
}

async fn _run_chaos_gateway_with_mock_on_port(chaos: Option<Chaos>, port: Option<u16>) -> RunGatewayResult {
	let listen_socket = tokio::net::TcpListener::bind(("127.0.0.1",port.unwrap_or_default())).await.unwrap();
	let addr = listen_socket.local_addr().unwrap();

	let (server_meta_tx, server_meta_rx) = tokio::sync::oneshot::channel();

	tokio::spawn(async move {
		let symphony = mock_symphony::http_chaos(chaos.unwrap_or_default(), port);
		let symphony_client = waterfurnace_symphony::Client::new_for(
			&symphony.get_login_uri(),
			&symphony.get_config_uri()
		);
		server_meta_tx.send(symphony).unwrap();
		run_incoming_with_client(symphony_client, listen_socket, "test_user", "bad7a55").await
	});

	let server_meta = server_meta_rx.await.unwrap();
	let url = Url::parse(&format!("http://{}:{}/", addr.ip(), addr.port())).unwrap();
	(url, server_meta,)
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap()
}

pub async fn get<T: reqwest::IntoUrl>(url: T) -> reqwest::Result<reqwest::Response>
{
        client().get(url).send().await
}
