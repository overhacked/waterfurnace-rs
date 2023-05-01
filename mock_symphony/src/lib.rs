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
use tracing::{debug, error};
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

#[derive(Debug)]
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

    pub fn shutdown(mut self) {
        self._shutdown();
    }

    fn _shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            debug!("Server{{}} dropped, shutting down warp thread");
            let _ = tx.send(());
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self._shutdown();

        if !::std::thread::panicking() {
            self.panic_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("test server should not panic");
        }
    }
}

#[derive(Debug)]
pub struct FailProbability(u8);

impl FailProbability {
    pub fn never() -> Self
    {
        FailProbability(0)
    }

    pub fn always() -> Self {
        FailProbability(100)
    }

    pub fn from_percent(input: u8) -> Self {
        if input > 100 {
            panic!("FailProbablity must be in the range 0-100, inclusive (got {})", input);
        }
        FailProbability(input)
    }
}

#[derive(Debug)]
pub struct Chaos {
    pub failure: FailProbability,
    pub delay_min: Duration,
    pub delay_max: Duration,
}

impl Chaos {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn none() -> Self {
        Self::default()
    }

    pub fn always_fail() -> Self {
        Chaos {
            failure: FailProbability::always(),
            ..Default::default()
        }
    }

    pub fn fail_at(mut self, probability: FailProbability) -> Self {
        self.failure = probability;
        self
    }

    pub fn delay_between_ms<T>(mut self, range: impl std::ops::RangeBounds<u64>) -> Self {
        use std::ops::Bound;
        enum End {
            Start,
            End,
        }

        let abs_bound = |end: End, bound: Bound<&u64>| -> Option<u64> {
            let op = match end {
                End::Start => u64::checked_add,
                End::End => u64::checked_sub,
            };
            match bound {
                Bound::Included(n) => Some(*n),
                Bound::Excluded(n) => Some(op(*n, 1).expect("delay_between_ms overflow")),
                Bound::Unbounded => None,
            }
        };
        let start_abs = abs_bound(End::Start, range.start_bound()).unwrap_or(0);
        let end_abs = abs_bound(End::End, range.start_bound())
            .expect("delay_between_ms does not support unbounded range end, only start");
        self.delay_min = Duration::from_millis(start_abs);
        self.delay_max = Duration::from_millis(end_abs);
        self
    }

    fn failure_probability(&self) -> f64 {
        self.failure.0 as f64 / 100.0
    }

    fn delay_mean(&self) -> Duration {
        (self.delay_max + self.delay_min) / 2
    }
}

impl Default for Chaos {
    fn default() -> Self {
        Chaos {
            failure: FailProbability(0),
            delay_min: Duration::default(),
            delay_max: Duration::default(),
        }
    }
}

#[derive(Debug)]
struct ChaosError;
impl warp::reject::Reject for ChaosError {}

pub fn http() -> Server {
    http_chaos(Chaos::default(), None)
}

pub fn http_chaos(chaos: Chaos, port: Option<u16>) -> Server
{
    //Spawn new runtime in thread to prevent reactor execution context conflict
    thread::spawn(move || {
        let rt = runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (addr, srv) = rt.block_on(async move {
            let filters = simulate_chaos(&chaos)
                .and(login_route()
                .or(logout_route())
                .or(wsconfig_route())
                .or(ws_route(&chaos)))
                .recover(handle_rejection);
            let localhost = net::IpAddr::V4(net::Ipv4Addr::new(127, 0, 0, 1));
            let port = port.unwrap_or(0);
            warp::serve(filters)
                .bind_with_graceful_shutdown(net::SocketAddr::new(localhost, port), async {
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
    let delay_fn = LogNormal::new(0.0, 0.5).unwrap();

    warp::any()
        .map(move || (failure_fn, delay_fn, mean_delay_ms,))
        .and_then(|(failure_fn, delay_fn, mean_delay_ms): (Bernoulli, LogNormal<f64>, f64)| async move {
            if failure_fn.sample(&mut rand::thread_rng()) {
                debug!("Introducing request failure");
                return Err(warp::reject::custom(ChaosError));
            } else {
                let delay_ms = delay_fn.sample(&mut rand::thread_rng()) * mean_delay_ms;
                let delay = Duration::from_micros((delay_ms * 1000.0) as u64);
                debug!("Introducing request delay = {:?}", delay);
                tokio::time::sleep(delay).await;
            }
            Ok(())
        })
        .untuple_one()
        .boxed()
}

fn login_route() -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    // Reminder: update const LOGIN_PATH
    warp::path!("account" / "login")
        .and(warp::post())
        .and(warp::cookie("legal-acknowledge"))
        .and(warp::body::form())
        .map(|legal_acknowledge: String, form_body: HashMap<String, String>| {
            debug!("warp::path!(account/login)");
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
            warp::reply::with_header(
                warp::reply(),
                http::header::SET_COOKIE,
                http::header::HeaderValue::from_str(&format!("sessionid=; Domain={}; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT", a.unwrap().host())).unwrap()
            )
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
    let delay_fn = LogNormal::new(0.0, 0.5).unwrap();

    let ws_handler = move |ws: warp::filters::ws::WebSocket| { handle_websocket_request(ws, failure_fn, delay_fn, mean_delay_ms) };
    warp::path("ws")
        .and(warp::filters::ws::ws())
        .map(move |ws: warp::filters::ws::Ws| ws.on_upgrade(ws_handler))
}

async fn handle_websocket_request(ws: warp::filters::ws::WebSocket, failure_fn: Bernoulli, delay_fn: LogNormal<f64>, mean_delay_ms: f64) {
    let (mut tx, mut rx) = ws.split();
    let mut message_handler = ws::MessageHandler::new(failure_fn, delay_fn, mean_delay_ms);
    loop {
        match rx.next().await {
            Some(Ok(message)) if message.is_text() => {
                tx.send(message_handler.handle_websocket_message(message).await).await
                    .expect("WebSocket send failure");
            },
            Some(Ok(message)) if message.is_binary() => panic!("Should not receive any binary frames"),
            Some(Ok(_)) => {}, // Ignore all other message types (Ping, Pong, Close)
            Some(Err(e)) => panic!("{}", e),
            None => break, // No more messages, client closed connection
        }
    }
}

async fn handle_rejection(err: warp::Rejection)
    -> Result<impl warp::Reply, std::convert::Infallible>
{
    use warp::http::StatusCode;
    let code =
        if err.is_not_found()
        { StatusCode::NOT_FOUND }
        else if err.find::<warp::reject::MethodNotAllowed>().is_some()
        { StatusCode::METHOD_NOT_ALLOWED }
        else if err.find::<ChaosError>().is_some()
        { StatusCode::SERVICE_UNAVAILABLE }
        else {
        error!("unhandled custom rejection, returning 500 response: {:?}", err);
        StatusCode::INTERNAL_SERVER_ERROR
        };

    Ok(warp::reply::with_status(warp::reply(), code))
}
