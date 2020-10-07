pub mod protocol;

use serde_json;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Mutex, Notify, RwLock};
use tracing;
use tracing_futures;
use tracing::{trace, debug, warn, error};

pub use protocol::{
    Command,
    Response,
};

use std::collections::HashMap;
use std::sync::Arc;

use crate::SessionManager;
use crate::manager::ManagerError;

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
    gateways: RwLock<HashMap<String, protocol::ResponseLoginGateway>>,
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
            gateways: RwLock::new(HashMap::new()),
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
        -> std::result::Result<(), DynError>
    {
        self.session.login().await?;
        let (tx, tx_session) = mpsc::unbounded_channel(); 
        let (rx_session, mut rx) = mpsc::unbounded_channel(); 
        *(self.socket.lock().await) = Some(tx);
        // TODO: implement retry logic
        tokio::try_join!(
            self.session.stream(tx_session, rx_session),
            async { Ok(self.ready.notify()) },
            async {
                while let Some(json) = rx.recv().await {
                    trace!(%json);
                    match serde_json::from_str::<Response>(&json) {
                        Ok(response) => {
                            let mut transactions = self.transactions.lock().await;
                            match transactions.list.remove(&response.transaction_id) {
                                Some(receiver) => {
                                    debug!(?response);
                                    let send_result = receiver.send(Ok(response));
                                    if let Err(r) = &send_result {
                                        warn!(
                                            transaction_id = r.as_ref().unwrap().transaction_id,
                                            json = %json,
                                            "transaction was not awaited"
                                        );
                                    }
                                }, // TODO: implement Err for {"err":"*"}
                                None => warn!(response.transaction_id, json = %json, "received response for invalid transaction"),
                            }
                        },
                        Err(e) => error!(json = %json, error = %e, "failed to deserialize response"),
                    }
                }
                debug!("End of message queue");
                Ok(())
            },
        )?;

        Ok(())
    }

    #[tracing::instrument]
    pub async fn ready(&self) {
        self.ready.notified().await
    }

    #[tracing::instrument]
    pub async fn login(&self)
        -> Result<Response>
    {
        let session_id = self.session.get_token().await?;
        let receiver = self.send(Command::Login {
            session_id: session_id,
        }).await?;
        let login_result = receiver.await.or(Err(ClientError::CommandFailed("login".to_string())))?;
        let login_response = login_result?;
        let mut gateways_lock = self.gateways.write().await;
        gateways_lock.clear();
        if let protocol::ResponseType::Login{ ref locations, .. } = login_response.data() {
            for location in locations {
                for gateway in &location.gateways {
                    gateways_lock.insert(gateway.awl_id.clone(), gateway.clone());
                }
            }
        }
        Ok(login_response)
    }

    #[tracing::instrument]
    pub async fn gateway_read(&self, awl_id: &str)
        -> Result<Response>
    {
        let gateways_lock = self.gateways.read().await;
        let gateway = gateways_lock.get(awl_id).ok_or(ClientError::UnknownGateway(awl_id.to_string()))?;
        let max_zones = gateway.max_zones;
        drop(gateways_lock);

        let zone_metrics_count = (max_zones as usize) * protocol::DEFAULT_ZONE_RLIST_SUFFIXES.len();
        let gateway_metrics_count = protocol::DEFAULT_READ_RLIST.len();
        let mut gateway_metrics = Vec::with_capacity(gateway_metrics_count + zone_metrics_count);

        for metric in protocol::DEFAULT_READ_RLIST.iter() {
            gateway_metrics.push(metric.to_string());
        }

        for zone_id in 1..max_zones {
            for suffix in protocol::DEFAULT_ZONE_RLIST_SUFFIXES.iter() {
                let zone_metric = format!("{}{}{}", protocol::ZONE_RLIST_PREFIX, zone_id, suffix);
                gateway_metrics.push(zone_metric);
            }
        }

        let request = Command::Read {
            awl_id: awl_id.to_string(),
            zone: 0,
            rlist: gateway_metrics,
        };

        let receiver = self.send(request).await?;
        let read_result = receiver.await.or(Err(ClientError::CommandFailed("read".to_string())))?;
        Ok(read_result?)
    }

    #[tracing::instrument]
    pub async fn send(&self, command: Command)
        -> Result<oneshot::Receiver<Result<Response>>>
    {
        let request = protocol::Request {
            transaction_id: self.next_transaction_id().await?,
            command: command,
            source: protocol::COMMAND_SOURCE.to_string(),
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

type DynError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("Command failed: {0}")]
    CommandFailed(String),

    #[error("Transaction failed: {0}")]
    TransactionFailed(u8),

    #[error("Maximum 255 transactions in progress")]
    TooManyTransactions,

    #[error(transparent)]
    SessionError(#[from] ManagerError),

    #[error("Could not send command")]
    SendError(#[from] mpsc::error::SendError<String>),

    #[error("Unknown gateway ID: {0}")]
    UnknownGateway(String),
}
