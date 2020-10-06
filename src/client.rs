use serde::{Serialize, Deserialize};
use serde_json::{self, Value};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use tracing;
use tracing_futures;
use tracing::{trace, debug, warn, error};

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::vec::Vec;

use crate::SessionManager;
use crate::session::{SessionError, Result as SessionResult};

const COMMAND_SOURCE: &str = "consumer dashboard";

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
struct Request {
    #[serde(rename = "tid")]
    transaction_id: u8,
    #[serde(flatten)]
    command: Command,
    source: String,
}

#[derive(Deserialize, Debug)]
pub struct Response {
    #[serde(rename = "tid")]
    transaction_id: u8,
    #[serde(rename = "err", default, deserialize_with="non_empty_str")]
    error: Option<String>,
    success: bool,
    #[serde(flatten)]
    data: ResponseType, // HashMap<String, Value>,
    /*
    #[serde(flatten)]
    extra: HashMap<String, Value>,
    */
}

#[derive(Deserialize, Debug)]
#[serde(tag = "rsp", rename_all = "lowercase")]
enum ResponseType {
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
    use serde::Deserialize;
    let o: Option<String> = Option::deserialize(d)?;
    Ok(o.filter(|s| !s.is_empty()))
}

fn zone_name_list<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<HashMap<u8, String>, D::Error> {
    use serde::{Deserialize, de::Error};
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

#[derive(Debug)]
struct TransactionList {
    last: u8, // will wrap to 0 on first transaction
    list: HashMap<u8, oneshot::Sender<Result<Response>>>,
}

#[derive(Debug)]
pub struct Client {
    session: SessionManager,
    ready: Notify,
    socket: Mutex<Option<mpsc::UnboundedSender<String>>>,
    transactions: Arc<Mutex<TransactionList>>,
}

impl Client {
    pub fn new(username: String, password: String)
        -> Client
    {
        Client {
            session: SessionManager::new(&username, &password),
            ready: Notify::new(),
            socket: Mutex::new(None),
            transactions: Arc::new(Mutex::new(TransactionList {
                last: u8::MAX,
                list: HashMap::new(),
            })),
        }
    }

    async fn next_transaction_id(&self) -> Result<u8> {
        let transactions = &mut self.transactions.lock().await;
        let first_candidate_tid =
            if transactions.last < u8::MAX { transactions.last + 1 }
            else { 1 };
        let mut candidate_tid = first_candidate_tid;
        transactions.last = loop {
            if !transactions.list.contains_key(&candidate_tid) {
                break candidate_tid;
            }

            if candidate_tid < u8::MAX {
                candidate_tid += 1;
            } else {
                candidate_tid = 1;
            }

            if candidate_tid == first_candidate_tid {
                return Err(ClientError::TooManyTransactions);
            }
        };

        Ok(transactions.last)
    }

    #[tracing::instrument]
    pub async fn connect(&self)
        -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync + 'static>>
    {
        self.session.login().await.map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(ClientError::SessionError(e)))?;
        let (tx, tx_session) = mpsc::unbounded_channel(); 
        let (rx_session, mut rx) = mpsc::unbounded_channel(); 
        *(self.socket.lock().await) = Some(tx);
        // TODO: implement retry logic
        let result = tokio::try_join!(
            self.session.stream(tx_session, rx_session),
            async { Ok(self.ready.notify()) },
            async {
                while let Some(json) = rx.recv().await {
                    // TODO: implement receive logic
                    trace!(%json);
                    match serde_json::from_str::<Response>(&json) {
                        Ok(response) => {
                            let mut transactions = self.transactions.lock().await;
                            match transactions.list.remove(&response.transaction_id) {
                                Some(receiver) => {
                                    debug!(?response);
                                    receiver.send(Ok(response));
                                }, // TODO: implement Err for {"err":"*"}
                                None => warn!(response.transaction_id, json = %json, "received response for invalid transaction"),
                            }
                        },
                        Err(e) => error!(json = %json, error = %e, "failed to deserialize response"),
                    }
                }
                warn!("End of message queue");
                Err::<(), SessionError>(SessionError::Pipe)
            },
        );
        if let Err(e) = result {
            return Err(Box::from(ClientError::SessionError(e)));
        }
        Ok(())
    }

    #[tracing::instrument]
    pub async fn ready(&self) {
        self.ready.notified().await
    }

    #[tracing::instrument]
    pub async fn login(&self)
        -> Result<()>
    {
        let session_id = self.session.get_token().await?;
        let receiver = self.send(Command::Login { // TODO: do something with receiver
            session_id: session_id,
        }).await?;
        receiver.await;
        Ok(())
    }

    #[tracing::instrument]
    pub async fn send(&self, command: Command)
        -> Result<oneshot::Receiver<Result<Response>>>
    {
        let request = Request {
            transaction_id: self.next_transaction_id().await?,
            command: command,
            source: COMMAND_SOURCE.to_string(),
        };

        let (sender, receiver) = oneshot::channel();
        let socket_lock = self.socket.lock().await;
        let socket = socket_lock.as_ref().ok_or(ClientError::CommandFailed("Socket disconnected".to_string()))?;
        let json = serde_json::to_string(&request).unwrap();
        trace!(%json);
        socket.send(json)
            .map_err(ClientError::SendError)?;
        
        let mut transactions = self.transactions.lock().await;
        transactions.list.insert(request.transaction_id, sender);
        trace!("Transaction {} inserted", request.transaction_id);

        Ok(receiver)
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("Command failed: {0}")]
    CommandFailed(String),

    #[error("Maximum 255 transactions in progress")]
    TooManyTransactions,

    #[error(transparent)]
    SessionError(#[from] SessionError),

    #[error("Could not send command")]
    SendError(#[from] mpsc::error::SendError<String>),
}
