pub mod protocol;

use ready_waiter::Waiter;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio::time::{timeout, Instant};
use tokio_tungstenite::tungstenite::Error as TungsteniteError;
use tracing::{trace, debug, info, warn, error, field, Span, Instrument as _};

pub use protocol::{
    Command,
    Response,
    LoginResponse,
    ReadResponse,
};

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
pub type ConnectResult = std::result::Result<(), Box<dyn std::error::Error + Send + Sync + 'static>>;

type Transaction = oneshot::Sender<Result<Response>>;
#[derive(Debug)]
struct TransactionList {
    last: Tid,
    list: HashMap<Tid, Transaction>,
}

#[derive(Debug)]
struct LoginData {
    zone_counts: HashMap<String, u8>,
    locations: Vec<protocol::ResponseLoginLocations>,
}

impl From<protocol::LoginResponse> for LoginData {
    fn from(response: protocol::LoginResponse) -> Self {
        let mut zone_counts: HashMap<String, u8> = HashMap::new();
        for location in &response.locations {
            for gateway in &location.gateways {
                zone_counts.insert(gateway.awl_id.clone(), gateway.max_zones);
            }
        }

        LoginData {
            zone_counts,
            locations: response.locations,
        }
    }
}

#[derive(Debug)]
pub struct Client {
    ready: Waiter,
    shutdown: Waiter,
    socket: Mutex<Option<mpsc::UnboundedSender<String>>>,
    transactions: Arc<Mutex<TransactionList>>,
    login_data: RwLock<Option<LoginData>>,
    awl_uri: String,
    awl_config_uri: String,
}

impl Client {
    pub fn new() -> Self {
        Client::new_for(LOGIN_URI, AWLCONFIG_URI)
    }

    pub fn new_for(awl_login_uri: &str, awl_config_uri: &str)
        -> Self
    {
        Client {
            ready: Waiter::new(false),
            shutdown: Waiter::new(false),
            socket: Mutex::new(None),
            transactions: Arc::new(Mutex::new(TransactionList {
                last: Tid::MAX, // will wrap on first transaction
                list: HashMap::new(),
            })),
            login_data: RwLock::new(None),
            awl_uri: awl_login_uri.to_string(),
            awl_config_uri: awl_config_uri.to_string(),
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

    #[tracing::instrument(skip(self, username, password), err)]
    pub async fn connect(&self, username: String, password: String)
        -> ConnectResult
    {
        use state::LoggedIn;

        info!("Connecting Session");
        let mut session =
            Session::new(&self.awl_uri, &self.awl_config_uri)
            .login(&username, &password).await?
            .connect().await?;
        let (tx_client, mut tx_session) = mpsc::unbounded_channel();
        *(self.socket.lock().await) = Some(tx_client);

        loop {
            let session_id = session.get_token().to_owned();
            let join_result = tokio::try_join!(
                self.handle_messages(session, &mut tx_session),
                self.login(&session_id),
            );
            session = match join_result {
                Ok(j) => {
                    // Return the Ok() result of handle_messages()
                    j.0
                },
                Err(e) => {
                    self.ready.set_unready();
                    if let ClientError::ExpectedTermination = e {
                        return Ok(());
                    } else {
                        return Err(e.into());
                    }
                }
            };
            info!("Renewing session");
            let disconnected_session = session.logout().await?;
            self.reset_transactions().await;
            session = disconnected_session
                .login().await?
                .connect().await?;
            info!("Session renewed");
        }
    }

    #[tracing::instrument(skip(self, session, tx_channel), level = "trace")]
    async fn handle_messages(&self, session: Session<state::Connected>, tx_channel: &mut mpsc::UnboundedReceiver<String>)
        -> Result<Session<state::Connected>>
    {
        let timeout_at = Instant::now() + SESSION_TIMEOUT;
        loop {
            tokio::select! {
                next_result = session.next() => {
                    match next_result? {
                        Some(msg) => {
                            let message_span = tracing::info_span!(
                                parent: None,
                                "client receive",
                                otel.kind = "consumer",
                                messaging.system = "symphony",
                                messaging.destination = "client",
                                messaging.protocol = "WebSocket",
                                messaging.conversation_id = field::Empty,
                            );

                            self.process_message(msg)
                                .instrument(message_span)
                                .await;
                        },
                        None => return Err(ClientError::ConnectionClosed),
                    }
                },
                Some(json) = tx_channel.recv() => {
                    if let Err(e) = session.send_text(json).await {
                        error!(error = %e, "failed to send message");
                    }
                },
                _ = tokio::time::sleep_until(timeout_at) => {
                    info!("Planned session timeout");
                    self.ready.set_unready();
                    break;
                },
                _ = self.shutdown.wait_ready() => {
                    info!("Shutdown requested");
                    self.ready.set_unready();
                    session.logout().await?;
                    self.reset_transactions().await;
                    // Shutdown finished
                    self.shutdown.set_unready();
                    return Err(ClientError::ExpectedTermination);
                },
            };
        }

        Ok(session)
    }

    // This function is instrumented, above, in the body of
    // handle_messages, because the #[instrument] macro doesn't
    // support the use of the parent argument to create a new
    // root span for a function.
    async fn process_message(&self, message: Message) {
        let span = Span::current();

        // Silently discard all non-Text message types
        if let Message::Text(json) = message {
            trace!(%json, "received websockets message");
            match serde_json::from_str::<Response>(&json) {
                Ok(response) => {
                    span.record("messaging.conversation_id", response.transaction_id());
                    self.commit_transaction(response).await;
                },
                Err(e) => {
                    // try to recover by getting just the transaction_id
                    // and error out of the response
                    match serde_json::from_str::<protocol::ResponseMeta>(&json) {
                        Ok(err_meta) => {
                            let err_message = err_meta.error.unwrap_or_default();
                            span.record("messaging.conversation_id", err_meta.transaction_id);
                            error!(transaction_id = %err_meta.transaction_id, error = %err_message, "Symphony error");
                            self.cancel_transaction(err_meta.transaction_id).await;
                        },
                        Err(_) => error!(json = %json, error = %e, "failed to deserialize response"),
                    }
                },
            }
        }
    }

    #[tracing::instrument(skip(self), level = "trace")]
    async fn find_transaction(&self, transaction_id: Tid) -> Option<Transaction> {
        let mut transactions_lock = self.transactions.lock().await;
        transactions_lock.list.remove(&transaction_id)
    }

    #[tracing::instrument(
        skip(self, response),
        level = "debug",
        name = "client process",
        fields(
            otel.kind = "consumer",
            messaging.system = "symphony",
            messaging.destination = "client",
            messaging.conversation_id = response.transaction_id(),
        ),
    )]
    async fn commit_transaction(&self, response: Response) {
        let transaction = self.find_transaction(response.transaction_id()).await;
        match transaction {
            Some(receiver) => {
                let send_result = receiver.send(
                    match response.error() {
                        Some(msg) => Err(ClientError::ResponseError(msg, response)),
                        None    => Ok(response),
                    }
                );
                if let Err(response) = &send_result {
                    warn!(
                        transaction_id = response.as_ref().unwrap().transaction_id(),
                        response = ?response,
                        "transaction was not awaited"
                    );
                }
            },
            None => warn!(transaction_id = response.transaction_id(), response = ?response, "received response for invalid transaction"),
        }
    }

    #[tracing::instrument(skip(self), level = "warn")]
    async fn cancel_transaction(&self, transaction_id: Tid) {
        let transaction = self.find_transaction(transaction_id).await;
        match transaction {
            Some(receiver) => {
                drop(receiver);
            },
            None => warn!(transaction_id, "tried to cancel invalid transaction"),
        }
    }

    #[tracing::instrument(skip(self), level = "debug")]
    async fn reset_transactions(&self) {
        let transactions = &mut self.transactions.lock().await;
        transactions.list.clear();
        transactions.last = Tid::MAX; // Will wrap on first transaction
        debug!("Transactions reset");
    }

    pub fn is_ready(&self) -> bool {
        self.ready.is_ready()
    }

    pub async fn wait_ready(&self) {
        self.ready.wait_ready().await
    }

    pub async fn wait_ready_timeout(&self, timeout: Duration) -> Result<()> {
        self.ready.wait_ready_timeout(timeout).await
            .map_err(|_| ClientError::CommandTimeout("(in wait_ready_timeout)".to_string()))?;
        Ok(())
    }

    #[tracing::instrument(skip(self, session_id), fields(symphony.session_id = session_id))]
    async fn login(&self, session_id: &str)
        -> Result<()>
    {
        info!("Sending login command via WebSockets");
        let receiver = self.send(Command::Login {
            session_id: session_id.to_string(),
        }).await?;
        let login_result = timeout(COMMAND_TIMEOUT, receiver).await
            .map_err(|_| ClientError::CommandTimeout("login".to_string()))? // First handle any timeout error
            .map_err(|_| ClientError::CommandFailed("login".to_string()))?; // Then, handle any error from the receiver
        let mut login_data = self.login_data.write().await;
        let login_response = login_result?;
        match login_response {
            Response::Login(data) => {
                info!(login_data = ?data, "Successful login");

                // Put the response, converted to struct LoginData
                // into self.login_data
                *login_data = Some(data.into());

                // Drop the lock before set_ready()
                drop(login_data);
                self.ready.set_ready();

                Ok(())
            },
            _ => Err(ClientError::ResponseError("Should have gotten a Response::Login".to_string(), login_response)),
        }
    }

    pub async fn get_locations(&self)
        -> Result<Vec<protocol::ResponseLoginLocations>>
    {
        self.ready.wait_ready_timeout(COMMAND_TIMEOUT).await
            .map_err(|_| ClientError::CommandTimeout("get_locations::wait_ready".to_string()))?;

        match *self.login_data.read().await {
            Some(ref data) => {
                Ok(data.locations.clone())
            },
            None => Err(ClientError::CommandFailed("get_locations - no AWL login data found".to_string())),
        }
    }

    #[tracing::instrument(skip(self, awl_id), fields(symphony.awl_id = awl_id))]
    pub async fn gateway_read(&self, awl_id: &str)
        -> Result<ReadResponse>
    {
        self.ready.wait_ready_timeout(COMMAND_TIMEOUT).await
            .map_err(|_| ClientError::CommandTimeout("gateway_read::wait_ready".to_string()))?;

        let login_data_lock = self.login_data.read().await;
        let max_zones = match *login_data_lock {
            Some(ref data) => {
                *data.zone_counts.get(awl_id)
                    .ok_or_else(|| ClientError::UnknownGateway(awl_id.to_string()))?
            },
            None => return Err(ClientError::UnknownGateway(awl_id.to_string())),
        };
        drop(login_data_lock);

        let zone_metrics_count = (max_zones as usize) * protocol::DEFAULT_ZONE_RLIST_SUFFIXES.len();
        let gateway_metrics_count = protocol::DEFAULT_READ_RLIST.len();
        let mut gateway_metrics = Vec::with_capacity(gateway_metrics_count + zone_metrics_count);

        for metric in protocol::DEFAULT_READ_RLIST.iter() {
            gateway_metrics.push(metric.to_string());
        }

        for zone_id in 1..=max_zones {
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
        let read_result = timeout(COMMAND_TIMEOUT, receiver).await
            .map_err(|_| ClientError::CommandTimeout("gateway_read".to_string()))?
            .map_err(|_| ClientError::CommandFailed("gateway_read".to_string()))?;
        let read_response = read_result?;

        match read_response {
            Response::Read(r) => Ok(r),
            _ => Err(ClientError::ResponseError("Should have gotten a Response::Login".to_string(), read_response),)
        }
    }

    #[tracing::instrument(
        skip(self, command),
        level = "info",
        name = "symphony send",
        fields(
            otel.kind = "producer",
            messaging.system = "symphony",
            messaging.destination = "symphony",
            messaging.conversation_id,
        ),
    )]
    async fn send(&self, command: Command)
        -> Result<oneshot::Receiver<Result<Response>>>
    {
        let span = Span::current();

        let request = protocol::Request {
            transaction_id: self.next_transaction_id().await?,
            command,
            source: protocol::COMMAND_SOURCE.to_string(),
        };

        span.record("messaging.conversation_id", request.transaction_id);

        let (sender, receiver) = oneshot::channel();
        let socket_lock = self.socket.lock().await;
        let socket = socket_lock.as_ref()
            .ok_or_else(|| ClientError::CommandFailed("Socket disconnected".to_string()))?;
        let json = serde_json::to_string(&request).unwrap();
        trace!(%json, "sent websockets message");
        socket.send(json)
            .map_err(ClientError::SendError)?;

        let mut transactions = self.transactions.lock().await;
        transactions.list.insert(request.transaction_id, sender);
        trace!("Transaction {} inserted", request.transaction_id);

        Ok(receiver)
    }

    #[tracing::instrument(skip(self))]
    pub async fn logout(&self) -> Result<()>
    {
        // Wait for ready in the edge case that the
        // caller calls connect() and logout() in really
        // quick succession
        self.ready.wait_ready_timeout(COMMAND_TIMEOUT).await
            .map_err(|_| ClientError::CommandTimeout("logout".to_string()))?;

        // Any logout errors are going to come from the connect() function,
        // so the instance owner must await connect() or the join handle
        // it was spawned with to receive any errors
        self.shutdown().await?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn shutdown(&self) -> Result<()>
    {
        self.shutdown.set_ready();
        self.shutdown.wait_unready_timeout(COMMAND_TIMEOUT).await
            .map_err(|_| ClientError::CommandTimeout("shutdown".to_string()))?;
        Ok(())
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("Command failed: {0}")]
    CommandFailed(String),

    #[error("Command timed out: {0}")]
    CommandTimeout(String),

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

    #[error("WebSockets connection closed unexpectedly")]
    ConnectionClosed,

    #[error("Expected closure of client session, this error should never be unhandled")]
    ExpectedTermination,
}
