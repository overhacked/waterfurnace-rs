use mock_symphony::{
    self,
    Server,
};

use tracing_subscriber;

use super::*;
use super::state::LoggedIn as _;
/*
use waterfurnace_symphony::{
    Session,
    Result,
    state,
    state::LoggedIn,
};
*/

pub async fn establish_login_session(server: &Server)
    -> Result<Session<state::Login>>
{
    let url = server.get_login_uri();
    let config_url = server.get_config_uri();

    let session = Session::new(&url, &config_url);
    session.login("test_user", "bad7a55").await
}

pub async fn establish_connected_session(server: &Server)
    -> Result<Session<state::Connected>>
{
    let login_session = establish_login_session(server).await.unwrap();
    login_session.connect().await
}

#[tokio::test]
async fn log_in() {
    let _ = tracing_subscriber::fmt::try_init();

    let server = mock_symphony::http();

    let login_result = establish_login_session(&server).await;

    match login_result {
        Ok(login_session) => {
            assert!(login_session.get_token() == mock_symphony::FAKE_SESSION_ID);
        },
        Err(e) => {
            println!("{}", e);
            panic!("session.login() returned error");
        }

    }
}

#[tokio::test]
async fn connect() {
    let _ = tracing_subscriber::fmt::try_init();

    let server = mock_symphony::http();

    match establish_connected_session(&server).await {
        Ok(session) => {
            assert!(matches!(session.get_state(), state::Connected{..}));
            let session = session.close().await.expect("session.close() failed");
            assert!(matches!(session.get_state(), state::Login{..}));
        },
        Err(e) => {
            println!("{}", e);
            panic!("session.connect() returned error");
        }
    }
}

#[tokio::test]
async fn send() {
    let _ = tracing_subscriber::fmt::try_init();

    let server = mock_symphony::http();

    let session = establish_connected_session(&server).await.expect("session.connect() failed");

    session.send_text(mock_symphony::HELLO_SEND).await.expect("session.send_text() failed");
    let reply = session.next().await.expect("No reply, WebSocket hung up")
        .expect("WebSocket error");
    assert_eq!(
        reply.into_text().expect("Wasn't a Text frame"),
        mock_symphony::HELLO_REPLY.to_string()
    );

    session.close().await.expect("session.close() failed");
}

#[tokio::test]
async fn log_out() {
    let _ = tracing_subscriber::fmt::try_init();

    let server = mock_symphony::http();

    let login_result = establish_login_session(&server).await.expect("session.login() failed");

    match login_result.logout().await {
        Ok(r) => {
            assert!(matches!(r.get_state(), state::Start{..}));
        },
        Err(e) => {
            println!("{}", e);
            panic!("session.login() returned error");
        },
    };
}
