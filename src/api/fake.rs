//! A fake Screeps server for the tests of the client: it keeps each
//! request and answers it, and joins its worker when it leaves scope.

use std::collections::BTreeMap;
use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::config::ServerUrl;

/// A request that a fake server received.
#[derive(Debug, Clone)]
pub(super) struct Received {
    /// The method.
    pub(super) method: String,
    /// The path and the query.
    pub(super) target: String,
    /// The headers, by lowercase name.
    pub(super) headers: BTreeMap<String, String>,
    /// The body.
    pub(super) body: Vec<u8>,
}

/// The answer of a fake server.
pub(super) struct Answer {
    /// The status.
    pub(super) status: u16,
    /// The headers besides the type and the length of the body.
    pub(super) headers: Vec<(&'static str, String)>,
    /// The body.
    pub(super) body: String,
}

impl Answer {
    /// A JSON answer with status 200.
    pub(super) fn json(body: &Value) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.to_string(),
        }
    }
}

/// A fake server on 127.0.0.1 that keeps each request and answers it,
/// one request per connection.
pub(super) struct Fake {
    /// Its URL, with the path of the server.
    pub(super) url: String,
    /// The requests, in order.
    received: Arc<Mutex<Vec<Received>>>,
    /// The listener address used to wake it during shutdown.
    address: SocketAddr,
    /// Signals the worker to stop accepting requests.
    stopped: Arc<AtomicBool>,
    /// Joined when this server leaves scope.
    worker: Option<thread::JoinHandle<()>>,
}

impl Fake {
    /// A fake server under `path` (`/season/`) that answers with
    /// `answer`.
    pub(super) fn start(path: &str, answer: impl Fn(&Received) -> Answer + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("an address");
        let url = format!("http://{address}{path}");
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        let stopped = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stopped);
        let worker = thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("a read timeout");
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .expect("a write timeout");
                if let Some(request) = read_request(&stream) {
                    let reply = answer(&request);
                    log.lock().expect("the log").push(request);
                    // A client that went away is the client's failure.
                    let _ = write_answer(&stream, &reply);
                }
            }
        });
        Self {
            url,
            received,
            address,
            stopped,
            worker: Some(worker),
        }
    }

    /// Its URL as a server URL.
    pub(super) fn server(&self) -> ServerUrl {
        ServerUrl::try_from(self.url.clone()).expect("a URL")
    }

    /// The requests so far.
    pub(super) fn received(&self) -> Vec<Received> {
        self.received.lock().expect("the log").clone()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("the server worker finishes");
        }
    }
}

/// The request on `stream`, or none when it ends first.
fn read_request(stream: &TcpStream) -> Option<Received> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut start = line.split_whitespace();
    let method = start.next()?.to_owned();
    let target = start.next()?.to_owned();
    let mut headers = BTreeMap::new();
    loop {
        line.clear();
        reader.read_line(&mut line).ok()?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    let length = headers
        .get("content-length")
        .and_then(|length| length.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Received {
        method,
        target,
        headers,
        body,
    })
}

/// Writes `answer` to `stream`, and closes the connection.
fn write_answer(mut stream: &TcpStream, answer: &Answer) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} Fake\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
        answer.status,
        answer.body.len()
    );
    for (name, value) in &answer.headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(answer.body.as_bytes())
}
