use thiserror::Error;

use http::{HeaderMap, HeaderValue, header::{COOKIE}};
use reqwest;

use std::time::Duration;

const LOGIN_URI: &str = "https://symphony.mywaterfurnace.com/account/login";

pub struct Session<S: SessionState> {
    client: reqwest::Client,
    credentials: Option<Login>,
    marker: std::marker::PhantomData<S>,
}

impl Session<Start> {
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
            client: {reqwest::Client::builder()
                .cookie_store(true)
                .redirect(redirect_policy)
                .build().unwrap()
            },
            credentials: None,
            marker: std::marker::PhantomData,
        }
    }

    pub async fn login(self, username: &str, password: &str)
        -> Result<Session<Login>>
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
            Err(e) if e.is_status() => {
                let status = e.status().unwrap();
                Err(SessionError::LoginFailed {
                    status: status,
                    reason: status.canonical_reason().unwrap_or("Unknown").to_string(),
                })
            },
            Err(e) => {
                if let Some(url) = e.url() {
                    let host = url.host_str().unwrap_or(url.as_str());
                    Err(SessionError::ConnectionFailed(host.to_string()))
                } else {
                    Err(SessionError::ConnectionFailed("Unknown".to_string()))
                }
            },
            Ok(response) => {
                let session_cookie = response.cookies().find(|c| c.name() == "sessionid");
                match session_cookie {
                    None => Err(SessionError::LoginFailed {
                        status: response.status(),
                        reason: "Response did not contain `sessionid` cookie.".to_string(),
                    }),
                    Some(session_cookie) => Ok(Session {
                        client: self.client,
                        credentials: Some(Login {
                            username: username.to_string(),
                            password: password.to_string(),
                            session_id: session_cookie.value().to_string(),
                        }),
                        marker: std::marker::PhantomData,
                    })
                }
            }
        }
    }
}

impl Session<Login> {
    pub async fn logout(self)
        -> Result<Session<Start>>
    {
        let mut logout_uri = reqwest::Url::parse(LOGIN_URI).unwrap();
        logout_uri.set_query(Some("op=logout"));
        let logout_result = self.client.get(logout_uri)
            .timeout(Duration::from_secs(2))
            .send().await;
        match logout_result.and_then(|r| r.error_for_status()) {
            Err(e) if e.is_status() => {
                let status = e.status().unwrap();
                Err(SessionError::LogoutFailed {
                    status: status,
                    reason: status.canonical_reason().unwrap_or("Unknown").to_string(),
                })
            },
            Err(e) => {
                if let Some(url) = e.url() {
                    let host = url.host_str().unwrap_or(url.as_str());
                    Err(SessionError::ConnectionFailed(host.to_string()))
                } else {
                    Err(SessionError::ConnectionFailed("Unknown".to_string()))
                }
            },
            Ok(_) => {
                Ok(Session::<Start>::new())
            }
        }
    }
}

// State type options
pub struct Start; // Initial state
pub struct Login { // HTTP login completed; websocket not connected
    username: String,
    password: String,
    session_id: String,
}
pub struct Connected {} // "Running" state: logged in and websocket connected
pub struct Disconnected {} // A possible error state

pub trait SessionState {}
impl SessionState for Start {}
impl SessionState for Login {}
impl SessionState for Connected {}
impl SessionState for Disconnected {}

#[derive(Error, Debug)]
pub enum SessionError {
    #[error("Unable to connect to AWL (host {0})")]
    ConnectionFailed(String),
    
    #[error("Login failed: [{status}] {reason}")]
    LoginFailed {
        status: reqwest::StatusCode,
        reason: String
    },
    
    #[error("Logout failed: [{status}] {reason}")]
    LogoutFailed {
        status: reqwest::StatusCode,
        reason: String
    },
}

type Result<T> = std::result::Result<T, SessionError>;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
