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
    pub(super) error: Option<String>,
    #[serde(flatten)]
    pub(super) data: ResponseType, // HashMap<String, Value>,
    #[serde(flatten)]
    pub(super) extra: HashMap<String, Value>,
}

impl Response {
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }

    pub fn is_err(&self) -> bool {
        !self.error.is_none()
    }

    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }

    pub fn data(&self) -> &ResponseType {
        &self.data
    }

    pub fn extra(&self) -> &HashMap<String, Value> {
        &self.extra
    }
}

#[derive(Deserialize, Debug)]
#[serde(tag = "rsp", rename_all = "lowercase")]
pub enum ResponseType {
    Login {
        success: bool,
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

