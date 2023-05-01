use std::collections::HashMap;
use std::sync::Arc;

use crate::cached_client::CachedClient;
use serde::Serialize;
use serde_json::{
    self,
    Value as JsonValue
};
use std::convert::Infallible;
use tracing::error;
use warp::{
    self,
    http::StatusCode,
    Rejection,
    Reply,
};
use waterfurnace_symphony::{
    self as wf,
    ClientError as WfError,
};

pub async fn gateways(client: Arc<wf::Client>) -> Result<impl Reply, Rejection> {
    #[derive(Serialize, Debug)]
    struct GatewayResponseItem<'a> {
        location: &'a str,
        gwid: &'a str,
        system_name: &'a str,
    }

    let locations = client.get_locations().await
        .map_err(ClientError)
        .map_err(warp::reject::custom)?;
    let mut response: Vec<GatewayResponseItem> = vec![];
    for location in &locations {
        let description = &location.description;
        for gateway in &location.gateways {
            response.push(GatewayResponseItem {
                location: description,
                gwid: &gateway.awl_id,
                system_name: &gateway.description,
            });
        }
    }
    let response = serde_json::to_string(&response).unwrap();
    Ok(response)
}

pub async fn gateway_read(request_awl_id: String, client: Arc<wf::Client>) -> Result<impl Reply, Rejection> {
    let gateway_read = client.cached_gateway_read(&request_awl_id).await
        .map_err(ClientError)
        .map_err(warp::reject::custom)?;
    let response = serde_json::to_string(&gateway_read.metrics).unwrap();
    Ok(response)
}

pub async fn zones(request_awl_id: String, request_zone_id: Option<u8>, client: Arc<wf::Client>) -> Result<Box<dyn Reply>, Rejection> {
    #[derive(Serialize, Debug)]
    struct ZonesResponseItem<'a> {
        location: &'a str,
        gwid: &'a str,
        system_name: &'a str,
        #[serde(rename = "zoneid")]
        zone_id: u8,
        zone_name: &'a str,
    }

    let locations = client.get_locations().await
        .map_err(ClientError)
        .map_err(warp::reject::custom)?;
    let mut response: Vec<ZonesResponseItem> = vec![];
    for location in &locations {
        let description = &location.description;
        for gateway in &location.gateways {
            for (zone_id, zone_name) in gateway.zone_names.iter() {
                response.push(ZonesResponseItem {
                    location: description,
                    gwid: &gateway.awl_id,
                    system_name: &gateway.description,
                    zone_id: *zone_id,
                    zone_name,
                });
            }
        }
    }
    // Filter by gateway ID
    if request_awl_id != "*" {
        response = response.drain(..).filter(|zone| zone.gwid == request_awl_id).collect();
    }

    // Filter by zone ID, if specified
    if let Some(zone_id) = request_zone_id {
        if request_awl_id == "*" {
            // Not allowed to request a specific zone_id for
            // all gateways
            return Ok(Box::new(StatusCode::BAD_REQUEST));
        }
        response = response.drain(..).filter(|zone| zone.zone_id == zone_id).collect();
    }

    // If an unknown gateway or zone ID specified, then
    // the result will be an empty Vec. Return 404 instead
    // of an empty result.
    if response.is_empty() {
        return Err(warp::reject::reject());
    }

    let response = serde_json::to_string(&response).unwrap();
    Ok(Box::new(response))
}

pub async fn zone_details(request_awl_id: String, request_zone_id: u8, client: Arc<wf::Client>) -> Result<impl Reply, Rejection> {
    type ZoneMetrics<'a, T> = HashMap<&'a T, &'a JsonValue>;
    let gateway_read = client.cached_gateway_read(&request_awl_id).await
        .map_err(ClientError)
        .map_err(warp::reject::custom)?;

    // Find all zone-specific data in the gateway
    let zone_prefix = format!("iz2_z{}_", request_zone_id);
    let mut zone_metrics: ZoneMetrics<String> = gateway_read.metrics.iter()
        .filter(|(k, _)| k.starts_with(&zone_prefix) ).collect();

    // Pull e.g. $.iz2_z1_activesettings.* up to the top level
    let settings_key = format!("{}activesettings", zone_prefix);
    if let Some(JsonValue::Object(settings)) = zone_metrics.remove(&settings_key) {
        zone_metrics.extend(settings.iter());
    }

    // Strip the zone prefix
    let zone_metrics: ZoneMetrics<str> = zone_metrics.drain()
        .filter_map(|(k, v)| k.strip_prefix(&zone_prefix).map(|s| (s, v,))).collect();
    let response = serde_json::to_string(&zone_metrics).unwrap();
    Ok(response)
}

pub async fn rejection(err: Rejection) -> Result<impl Reply, Infallible> {
    let code = if err.is_not_found() {
        StatusCode::NOT_FOUND
    } else if let Some(ClientError(client_err)) = err.find::<ClientError>() {
        match client_err {
            WfError::UnknownGateway(_) => StatusCode::NOT_FOUND,
            WfError::CommandTimeout(_) => StatusCode::GATEWAY_TIMEOUT,
            _ => {
                error!("waterfurnace_symphony::ClientError: {:?}", client_err);
                StatusCode::BAD_GATEWAY
            },
        }
    } else if let Some(crate::routes::BackendUnavailable) = err.find() {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        // We should have expected this... Just log and say its a 500
        error!("unhandled custom rejection, returning 500 response: {:?}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    };

    Ok(warp::reply::with_status(warp::reply(), code))
}

#[derive(Debug)]
struct ClientError(wf::ClientError);
impl warp::reject::Reject for ClientError {}
