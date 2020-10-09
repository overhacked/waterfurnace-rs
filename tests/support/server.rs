use std::collections::HashMap;
use std::net;
use std::str::FromStr;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use futures::sink::SinkExt;
use futures::stream::StreamExt;
use tokio::{
    runtime,
    sync::oneshot,
};
use warp::{
    self,
    Filter,
    http::{self, Uri},
};

pub use http::Response;

const FAKE_SESSION_ID: &str = "0ddc0ffee0ddc0ffee0ddc0ffee";

pub struct Server {
    addr: net::SocketAddr,
    panic_rx: mpsc::Receiver<()>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl Server {
    pub fn addr(&self) -> net::SocketAddr {
        self.addr
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        if !::std::thread::panicking() {
            self.panic_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("test server should not panic");
        }
    }
}

pub fn http() -> Server
{
    //Spawn new runtime in thread to prevent reactor execution context conflict
    thread::spawn(move || {
        let mut rt = runtime::Builder::new()
            .basic_scheduler()
            .enable_all()
            .build()
            .expect("new rt");

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (addr, srv) = rt.block_on(async move {
            let filters = login_route().or(logout_route()).or(wsconfig_route()).or(ws_route());
            let localhost = net::IpAddr::V4(net::Ipv4Addr::new(127, 0, 0, 1));
            warp::serve(filters)
                .bind_with_graceful_shutdown(net::SocketAddr::new(localhost, 0), async {
                    shutdown_rx.await.ok();
                })
        });

        let (panic_tx, panic_rx) = mpsc::channel();
        let tname = format!(
            "test({})-support-server",
            thread::current().name().unwrap_or("<unknown>")
        );
        thread::Builder::new()
            .name(tname)
            .spawn(move || {
                rt.block_on(srv);
                let _ = panic_tx.send(());
            })
            .expect("thread spawn");

        Server {
            addr,
            panic_rx,
            shutdown_tx: Some(shutdown_tx),
        }
    })
    .join()
    .unwrap()
}

fn login_route() -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    warp::path!("account" / "login")
        .and(warp::post())
        .and(warp::cookie("legal-acknowledge"))
        .and(warp::body::form())
        .map(|legal_acknowledge: String, form_body: HashMap<String, String>| {
            assert!(legal_acknowledge == "yes", "Cookie `legal-acknowledge=yes` was not sent");
            assert!(form_body.get("op").unwrap() == "login");
            assert!(form_body.get("redirect").unwrap() == "/");
            assert!(form_body.contains_key("emailaddress"));
            assert!(form_body.contains_key("password"));
            form_body.get("redirect").unwrap().clone()
        })
        .and(warp::host::optional())
        .map(|redirect_to: String, a: Option<warp::host::Authority>| {
            let uri: Uri = Uri::from_str(&redirect_to).unwrap();
            let reply = warp::redirect(uri);
            let reply = warp::reply::with_header(
                reply,
                http::header::SET_COOKIE,
                http::header::HeaderValue::from_str(&format!("sessionid={}; Domain={}; Path=/", FAKE_SESSION_ID, a.unwrap().host())).unwrap()
            );
            warp::reply::with_status(reply, warp::http::StatusCode::FOUND)
        })
}

fn logout_route() -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    warp::path!("account" / "login")
        .and(warp::get())
        .and(warp::host::optional())
        .and(warp::query::raw())
        .map(|a: Option<warp::host::Authority>, qs: String| {
            assert!(qs == "op=logout");
            let reply = warp::reply();
            let reply = warp::reply::with_header(
                reply,
                http::header::SET_COOKIE,
                http::header::HeaderValue::from_str(&format!("sessionid=; Domain={}; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT", a.unwrap().host())).unwrap()
            );
            reply
        })
}

fn wsconfig_route() -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    warp::path("ws_location.json")
        .and(warp::get())
        .and(warp::host::optional())
        .map(|a: Option<warp::host::Authority>| {
            warp::reply::json(&format!("ws://{}/ws", a.unwrap()))
        })
}

fn ws_route() -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    warp::path("ws")
        .and(warp::filters::ws::ws())
        .map(|ws: warp::filters::ws::Ws| ws.on_upgrade(handle_websocket_request))
}

async fn handle_websocket_request(ws: warp::filters::ws::WebSocket) {
    let (mut tx, mut rx) = ws.split();
    loop {
        match rx.next().await {
            Some(Ok(message)) if message.is_text() => {
                tx.send(message).await
                    .expect("WebSocket send failure");
            },
            Some(Ok(message)) if message.is_binary() => panic!("Should not receive any binary frames"),
            Some(Ok(_)) => {}, // Ignore all other message types (Ping, Pong, Close)
            Some(Err(e)) => panic!("WebSocket error: {}", e),
            None => break, // No more messages, client closed connection
        }
    }
}
