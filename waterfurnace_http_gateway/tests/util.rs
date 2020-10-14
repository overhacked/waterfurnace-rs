use mock_symphony::{
    self,
    Chaos,
};
use std::time::Duration;
use tokio;
use url::Url;
use waterfurnace_http_gateway::*;

pub async fn run_gateway_with_mock(chaos: Option<Chaos>) -> (Url, tokio::task::JoinHandle<Result<(), ServerError>>,) {
	let chaos = match chaos {
		None => Chaos::default(),
		Some(c) => c,
	};
	let listen_socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listen_socket.local_addr().unwrap();

	let gateway_h = tokio::spawn(async move {
		let symphony = mock_symphony::http_chaos(chaos);
		let symphony_client = waterfurnace_symphony::Client::new_for(
			&symphony.get_login_uri(),
			&symphony.get_config_uri()
		);
		run_incoming_with_client(symphony_client, listen_socket, "test_user", "bad7a55").await
	});

	let url = Url::parse(&format!("http://{}:{}/", addr.ip(), addr.port())).unwrap();
	(url, gateway_h,)
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
