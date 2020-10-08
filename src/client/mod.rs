pub mod protocol;

use serde_json;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio::time::{timeout, Elapsed, Instant};
use tokio_tungstenite::tungstenite::Error as TungsteniteError;
use tracing;
use tracing_futures;
use tracing::{trace, debug, info, warn, error};

pub use protocol::{
    Command,
    Response,
};

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::Waiter;
use crate::session::{
    Message,
    Session,
    SessionError,
    state,
};

const LOGIN_URI: &str = "https://symphony.mywaterfurnace.com/account/login";
const AWLCONFIG_URI: &str = "https://symphony.mywaterfurnace.com/assets/js/awlconfig.js.php";
// Taken from setTimeout(1500000, ...) in Symphony JavaScript
const SESSION_TIMEOUT: Duration = Duration::from_millis(1500000);

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

type Tid = u8;

#[derive(Debug)]
struct TransactionList {
    last: Tid,
    list: HashMap<Tid, oneshot::Sender<Result<Response>>>,
}

#[derive(Debug)]
pub struct Client {
    ready: Waiter,
    socket: Mutex<Option<mpsc::UnboundedSender<String>>>,
    transactions: Arc<Mutex<TransactionList>>,
    gateways: RwLock<HashMap<String, protocol::ResponseLoginGateway>>,
}

impl Client {
    pub fn new()
        -> Client
    {
        Client {
            ready: Waiter::new(false),
            socket: Mutex::new(None),
            transactions: Arc::new(Mutex::new(TransactionList {
                last: Tid::MAX, // will wrap on first transaction
                list: HashMap::new(),
            })),
            gateways: RwLock::new(HashMap::new()),
        }
    }

    async fn next_transaction_id(&self) -> Result<Tid> {
        let transactions = &mut self.transactions.lock().await;
        let first_candidate_tid =
            if transactions.last < Tid::MAX { transactions.last + 1 }
            else { 1 };
        let mut candidate_tid = first_candidate_tid;
        transactions.last = loop {
            if !transactions.list.contains_key(&candidate_tid) {
                break candidate_tid;
            }

            if candidate_tid < Tid::MAX {
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
    pub async fn connect(&self, username: String, password: String)
        -> std::result::Result<(), DynError>
    {
        use state::LoggedIn;

        let mut session =
            Session::new(LOGIN_URI, AWLCONFIG_URI)
            .login(&username, &password).await?
            .connect().await?;
        let (tx_client, mut tx_session) = mpsc::unbounded_channel();
        *(self.socket.lock().await) = Some(tx_client);

        // TODO: implement retry logic
        loop {
            let session_id = session.get_token().to_owned();
            let join_result = tokio::try_join!(
                self.handle_messages(session, &mut tx_session),
                self.login(&session_id),
            )?;
            session = join_result.0;
            info!("Renewing session");
            let disconnected_session = session.logout().await?;
            self.reset_transactions().await;
            session = disconnected_session
                .login().await?
                .connect().await?;
            info!("Session renewed");
        }
    }

    async fn handle_messages(&self, session: Session<state::Connected>, tx_channel: &mut mpsc::UnboundedReceiver<String>)
        -> Result<Session<state::Connected>>
    {
        let timeout_at = Instant::now() + SESSION_TIMEOUT;
        loop {
            tokio::select! {
                Some(msg) = session.next() => {
                    debug!("Received message: {:?}", msg);
                    match msg? {
                        Message::Text(json) => {
                            trace!(%json);
                            match serde_json::from_str::<Response>(&json) {
                                Ok(response) => self.commit_transaction(response).await,
                                Err(e) => error!(json = %json, error = %e, "failed to deserialize response"),
                            }
                        },
                        _ => continue,
                    }
                },
                Some(json) = tx_channel.recv() => {
                    debug!("Sending message: {:?}", json);
                    if let Err(e) = session.send_text(json).await {
                        error!(error = %e, "failed to send message");
                    }
                },
                _ = tokio::time::delay_until(timeout_at) => {
                    debug!("Session timeout");
                    self.ready.set_unready();
                    break;
                },
            };
        }

        Ok(session)
    }

    #[tracing::instrument]
    async fn commit_transaction(&self, response: Response) {
        let mut transactions_lock = self.transactions.lock().await;
        let transaction = transactions_lock.list.remove(&response.transaction_id);
        drop(transactions_lock);

        match transaction {
            Some(receiver) => {
                debug!(?response);
                let send_result = receiver.send(
                    match response.error {
                        Some(ref msg) => Err(ClientError::ResponseError(msg.to_string(), response)),
                        None    => Ok(response),
                    }
                );
                if let Err(response) = &send_result {
                    warn!(
                        transaction_id = response.as_ref().unwrap().transaction_id,
                        response = ?response,
                        "transaction was not awaited"
                    );
                }
            },
            None => warn!(response.transaction_id, response = ?response, "received response for invalid transaction"),
        }
    }

    #[tracing::instrument]
    async fn reset_transactions(&self) {
        let transactions = &mut self.transactions.lock().await;
        transactions.list.clear();
        transactions.last = Tid::MAX; // Will wrap on first transaction
        debug!("Transactions reset");
    }

    #[tracing::instrument]
    pub async fn ready(&self) {
        self.ready.wait_ready().await
    }

    #[tracing::instrument]
    async fn login(&self, session_id: &str)
        -> Result<Response>
    {
        let receiver = self.send(Command::Login {
            session_id: session_id.to_string(),
        }).await?;
        let login_result = timeout(COMMAND_TIMEOUT, receiver).await?
            .or(Err(ClientError::CommandFailed("login".to_string())))?;
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

        self.ready.set_ready();
        info!(gateways = ?*gateways_lock, "Successful login");
        Ok(login_response)
    }

    #[tracing::instrument]
    pub async fn gateway_read(&self, awl_id: &str)
        -> Result<Response>
    {
        self.ready.wait_ready_timeout(COMMAND_TIMEOUT).await?;

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
        let read_result = timeout(COMMAND_TIMEOUT, receiver).await?
            .or(Err(ClientError::CommandFailed("read".to_string())))?;
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

    #[error("Command timed out after {0}")]
    CommandTimeout(#[from] Elapsed),

    #[error("Response error: {0}")]
    ResponseError(String, Response),

    #[error("Transaction failed: {0}")]
    TransactionFailed(Tid),

    #[error("Maximum 255 transactions in progress")]
    TooManyTransactions,

    #[error(transparent)]
    SessionError(#[from] SessionError),

    #[error("Could not send command")]
    SendError(#[from] mpsc::error::SendError<String>),

    #[error("Unknown gateway ID: {0}")]
    UnknownGateway(String),

    #[error(transparent)]
    WebSockets(#[from] TungsteniteError),
}
