use serde::{Serialize, Deserialize};
use serde_json::Value;

use std::collections::HashMap;
use std::vec::Vec;

pub(super) const COMMAND_SOURCE: &str = "consumer dashboard";

#[derive(Serialize, Debug)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Command {
    Login {
        #[serde(rename = "sessionid")]
        session_id: String
    },
    Read {
        #[serde(rename = "awlid")]
        awl_id: String,
        zone: u8,
    },
}

#[derive(Serialize, Debug)]
pub(super) struct Request {
    #[serde(rename = "tid")]
    pub transaction_id: u8,
    #[serde(flatten)]
    pub command: Command,
    pub source: String,
}

#[derive(Deserialize, Debug)]
pub struct Response {
    #[serde(rename = "tid")]
    pub(super) transaction_id: u8,
    #[serde(rename = "err", default, deserialize_with="non_empty_str")]
    pub error: Option<String>,
    pub success: bool,
    #[serde(flatten)]
    pub data: ResponseType, // HashMap<String, Value>,
    /*
    #[serde(flatten)]
    extra: HashMap<String, Value>,
    */
}

#[derive(Deserialize, Debug)]
#[serde(tag = "rsp", rename_all = "lowercase")]
pub enum ResponseType {
    Login {
        #[serde(rename = "firstname")]
        first_name: String,
        #[serde(rename = "lastname")]
        last_name: String,
        #[serde(rename = "emailaddress")]
        email: String,
        key: u64,
        locations: Vec<ResponseLoginLocations>,
    },
    Read {
        #[serde(rename = "awlid")]
        awl_id: String,
        #[serde(flatten)]
        metrics: HashMap<String, Value>,
    },
}

#[derive(Deserialize, Debug)]
pub struct ResponseLoginLocations {
    description: String,
    postal: String,
    city: String,
    state: String,
    country: String,
    latitude: f64,
    longitude: f64,
    gateways: Vec<ResponseLoginGateway>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Deserialize, Debug)]
pub struct ResponseLoginGateway {
    #[serde(rename = "gwid")]
    awl_id: String,
    description: String,
    #[serde(rename = "type")]
    gateway_type: ResponseGatewayType,
    online: u8,
    #[serde(rename = "iz2_max_zones")]
    max_zones: u8,
    #[serde(rename = "tstat_names", deserialize_with="zone_name_list")]
    zone_names: HashMap<u8, String>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Deserialize, Debug)]
pub enum ResponseGatewayType {
    AWL,
    #[serde(other)]
    Other,
}

fn non_empty_str<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    let o: Option<String> = Option::deserialize(d)?;
    Ok(o.filter(|s| !s.is_empty()))
}

fn zone_name_list<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<HashMap<u8, String>, D::Error> {
    use serde::de::Error;
    let m: serde_json::Map<String, Value> = Deserialize::deserialize(d)?;
    let mut out = HashMap::<u8, String>::new();
    for (key, value) in m.iter() {
        let index = match key.strip_prefix("z") {
            None => continue,
            Some(k) => k.parse().or(Err(D::Error::custom("Invalid index")))?,
        };
        if let serde_json::Value::String(s) = value {
            out.insert(index, s.into());
        }
    }
    Ok(out)
}

