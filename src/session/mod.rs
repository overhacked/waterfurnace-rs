use thiserror::Error;

use futures::sink::SinkExt;
use futures::stream::StreamExt;
use http::{HeaderMap, HeaderValue, header::{COOKIE}};
use regex::Regex;
use reqwest;
use tokio_tungstenite::{
    self,
    tungstenite::{
        Error as TungsteniteError,
        protocol::Message as TungsteniteMessage,
    },
};
pub use tokio_tungstenite::tungstenite::protocol::Message;
use tokio::sync::{
    Mutex,
    TryLockError,
};
use tracing::{self, trace, debug, info,};
use tracing_futures;
use url::Url;

use std::sync::Arc;
use std::time::Duration;

type WebSocketStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct Session<S: state::SessionState> {
    state: S,
    client: reqwest::Client,
    login_uri: String,
    config_uri: String,
}

impl<S: state::SessionState> Session<S>
{
    fn get_state(&self) -> &S {
        &self.state
    }
}

impl Session<state::Start> {
    #[tracing::instrument]
    pub fn new(uri: &str, config_uri: &str) -> Self {
        let redirect_policy =
            reqwest::redirect::Policy::custom(|attempt| {
                match attempt.previous().last() {
                    Some(previous) if previous.path().starts_with("/account/login")
                        => attempt.stop(),
                    Some(_) | None
                        => attempt.follow(),
                }
            });
        Session {
            state: state::Start{},
            client: {reqwest::Client::builder()
                .cookie_store(true)
                .redirect(redirect_policy)
                .timeout(HTTP_TIMEOUT)
                .build().unwrap()
            },
            login_uri: uri.to_string(),
            config_uri: config_uri.to_string(),
        }
    }

    #[tracing::instrument(skip(self), fields(password="********"))]
    pub async fn login(self, username: &str, password: &str)
        -> Result<Session<state::Login>>
    {
        let mut login_headers = HeaderMap::new();
        login_headers.insert(COOKIE, HeaderValue::from_str("legal-acknowledge=yes").unwrap());

        info!("Logging into Symphony...");
        let login_result = self.client.post(&self.login_uri)
            .headers(login_headers)
            .form(&[
                ("op", "login"),
                ("redirect", "/"),
                ("emailaddress", username),
                ("password", password),
            ])
            .send().await;

        let login_response = login_result.and_then(|r| r.error_for_status())?;
        let session_cookie = login_response.cookies()
            .find(|c| c.name() == "sessionid")
            .ok_or(SessionError::InvalidCredentials)?;
        debug!("Successfully logged into Symphony");
        Ok(Session {
            state: state::Login {
                username: username.to_string(),
                password: password.to_string(),
                session_id: session_cookie.value().to_string(),
            },
            client: self.client,
            login_uri: self.login_uri,
            config_uri: self.config_uri,
        })
    }
}

impl Session<state::Login> {
    #[tracing::instrument(skip(self))]
    pub async fn logout(self)
        -> Result<Session<state::Start>>
    {
        let mut logout_uri = reqwest::Url::parse(&self.login_uri).unwrap();
        logout_uri.set_query(Some("op=logout"));
        info!("Logging out of Symphony...");
        self.client.get(logout_uri)
            .send().await
            .and_then(|r| r.error_for_status())?;
        debug!("Successfully logged out of Symphony");
        Ok(Session {
            state: state::Start {},
            client: self.client,
            login_uri: self.login_uri,
            config_uri: self.config_uri,
        })
    }

    #[tracing::instrument(skip(self))]
    pub async fn connect(self)
        -> Result<Session<state::Connected>>
    {
        let ws_url = self.get_websockets_uri().await?;
        info!("Connecting to Symphony WebSockets at {}", &ws_url);
        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url).await?;
        debug!("Connected!");
        Ok(Session {
            state: state::Connected {
                credentials: self.state,
                websocket: Arc::new(Mutex::new(ws_stream)),
            },
            client: self.client,
            login_uri: self.login_uri,
            config_uri: self.config_uri,
        })
    }

    #[tracing::instrument(skip(self))]
    async fn get_websockets_uri(&self) -> Result<Url> {
        debug!("Fetching Symphony WebSockets URI from {}", &self.config_uri);
        let wssuri_result = self.client
            .get(&self.config_uri)
            .send().await;
        let wssuri_response = wssuri_result.and_then(|r| r.error_for_status())?;
        let text = wssuri_response.text().await?;
        let re = Regex::new(r#"wss?://[^"']+"#).unwrap();
        match re.find(&text) {
            None => Err(
                SessionError::UnexpectedValue(
                    format!("Could not find wss://* URI in {}", self.config_uri).to_string()
                )
            ),
            Some(m) => {
                debug!("Got WebSockets URI: {}", &m.as_str());
                Url::parse(m.as_str())
                    .map_err(|e| SessionError::UnexpectedValue(
                        format!("Could not parse URI ({:?}): {}", e, m.as_str()).to_string()
                    ))
            },
        }
    }
}

impl state::LoggedIn for Session<state::Login> {
    fn get_token(&self) -> &str {
        &self.state.session_id
    }
}

impl Session<state::Connected> {
    #[tracing::instrument(skip(self))]
    pub async fn next(&self)
        -> Option<std::result::Result<TungsteniteMessage, TungsteniteError>>
    {
        trace!("Awaiting next WebSockets message");
        let websocket_c = self.state.websocket.clone();
        let mut websocket_lock = websocket_c.lock().await;
        websocket_lock.next().await
    }

    #[tracing::instrument(skip(self))]
    pub async fn send(&self, message: TungsteniteMessage)
        -> Result<()>
    {
        trace!("Sending WebSockets message: {:?}", message);
        let websocket_c = self.state.websocket.clone();
        let mut websocket_lock = websocket_c.lock().await;
        Ok(websocket_lock.send(message).await?)
    }

    pub async fn send_text<S>(&self, message: S)
        -> Result<()>
        where S: Into<String>,
    {
        self.send(Message::text(message)).await
    }

    #[tracing::instrument]
    pub async fn close(self)
        -> Result<Session<state::Login>>
    {
        info!("Closing Symphony WebSockets connection...");
        let mut websocket_lock = self.state.websocket.try_lock()?;
        websocket_lock.close(None).await?;

        debug!("Successfully closed Symphony WebSockets connection.");
        Ok(Session {
            state: self.state.credentials,
            client: self.client,
            login_uri: self.login_uri,
            config_uri: self.config_uri,
        })
    }

    #[tracing::instrument(skip(self))]
    pub async fn logout(self)
        -> Result<Session<state::Disconnected>>
    {
        debug!("Logging out connected Symphony WebSockets connection");
        let credentials = self.state.credentials.clone();
        let login_session = self.close().await?;
        let start_session = login_session.logout().await?;

        Ok(Session {
            state: state::Disconnected {
                credentials: credentials,
            },
            client: start_session.client,
            login_uri: start_session.login_uri,
            config_uri: start_session.config_uri,
        })
    }
}

impl state::LoggedIn for Session<state::Connected> {
    fn get_token(&self) -> &str {
        &self.state.credentials.session_id
    }
}

impl Session<state::Disconnected> {
    pub async fn login(self)
        -> Result<Session<state::Login>>
    {
        debug!("Logging back into Symphony from disconnected session.");
        Session::new(&self.login_uri, &self.config_uri).login(
            &self.state.credentials.username,
            &self.state.credentials.password
        ).await
    }
}

// State type options
pub mod state {
    #[derive(Debug)]
    pub struct Start; // Initial state
    #[derive(Clone)]
    pub struct Login { // HTTP login completed; websocket not connected
        pub(super) username: String,
        pub(super) password: String,
        pub(super) session_id: String,
    }
    impl std::fmt::Debug for Login {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Login")
                .field("username", &self.username)
                .field("password", &"********")
                .field("session_id", &self.session_id)
                .finish()
        }
    }
    pub struct Connected { // "Running" state: logged in and websocket connected
        pub(super) credentials: Login,
        pub(super) websocket: super::Arc<super::Mutex<super::WebSocketStream>>,
    }
    impl std::fmt::Debug for Connected {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Connected")
                .field("credentials", &self.credentials)
                .field("websocket", &"SOCKET")
                .finish()
        }
    }
    #[derive(Debug)]
    pub struct Disconnected { // A possible error state
        pub(super) credentials: Login,
    }

    pub trait SessionState {}
    impl SessionState for Start {}
    impl SessionState for Login {}
    impl SessionState for Connected {}
    impl SessionState for Disconnected {}

    pub trait LoggedIn {
        fn get_token(&self) -> &str;
    }
}


#[derive(Error, Debug)]
pub enum SessionError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    WebSockets(#[from] TungsteniteError),

    #[error("stream() already called")]
    AlreadyStreaming(#[from] TryLockError),

    #[error("Invalid credentials, could not get session ID")]
    InvalidCredentials,

    #[error("Unexpected value: {0}")]
    UnexpectedValue(String),
}

pub type Result<T> = std::result::Result<T, SessionError>;

#[cfg(test)]
mod tests;
