//! Loopback TCP listener that exchanges newline-delimited messages with authenticated MCP servers.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Line sent to a connection once its token has been accepted.
const HELLO: &[u8] = b"{\"hello\":\"mcplink\",\"version\":\"0.1.01\"}\n";

/// How long background threads wait before checking whether the link is shutting down.
const TICK: Duration = Duration::from_millis(100);

/// How long a new connection has to present the token.
const AUTH_WINDOW: Duration = Duration::from_secs(5);

/// How long the game thread may block while writing a response.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest line accepted before a connection has authenticated.
const MAX_AUTH_LINE: usize = 256;

/// Longest request line accepted from an authenticated connection.
const MAX_LINE: usize = 64 * 1024 * 1024;

/// Number of random bytes in a session token.
const TOKEN_BYTES: usize = 24;

type Clients = Arc<Mutex<HashMap<u64, TcpStream>>>;

/// One request line received from an authenticated connection.
pub struct Message {
    pub client: u64,
    pub data: Vec<u8>,
}

/// A running listener together with the threads and connections it owns.
pub struct Link {
    port: u16,
    token: String,
    shutdown: Arc<AtomicBool>,
    inbox: Receiver<Message>,
    clients: Clients,
    acceptor: Option<JoinHandle<()>>,
}

impl Link {
    /// Binds a free loopback port, generates a session token and starts accepting connections.
    pub fn listen() -> io::Result<Link> {
        let token = generate_token()?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;

        let port = listener.local_addr()?.port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
        let (outbox, inbox) = mpsc::channel();

        let context = Context {
            token: token.clone(),
            shutdown: Arc::clone(&shutdown),
            clients: Arc::clone(&clients),
            outbox,
        };
        let acceptor = thread::Builder::new()
            .name("mcplink-accept".into())
            .spawn(move || accept_loop(listener, context))?;

        Ok(Link {
            port,
            token,
            shutdown,
            inbox,
            clients,
            acceptor: Some(acceptor),
        })
    }

    /// Port the listener is bound to on 127.0.0.1.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Token a connection must send as its first line.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Takes every request line received since the previous call, without blocking.
    pub fn drain(&self) -> Vec<Message> {
        self.inbox.try_iter().collect()
    }

    /// Writes one line to a connection and reports whether it was delivered; a failed write drops the connection.
    pub fn send(&self, client: u64, data: &[u8]) -> bool {
        let mut clients = lock(&self.clients);
        let Some(stream) = clients.get_mut(&client) else {
            return false;
        };

        let delivered = stream
            .write_all(data)
            .and_then(|()| stream.write_all(b"\n"))
            .is_ok();
        if !delivered {
            let _ = stream.shutdown(Shutdown::Both);
            clients.remove(&client);
        }

        delivered
    }
}

impl Drop for Link {
    /// Stops every background thread and closes every connection before the module can be unloaded.
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);

        for stream in lock(&self.clients).values() {
            let _ = stream.shutdown(Shutdown::Both);
        }

        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }

        lock(&self.clients).clear();
    }
}

/// State shared by the accept thread and every connection thread.
#[derive(Clone)]
struct Context {
    token: String,
    shutdown: Arc<AtomicBool>,
    clients: Clients,
    outbox: Sender<Message>,
}

/// Locks `mutex`, recovering the data if another thread panicked while holding it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Creates a hex session token from operating system randomness.
fn generate_token() -> io::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;

    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Compares two byte strings without stopping at the first difference.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// Reports whether a read failed only because no data arrived within one tick.
fn is_idle(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
    )
}

/// Accepts connections until shutdown, giving each its own thread, then waits for those threads to finish.
fn accept_loop(listener: TcpListener, context: Context) {
    let mut readers: Vec<JoinHandle<()>> = Vec::new();
    let mut next_client = 1u64;

    while !context.shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let client = next_client;
                let reader_context = context.clone();
                next_client += 1;

                let spawned = thread::Builder::new()
                    .name("mcplink-client".into())
                    .spawn(move || serve(stream, client, reader_context));
                if let Ok(reader) = spawned {
                    readers.push(reader);
                }
            }
            Err(_) => thread::sleep(TICK),
        }

        readers.retain(|reader| !reader.is_finished());
    }

    for reader in readers {
        let _ = reader.join();
    }
}

/// Authenticates one connection, then forwards its request lines to the game thread until it closes.
fn serve(stream: TcpStream, client: u64, context: Context) {
    let configured = stream
        .set_nonblocking(false)
        .and_then(|()| stream.set_read_timeout(Some(TICK)))
        .and_then(|()| stream.set_write_timeout(Some(WRITE_TIMEOUT)))
        .and_then(|()| stream.set_nodelay(true));
    if configured.is_err() {
        return;
    }

    let Ok(mut writer) = stream.try_clone() else {
        return;
    };

    let mut reader = BufReader::new(stream);
    let deadline = Instant::now() + AUTH_WINDOW;
    let Some(presented) = read_line(
        &mut reader,
        &context.shutdown,
        MAX_AUTH_LINE,
        Some(deadline),
    ) else {
        return;
    };

    if !constant_time_eq(&presented, context.token.as_bytes()) || writer.write_all(HELLO).is_err() {
        return;
    }

    lock(&context.clients).insert(client, writer);

    while let Some(data) = read_line(&mut reader, &context.shutdown, MAX_LINE, None) {
        if context.outbox.send(Message { client, data }).is_err() {
            break;
        }
    }

    lock(&context.clients).remove(&client);
}

/// Reads one line without its terminator, or returns `None` on disconnect, shutdown, an oversized line or a missed deadline.
fn read_line(
    reader: &mut BufReader<TcpStream>,
    shutdown: &AtomicBool,
    limit: usize,
    deadline: Option<Instant>,
) -> Option<Vec<u8>> {
    let mut line = Vec::new();

    loop {
        if shutdown.load(Ordering::SeqCst)
            || deadline.is_some_and(|deadline| Instant::now() > deadline)
        {
            return None;
        }

        let (consumed, finished) = match reader.fill_buf() {
            Ok([]) => return None,
            Ok(available) => match available.iter().position(|&byte| byte == b'\n') {
                Some(end) => {
                    line.extend_from_slice(&available[..end]);
                    (end + 1, true)
                }
                None => {
                    line.extend_from_slice(available);
                    (available.len(), false)
                }
            },
            Err(error) if is_idle(&error) => continue,
            Err(_) => return None,
        };

        reader.consume(consumed);

        if line.len() > limit {
            return None;
        }

        if finished {
            if line.last() == Some(&b'\r') {
                line.pop();
            }

            return Some(line);
        }
    }
}
