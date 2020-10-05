use serde::{Serialize, Deserialize};
use serde_json::{self, Value};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing;
use tracing_futures;
use tracing::{trace, warn};

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

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
    #[serde(rename = "err", default)]
    error: Option<String>,
    #[serde(flatten)]
    data: HashMap<String, Value>,
}

#[derive(Debug)]
struct TransactionList {
    last: u8, // will wrap to 0 on first transaction
    list: HashMap<u8, oneshot::Sender<Result<Response>>>,
}

#[derive(Debug)]
pub struct Client {
    session: SessionManager,
    socket: Mutex<Option<mpsc::UnboundedSender<String>>>,
    transactions: Arc<Mutex<TransactionList>>,
}

impl Client {
    pub fn new(username: String, password: String)
        -> Client
    {
        Client {
            session: SessionManager::new(&username, &password),
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
            async move {
                while let Some(message) = rx.recv().await {
                    // TODO: implement receive logic
                    trace!(%message);
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
    pub async fn login(&self)
        -> Result<()>
    {
        let session_id = self.session.get_token().await?;
        let _receiver = self.send(Command::Login { // TODO: do something with receiver
            session_id: session_id,
        }).await;
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
