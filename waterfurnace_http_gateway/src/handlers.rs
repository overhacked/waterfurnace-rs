use std::sync::Arc;

use serde::Serialize;
use serde_json;
use warp::{
    self,
    Rejection,
    Reply,
};
use waterfurnace_symphony as wf;

pub async fn gateways_handler(client: Arc<wf::Client>) -> Result<impl Reply, Rejection> {
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

pub async fn zones_handler(request_awl_id: String, client: Arc<wf::Client>) -> Result<impl Reply, Rejection> {
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
                    zone_name: &zone_name,
                });
            }
        }
    }
    if request_awl_id != "*" {
        response = response.drain(..).filter(|zone| zone.gwid == request_awl_id).collect();
    }
    if response.len() == 0 {
        return Err(warp::reject::reject());
    }
    let response = serde_json::to_string(&response).unwrap();
    Ok(response)
}


#[derive(Debug)]
struct ClientError(wf::ClientError);
impl warp::reject::Reject for ClientError {}
