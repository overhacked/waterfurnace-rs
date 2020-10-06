pub mod protocol;

use serde_json;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
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
use crate::session::SessionError;

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
