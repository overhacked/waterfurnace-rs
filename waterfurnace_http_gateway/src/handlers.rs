use std::sync::Arc;

use serde::Serialize;
use serde_json;
use warp::{
    self,
    Rejection,
    Reply,
};
use waterfurnace_symphony as wf;

pub async fn gateways_handler(client: Arc<wf::Client>) -> std::result::Result<impl Reply, Rejection> {
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

#[derive(Serialize, Debug)]
struct GatewayResponseItem<'a> {
    location: &'a str,
    gwid: &'a str,
    system_name: &'a str,
}

#[derive(Debug)]
struct ClientError(wf::ClientError);
impl warp::reject::Reject for ClientError {}
