mod support;

use support::*;
use support::server::Server;

use tracing_subscriber;

use waterfurnace_symphony::{
    Session,
    SessionResult,
    state,
    state::LoggedIn,
};

async fn establish_login_session(server: &Server)
    -> SessionResult<Session<state::Login>>
{
    let url = format!("http://localhost:{}/account/login", server.addr().port());
    let config_url = format!("http://localhost:{}/ws_location.json", server.addr().port());

    let session = Session::new(&url, &config_url);
    session.login("test_user", "bad7a55").await
}

async fn establish_connected_session(server: &Server)
    -> SessionResult<Session<state::Connected>>
{
    let login_session = establish_login_session(server).await.unwrap();
    login_session.connect().await
}

#[tokio::test]
async fn log_in() {
    let _ = tracing_subscriber::fmt::try_init();

    let server = server::http();

    let login_result = establish_login_session(&server).await;

    match login_result {
        Ok(login_session) => {
            assert!(login_session.get_token() == "0ddc0ffee0ddc0ffee0ddc0ffee");
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

    let server = server::http();

    match establish_connected_session(&server).await {
        Ok(_) => { }, // Success 
        Err(e) => {
            println!("{}", e);
            panic!("session.connect() returned error");
        }
    }
}

#[tokio::test]
async fn send() {
    let _ = tracing_subscriber::fmt::try_init();

    let server = server::http();
 
    let session = establish_connected_session(&server).await.expect("session.connect() failed");

    session.send_text("Hello").await.expect("session.send_text() failed");
    let reply = session.next().await.expect("No reply, WebSocket hung up")
        .expect("WebSocket error");
    assert_eq!(reply.into_text().expect("Wasn't a Text frame"), "Hello".to_string());
}