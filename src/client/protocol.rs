use serde::{Serialize, Deserialize};
use serde_json::Value;

use std::collections::HashMap;
use std::vec::Vec;

pub(super) const COMMAND_SOURCE: &str = "consumer dashboard";

pub(super) const DEFAULT_READ_RLIST: [&str; 34] = [
    "ActualCompressorSpeed",
    "AirflowCurrentSpeed",
    "AOCEnteringWaterTemp",
    "AuroraOutputCC",
    "AuroraOutputCC2",
    "AuroraOutputEH1",
    "AuroraOutputEH2",
    "auroraoutputrv",
    "auxpower",
    "AWLABCType",
    "AWLTStatType",
    "compressorpower",
    "dehumid_humid_sp",
    "EnteringWaterTemp",
    "fanpower",
    "homeautomationalarm1",
    "homeautomationalarm2",
    "humidity_offset_settings",
    "iz2_dehumid_humid_sp",
    "iz2_humidity_offset_settings",
    "lastfault",
    "lastlockout",
    "LeavingAirTemp",
    "lockoutstatus",
    "looppumppower",
    "ModeOfOperation",
    "totalunitpower",
    "TStatActiveSetpoint",
    "TStatCoolingSetpoint",
    "TStatDehumidSetpoint",
    "TStatHeatingSetpoint",
    "TStatMode",
    "TStatRelativeHumidity",
    "TStatRoomTemp",
];

pub(super) const ZONE_RLIST_PREFIX: &str = "iz2_z";
pub(super) const DEFAULT_ZONE_RLIST_SUFFIXES: [&str; 2] = [
    "_roomtemp",
    "_activesettings",
];

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
        rlist: Vec<String>,
    },
}

#[derive(Serialize, Debug)]
pub(super) struct Request {
    #[serde(rename = "tid")]
    pub transaction_id: super::Tid,
    #[serde(flatten)]
    pub command: Command,
    pub source: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "rsp", rename_all = "lowercase")]
pub enum Response {
    Login(LoginResponse),
    Read(ReadResponse),
}

impl Response {
    pub fn transaction_id(&self) -> super::Tid {
        match self {
            Response::Login(r) => r.meta.transaction_id,
            Response::Read(r) => r.meta.transaction_id,
        }
    }

    pub fn error(&self) -> Option<String> {
        match self {
            Response::Login(r) => r.meta.error.clone(),
            Response::Read(r) => r.meta.error.clone(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct ResponseMeta {
    #[serde(rename = "tid")]
    pub(super) transaction_id: super::Tid,
    #[serde(rename = "err", default, deserialize_with="non_empty_str")]
    pub(super) error: Option<String>,
    #[serde(flatten)]
    pub(super) _extra: HashMap<String, Value>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct LoginResponse {
    #[serde(flatten)]
    pub meta: ResponseMeta,
    pub success: bool,
    #[serde(rename = "firstname")]
    pub first_name: String,
    #[serde(rename = "lastname")]
    pub last_name: String,
    #[serde(rename = "emailaddress")]
    pub email: String,
    pub key: u64,
    pub locations: Vec<ResponseLoginLocations>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ReadResponse {
    #[serde(flatten)]
    pub meta: ResponseMeta,
    #[serde(rename = "awlid")]
    pub awl_id: String,
    #[serde(flatten)]
    pub metrics: HashMap<String, Value>,
}



#[derive(Deserialize, Debug, Clone)]
pub struct ResponseLoginLocations {
    pub description: String,
    pub postal: String,
    pub city: String,
    pub state: String,
    pub country: String,
    pub latitude: f64,
    pub longitude: f64,
    pub gateways: Vec<ResponseLoginGateway>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ResponseLoginGateway {
    #[serde(rename = "gwid")]
    pub awl_id: String,
    pub description: String,
    #[serde(rename = "type")]
    pub gateway_type: ResponseGatewayType,
    pub online: u8,
    #[serde(rename = "iz2_max_zones")]
    pub max_zones: u8,
    #[serde(rename = "tstat_names", deserialize_with="zone_name_list")]
    pub zone_names: HashMap<u8, String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Deserialize, Debug, Clone)]
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

