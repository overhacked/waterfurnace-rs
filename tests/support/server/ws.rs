use serde::{Serialize, Deserialize};
use serde_json::{
    self,
    json,
    Value
};
use std::collections::HashMap;

type Tid = u8;

pub(super) struct MessageHandler {
    last_tid: Tid,    
}

impl MessageHandler {
    pub(super) fn new() -> Self {
        MessageHandler {
            last_tid: 0, // Client starts at 1
        }
    }

    pub(super) fn handle_websocket_message(&mut self, message: warp::ws::Message) -> warp::ws::Message {
        assert!(message.is_text());

        // Early check for simple test message
        if message.as_bytes() == super::HELLO_SEND.as_bytes() {
            return warp::ws::Message::text(super::HELLO_REPLY);
        }

        let request: Request = serde_json::from_slice(message.as_bytes())
            .expect("Could not deserialize client JSON");

        // TODO: implement test for wrapping or out-of-order transaction IDs
        assert!(
            request.transaction_id > self.last_tid,
            "Client transaction IDs did not monotonically increase: received {} but last_tid = {}",
            request.transaction_id,
            self.last_tid
        );
        self.last_tid = request.transaction_id;

        match request {
            req @ Request { command: Command::Login{..}, .. } => self.handle_login(req),
            req @ Request { command: Command::Read{..}, .. } => self.handle_read(req),
        }
    }

    fn handle_login(&mut self, request: Request) -> warp::ws::Message {
       match request {
            Request { transaction_id: tid, command: Command::Login{ session_id }, .. } => {
                assert_eq!(session_id, super::FAKE_SESSION_ID);
                let mut zone_names: HashMap<u8, String> = HashMap::new();
                for zone in 1..=super::NUM_ZONES {
                    zone_names.insert(1, format!("Zone {}", zone));
                }

                let response = Response {
                        transaction_id: tid,
                        error: "".to_string(),
                        data: ResponseType::Login {
                            success: true,
                            key: 987654321,
                            first_name: "Uther".to_string(),
                            last_name: "Pendragon".to_string(),
                            email: "uther@camelot.test".to_string(),
                            locations: vec![ResponseLoginLocations {
                                description: "Home".to_string(),
                                postal: "Cadbury Castle née Camelot".to_string(),
                                city: "Yeovil".to_string(),
                                state: "Somerset".to_string(),
                                country: "England".to_string(),
                                latitude: 50.83682,
                                longitude: -3.54466,
                                gateways: vec![ResponseLoginGateway{
                                    awl_id: super::FAKE_GWID.to_string(),
                                    description: "Great Hall".to_string(),
                                    gateway_type: ResponseGatewayType::AWL,
                                    online: 1,
                                    max_zones: super::NUM_ZONES,
                                    zone_names: zone_names, 
                                }],
                            }],
                        }
                };
                warp::ws::Message::text(serde_json::to_string(&response).unwrap())
            },
            _ => panic!("handle_login received a non-Login command")
        }
    }

    fn handle_read(&self, request: Request) -> warp::ws::Message {
       use std::collections::HashSet;

       match request {
            Request { transaction_id: tid, command: Command::Read{ awl_id, zone, rlist, .. }, .. } => {
                let mut rlist_response: HashMap<String, Value> = HashMap::new();
                rlist_response.insert("zone".to_string(), json!(zone));

                let mut request_zones = HashSet::new();

                for ref key in rlist {
                    // TODO Set all values to zero, for now, until
                    // I develop more in-depth return-value testing
                    rlist_response.insert(key.to_string(), json!(0));

                    // From private crate::client::protocol::ZONE_RLIST_PREFIX
                    if key.starts_with("iz2_z") {
                        let segments: Vec<&str> = key.split('_').collect();
                        request_zones.insert(segments[1].to_string());
                    }
                }

                assert_eq!(request_zones.len(), super::NUM_ZONES as usize, "Wrong number of request zones");

                let response = Response {
                    transaction_id: tid,
                    error: "".to_string(),
                    data: ResponseType::Read {
                        awl_id: awl_id,
                        metrics: rlist_response,
                    }
                };
                warp::ws::Message::text(serde_json::to_string(&response).unwrap())
            },
            _ => panic!("handle_read received a non-Read command")
        }
    }
}

#[derive(Deserialize, Debug)]
struct Request {
    #[serde(rename = "tid")]
    pub transaction_id: u8,
    #[serde(flatten)]
    pub command: Command,
    pub source: String,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "cmd", rename_all = "lowercase")]
enum Command {
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
pub struct Response {
    #[serde(rename = "tid")]
    pub(super) transaction_id: u8,
    #[serde(rename = "err", default, deserialize_with="non_empty_str")]
    pub(super) error: String,
    #[serde(flatten)]
    pub(super) data: ResponseType, // HashMap<String, Value>,
}

#[derive(Serialize, Debug)]
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

#[derive(Serialize, Debug, Clone)]
pub struct ResponseLoginLocations {
    pub description: String,
    pub postal: String,
    pub city: String,
    pub state: String,
    pub country: String,
    pub latitude: f64,
    pub longitude: f64,
    pub gateways: Vec<ResponseLoginGateway>,
}

#[derive(Serialize, Debug, Clone)]
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
}

#[derive(Serialize, Debug, Clone)]
pub enum ResponseGatewayType {
    AWL,
}
