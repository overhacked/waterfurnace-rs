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
	time::delay_for,
};
use tokio_test::{assert_ok, assert_err};
use tracing::{debug, info};
use tracing_subscriber::{
	filter::EnvFilter,
	fmt::format::FmtSpan,
};
use util::get;
use warp::http::StatusCode;
use waterfurnace_http_gateway::*;

static INIT_TRACING: Once = Once::new();

#[tokio::test]
async fn login_success() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

	let (base, gateway_h) = util::run_gateway_with_mock(None).await;

	let gateways_uri = base.join("/gateways")?;
	let gateways_response = get(gateways_uri).await?
		.error_for_status()?;

	Ok(())
}

#[tokio::test]
async fn login_failure() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

	let (base, gateway_h) = util::run_gateway_with_mock(Some(Chaos::always_fail())).await;

	let gateways_uri = base.join("/gateways")?;
	assert_eq!(get(gateways_uri.clone()).await?.status(), StatusCode::GATEWAY_TIMEOUT);
	// Wait another second, because after the above request
	// times out, the server should also have timed out logging
	// into Symphony and start reporting 503 Service Unavailable
	delay_for(Duration::from_secs(1)).await;
	assert_eq!(get(gateways_uri.clone()).await?.status(), StatusCode::SERVICE_UNAVAILABLE);


	Ok(())
}

fn init_tracing() {
	INIT_TRACING.call_once(|| {
		tracing_subscriber::fmt()
			.with_env_filter(EnvFilter::from_default_env())
			.with_span_events(FmtSpan::ACTIVE)
			.try_init()
			.expect("Could not initialize tracing_subscriber");
	});
}
