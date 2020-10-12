mod ws;

use std::collections::HashMap;
use std::net;
use std::str::FromStr;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use futures::sink::SinkExt;
use futures::stream::StreamExt;
use rand_distr::{Bernoulli, LogNormal, Distribution};
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

pub const FAKE_SESSION_ID: &str = "0ddc0ffee0ddc0ffee0ddc0ffee";
pub const HELLO_SEND: &str = "Hello";
pub const HELLO_REPLY: &str = "Hola";
pub const FAKE_GWID: &str = "0DDC0FFEE";
pub const NUM_ZONES: u8 = 3;

const LOGIN_PATH: &str = "account/login";
const CONFIG_PATH: &str = "ws_location.json";

pub struct Server {
    addr: net::SocketAddr,
    panic_rx: mpsc::Receiver<()>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl Server {
    pub fn addr(&self) -> net::SocketAddr {
        self.addr
    }

    pub fn get_login_uri(&self) -> String {
        format!("http://localhost:{}/{}", self.addr().port(), LOGIN_PATH)
    }

    pub fn get_config_uri(&self) -> String {
        format!("http://localhost:{}/{}", self.addr().port(), CONFIG_PATH)
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

struct FailProbability(u8);

impl FailProbability {
    pub fn new(input: u8) -> Self {
        if input > 100 {
            panic!("FailProbablity must be in the range 0-100, inclusive (got {})", input);
        }
        FailProbability(input)
    }
}

pub struct Chaos {
    failure_pct: FailProbability,
    delay_min: Duration,
    delay_max: Duration,
}

impl Chaos {
    fn failure_probability(&self) -> f64 {
        self.failure_pct.0 as f64 / 100.0 
    }

    fn delay_mean(&self) -> Duration {
        (self.delay_max + self.delay_min) / 2 
    }
}

impl Default for Chaos {
    fn default() -> Self {
        Chaos {
            failure_pct: FailProbability(0),
            delay_min: Duration::default(),
            delay_max: Duration::default(),
        }
    }
}

#[derive(Debug)]
struct ChaosError;
impl warp::reject::Reject for ChaosError {}

/*
 * TODO: Chaos
 * - % of requests fail (500 Server Error)
 * - Delay with deviation
 */
pub fn http() -> Server {
    http_chaos(Chaos::default())
}

pub fn http_chaos(chaos: Chaos) -> Server
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
            let filters = simulate_chaos(&chaos)
                .and(
                    login_route()
                    .or(logout_route())
                    .or(wsconfig_route())
                    .or(ws_route(&chaos))
                );
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

fn simulate_chaos(chaos: &Chaos) -> impl Filter<Extract = (), Error = warp::Rejection> + Clone {
    let failure_fn = Bernoulli::new(chaos.failure_probability()).unwrap();
    let mean_delay_ms = (chaos.delay_mean().as_micros() / 1000) as f64;
    let delay_fn = LogNormal::new(mean_delay_ms, 3.7).unwrap();

    warp::any()
        .map(move || (failure_fn, delay_fn,))
        .and_then(|(failure_fn, delay_fn): (Bernoulli, LogNormal<f64>)| async move {
            if failure_fn.sample(&mut rand::thread_rng()) {
                return Err(warp::reject::custom(ChaosError));
            } else {
                let delay_ms = delay_fn.sample(&mut rand::thread_rng());
                let delay = Duration::from_micros((delay_ms * 1000.0) as u64);
                tokio::time::delay_for(delay).await;
            }
            Ok(())
        })
        .untuple_one()
}

fn login_route() -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    // Reminder: update const LOGIN_PATH
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
    // Reminder: update const LOGIN_PATH
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
    warp::path(CONFIG_PATH)
        .and(warp::get())
        .and(warp::host::optional())
        .map(|a: Option<warp::host::Authority>| {
            warp::reply::json(&format!("ws://{}/ws", a.unwrap()))
        })
}

fn ws_route(chaos: &Chaos) -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    let failure_fn = Bernoulli::new(chaos.failure_probability()).unwrap();
    let mean_delay_ms = (chaos.delay_mean().as_micros() / 1000) as f64;
    let delay_fn = LogNormal::new(mean_delay_ms, 3.7).unwrap();

    let ws_handler = move |ws: warp::filters::ws::WebSocket| { handle_websocket_request(ws, failure_fn, delay_fn) };
    warp::path("ws")
        .and(warp::filters::ws::ws())
        .map(move |ws: warp::filters::ws::Ws| ws.on_upgrade(ws_handler))
}

async fn handle_websocket_request(ws: warp::filters::ws::WebSocket, failure_fn: Bernoulli, delay_fn: LogNormal<f64>) {
    let (mut tx, mut rx) = ws.split();
    let mut message_handler = ws::MessageHandler::new(failure_fn, delay_fn);
    loop {
        match rx.next().await {
            Some(Ok(message)) if message.is_text() => {
                tx.send(message_handler.handle_websocket_message(message).await).await
                    .expect("WebSocket send failure");
            },
            Some(Ok(message)) if message.is_binary() => panic!("Should not receive any binary frames"),
            Some(Ok(_)) => {}, // Ignore all other message types (Ping, Pong, Close)
            Some(Err(e)) => panic!("WebSocket error: {}", e),
            None => break, // No more messages, client closed connection
        }
    }
}
