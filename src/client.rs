use thiserror::Error;

use http::{HeaderMap, HeaderValue, header::{COOKIE}};
use regex::Regex;
use reqwest;
use tokio_tungstenite::{
    self,
    tungstenite::{
        Error as TungsteniteError,
    },
};
use tracing;
use tracing_futures;
use url::Url;

use std::time::Duration;

type WebSocketStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const LOGIN_URI: &str = "https://symphony.mywaterfurnace.com/account/login";
const AWLCONFIG_URI: &str = "https://symphony.mywaterfurnace.com/assets/js/awlconfig.js.php";
// Taken from setTimeout(1500000, ...) in Symphony JavaScript
const SESSION_TIMEOUT: Duration = Duration::from_millis(1500000);

#[derive(Debug)]
pub struct Session<S: state::SessionState> {
    state: S,
    client: reqwest::Client,
}

impl Session<state::Start> {
    pub fn new() -> Self {
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
                .build().unwrap()
            },
        }
    }

    #[tracing::instrument(fields(password="********"))]
    pub async fn login(self, username: &str, password: &str)
        -> Result<Session<state::Login>>
    {
        let mut login_headers = HeaderMap::new();
        login_headers.insert(COOKIE, HeaderValue::from_str("legal-acknowledge=yes").unwrap());

        let login_result = self.client.post(LOGIN_URI)
            .headers(login_headers)
            .form(&[
                ("op", "login"),
                ("redirect", "/"),
                ("emailaddress", username),
                ("password", password),
            ])
            .send().await;

        match login_result.and_then(|r| r.error_for_status()) {
            Err(e) => Err(SessionError::Http(e)),
            Ok(response) => {
                let session_cookie = response.cookies().find(|c| c.name() == "sessionid");
                match session_cookie {
                    None => Err(SessionError::InvalidCredentials("Response did not contain `sessionid` cookie.".to_string())),
                    Some(session_cookie) => Ok(Session {
                        state: state::Login {
                            username: username.to_string(),
                            password: password.to_string(),
                            session_id: session_cookie.value().to_string(),
                        },
                        client: self.client,
                    })
                }
            }
        }
    }
}

impl Session<state::Login> {
    #[tracing::instrument]
    pub async fn logout(self)
        -> Result<Session<state::Start>>
    {
        let mut logout_uri = reqwest::Url::parse(LOGIN_URI).unwrap();
        logout_uri.set_query(Some("op=logout"));
        let logout_result = self.client.get(logout_uri)
            .timeout(Duration::from_secs(2))
            .send().await;
        match logout_result.and_then(|r| r.error_for_status()) {
            Err(e) => Err(SessionError::Http(e)),
            Ok(_) => {
                Ok(Session {
                    state: state::Start {},
                    client: self.client,
                })
            }
        }
    }

    #[tracing::instrument]
    pub async fn connect(self)
        -> Result<Session<state::Connected>>
    {
        let ws_url = self.get_websockets_uri().await?;
        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url).await.map_err(|e| SessionError::WebSockets(e))?;
        Ok(Session {
            state: state::Connected {
                credentials: self.state,
                websocket: ws_stream,
            },
            client: self.client,
        })
    }

    #[tracing::instrument]
    async fn get_websockets_uri(&self) -> Result<Url> {
        let wssuri_result = self.client
            .get(AWLCONFIG_URI)
            .send().await;
        match wssuri_result.and_then(|r| r.error_for_status()) {
            Err(e) => Err(SessionError::Http(e)),
            Ok(response) => {
                let text = response.text().await?;
                let re = Regex::new(r#"wss?://[^"']+"#).unwrap();
                match re.find(&text) {
                    None => Err(SessionError::UnexpectedValue(format!("Could not find wss://* URI in {}", AWLCONFIG_URI).to_string())),
                    Some(m) => Url::parse(m.as_str())
                        .map_err(|e| SessionError::UnexpectedValue(
                            format!("Could not parse URI ({:?}): {}", e, m.as_str()).to_string()
                        )),
                }
            }
        }
    }
}

impl Session<state::Connected> {
    #[tracing::instrument]
    pub async fn close(mut self)
        -> Result<Session<state::Login>>
    {
        self.state.websocket.close(None).await?;

        Ok(Session {
            state: self.state.credentials,
            client: self.client,
        })
    }

    #[tracing::instrument]
    pub async fn logout(self)
        -> Result<Session<state::Disconnected>>
    {
        let credentials = self.state.credentials.clone();
        let login_session = self.close().await?;
        let start_session = login_session.logout().await?;

        Ok(Session {
            state: state::Disconnected {
                credentials: credentials,
            },
            client: start_session.client,
        })
    }
}

impl Session<state::Disconnected> {
    pub async fn login(self)
        -> Result<Session<state::Login>>
    {
        Session::new().login(
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
        pub(super) websocket: super::WebSocketStream,
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
}


#[derive(Error, Debug)]
pub enum SessionError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    WebSockets(#[from] TungsteniteError),

    #[error("Login failed: {0}")]
    InvalidCredentials(String),

    #[error("Unexpected value: {0}")]
    UnexpectedValue(String),
}

type Result<T> = std::result::Result<T, SessionError>;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
