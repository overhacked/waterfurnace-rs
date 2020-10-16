mod util;

use mock_symphony::{
    self,
    Chaos,
    FailProbability,
};
use std::sync::Once;
use std::time::Duration;
use tokio::{
	self,
	time::{delay_for, timeout},
};
use tokio_test::{assert_ok, assert_err};
use tracing::debug;
use tracing_subscriber::{
	filter::EnvFilter,
	fmt::format::FmtSpan,
};
use util::get;
use warp::http::StatusCode;

static INIT_TRACING: Once = Once::new();

#[tokio::test]
async fn login_success() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

	// NOTE TO FUTURE SELF: don't make _server a bare underscore,
	// or the mock_symphony::Server will be dropped before it even
	// starts.
	let (base, _server) = util::run_gateway_with_mock().await;

	let gateways_uri = base.join("/gateways")?;
	get(gateways_uri).await?
		.error_for_status()?;

	Ok(())
}

#[tokio::test]
async fn login_failure() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

	let (base, _server) = util::run_chaos_gateway_with_mock(Chaos::always_fail()).await;

	let gateways_uri = base.join("/gateways")?;
	assert_eq!(get(gateways_uri.clone()).await?.status(), StatusCode::GATEWAY_TIMEOUT);
	// Wait another second, because after the above request
	// times out, the server should also have timed out logging
	// into Symphony and start reporting 503 Service Unavailable
	delay_for(Duration::from_secs(1)).await;
	assert_eq!(get(gateways_uri.clone()).await?.status(), StatusCode::SERVICE_UNAVAILABLE);


	Ok(())
}

#[tokio::test]
async fn server_goes_away() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

	let (base, pre_failure) = util::run_gateway_with_mock().await;

	let gateways_uri = base.join("/gateways")?;
	assert_eq!(get(gateways_uri.clone()).await?.status(), StatusCode::OK);

	let read_uri = base.join(&format!("/gateways/{}", mock_symphony::FAKE_GWID))?;
	assert_eq!(get(read_uri.clone()).await?.status(), StatusCode::OK);

	debug!("Shutting down mock server");
	let port = pre_failure.addr().port();
	pre_failure.shutdown();

	// Wait long enough for the cache to expire: see src/cached_client.rs
	delay_for(Duration::from_secs(11)).await;

	assert_eq!(get(read_uri.clone()).await?.status(), StatusCode::SERVICE_UNAVAILABLE);

	debug!("Starting back mock server on same port");
	let _post_failure = mock_symphony::http_chaos(Chaos::default(), Some(port));
	// Wait long enough for the server to actually start, and the client's
	// exponential backoff to recover
	delay_for(Duration::from_secs(10)).await;

	debug!("Expecting read request to succeed");
	assert_eq!(get(read_uri.clone()).await?.status(), StatusCode::OK);

	Ok(())
}

#[tokio::test]
async fn cache_is_working() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

	let chaos = Chaos::new().fail_at(FailProbability::from_percent(40));
	let (base, _server) = util::run_chaos_gateway_with_mock(chaos).await;

	let read_uri = base.join(&format!("/gateways/{}", mock_symphony::FAKE_GWID))?;
	let mut request_interval = tokio::time::interval(Duration::from_millis(250));

	// Submit requests up to 30 seconds, until one succeeds
	assert_ok!(
		timeout(Duration::from_secs(30), async {
			loop {
				if get(read_uri.clone()).await.unwrap().status() == StatusCode::OK {
					break;
				}
				request_interval.tick().await;
			}
		}).await
	);

	// Now, submit requests for less than the caching period, and
	// they should all succeed, even though the mock server is still flaky
	assert_err!(
		timeout(Duration::from_secs(5), async {
			loop {
				debug!("Sending expected successful request");
				assert_eq!(get(read_uri.clone()).await.unwrap().status(), StatusCode::OK);
				request_interval.tick().await;
			}
		}).await
	);
	Ok(())
}

fn init_tracing() {
	INIT_TRACING.call_once(|| {
		let mut subscriber = tracing_subscriber::fmt()
			.with_env_filter(EnvFilter::from_default_env());
		subscriber = match std::env::var("TRACING_SPANS") {
			Ok(val) if val != "" => subscriber.with_span_events(FmtSpan::ACTIVE),
			_ => subscriber,
		};
		subscriber
			.try_init()
			.expect("Could not initialize tracing_subscriber");
	});
}
