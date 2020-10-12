use std::convert::Infallible;
use std::sync::{
    Arc,
    atomic::{
        AtomicBool,
        Ordering,
    },
};

use warp::{
    self,
    Filter,
    reject::Reject,
    Rejection,
    Reply,
};
use waterfurnace_symphony as wf;

pub fn all(client: &Arc<wf::Client>, ready: Arc<AtomicBool>) -> impl Filter<Extract = (impl Reply,), Error = Infallible> + Clone {
        check_ready(ready)
        .and(
            gateways(client)
                .or(zones(client))
                .or(gateway_read(client))
                .or(zone_details(client))
        )
        .recover(crate::handlers::rejection)
}

fn gateways(client: &Arc<wf::Client>) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    warp::path("gateways")
        .and(warp::path::end())
        .and(warp::get())
        .and(with_client(client.clone()))
        .and_then(crate::handlers::gateways)
}

fn gateway_read(client: &Arc<wf::Client>) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    warp::path!("gateways" / String)
        .and(warp::path::end())
        .and(warp::get())
        .and(with_client(client.clone()))
        .and_then(crate::handlers::gateway_read)
}

fn zones(client: &Arc<wf::Client>) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    let without_zone_id = warp::path!("gateways" / String / "zones")
        .map(|awl_id| (awl_id, None,)).untuple_one();
    let with_zone_id = warp::path!("gateways" / String / "zones" / u8)
        .map(|awl_id, zone_id| (awl_id, Some(zone_id),)).untuple_one();

    without_zone_id.or(with_zone_id).unify()
        .and(warp::path::end())
        .and(warp::get())
        .and(with_client(client.clone()))
        .and_then(crate::handlers::zones)
}

fn zone_details(client: &Arc<wf::Client>) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    warp::path!("gateways" / String / "zones" / u8 / "details")
        .and(warp::path::end())
        .and(warp::get())
        .and(with_client(client.clone()))
        .and_then(crate::handlers::zone_details)
}

fn with_client(client: Arc<wf::Client>) -> impl Filter<Extract = (Arc<wf::Client>,), Error = Infallible> + Clone {
    warp::any()
        .map(move || client.clone() )
}

fn check_ready(ready: Arc<AtomicBool>) -> impl Filter<Extract = (), Error = Rejection> + Clone {
    warp::any()
        .map(move || ready.clone())
        .and_then(|ready: Arc<AtomicBool>| async move {
            if ready.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(warp::reject::custom(BackendUnavailable))
            }
        })
        .untuple_one()
}

#[derive(Debug)]
pub struct BackendUnavailable;

impl Reject for BackendUnavailable {}
