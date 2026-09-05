//! Private native-UI transport with a migration execution fence.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tungstenite::{Message, WebSocket};

pub(super) struct Proxy {
    pub endpoint: String,
    shared: Arc<Shared>,
    listener: Option<JoinHandle<()>>,
    _directory: TempDir,
}

struct Shared {
    expected: String,
    gate: Mutex<Gate>,
    stop: AtomicBool,
    listening: AtomicBool,
    next_connection: AtomicU64,
    sockets: Mutex<HashMap<u64, Vec<UnixStream>>>,
}

struct Gate {
    fenced: bool,
    current_session: String,
    pending_mutation: usize,
    pending_server_request: usize,
    active_processes: usize,
    active_realtime: usize,
    attached: usize,
    connections: usize,
    ever_connected: bool,
    uncertain_mutation: bool,
}

impl Proxy {
    pub fn start(upstream: &str, session_id: &str, initially_fenced: bool) -> Result<Self, String> {
        let upstream = upstream
            .strip_prefix("unix://")
            .filter(|path| Path::new(path).is_absolute())
            .ok_or("Codex UI proxy requires an absolute unix:// endpoint")?;
        if session_id.is_empty() {
            if initially_fenced {
                return Err("a staged Codex UI proxy requires a known session".into());
            }
        } else {
            crate::session::SessionId::new(session_id)?;
        }
        let directory = tempfile::Builder::new()
            .prefix("ah-ui-")
            .tempdir()
            .map_err(super::error)?;
        let socket = directory.path().join("s");
        let listener = UnixListener::bind(&socket).map_err(super::error)?;
        listener.set_nonblocking(true).map_err(super::error)?;
        let shared = Arc::new(Shared {
            expected: session_id.into(),
            gate: Mutex::new(Gate {
                fenced: initially_fenced,
                current_session: session_id.into(),
                pending_mutation: 0,
                pending_server_request: 0,
                active_processes: 0,
                active_realtime: 0,
                attached: 0,
                connections: 0,
                ever_connected: false,
                uncertain_mutation: false,
            }),
            stop: AtomicBool::new(false),
            listening: AtomicBool::new(true),
            next_connection: AtomicU64::new(0),
            sockets: Mutex::new(HashMap::new()),
        });
        let worker_state = Arc::clone(&shared);
        let upstream = PathBuf::from(upstream);
        let worker = thread::Builder::new()
            .name("agent-hop-ui-proxy".into())
            .spawn(move || listen(listener, upstream, worker_state))
            .map_err(super::error)?;
        Ok(Self {
            endpoint: format!("unix://{}", socket.display()),
            shared,
            listener: Some(worker),
            _directory: directory,
        })
    }

    /// A live native client has resumed the expected session, with no mutation RPC pending.
    pub fn ready(&self) -> bool {
        let alive = self.alive();
        let gate = self.shared.gate.lock().unwrap_or_else(|e| e.into_inner());
        alive
            && self.shared.listening.load(Ordering::Acquire)
            && !self.shared.stop.load(Ordering::Acquire)
            && gate.attached > 0
            && gate.pending_mutation == 0
            && gate.pending_server_request == 0
            && gate.active_processes == 0
            && gate.active_realtime == 0
            && !gate.uncertain_mutation
    }

    pub fn alive(&self) -> bool {
        let gate = self.shared.gate.lock().unwrap_or_else(|e| e.into_inner());
        self.shared.listening.load(Ordering::Acquire)
            && !self.shared.stop.load(Ordering::Acquire)
            && (!gate.ever_connected || gate.connections > 0)
    }

    /// A forwarding barrier. After fencing, wait for `ready()` before reading the selected ID.
    pub fn fence(&self, fenced: bool) {
        self.shared
            .gate
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fenced = fenced;
    }

    pub fn current_session(&self) -> String {
        self.shared
            .gate
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .current_session
            .clone()
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        let sockets = self
            .shared
            .sockets
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for socket in sockets.values().flatten() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        drop(sockets);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

fn listen(listener: UnixListener, upstream: PathBuf, shared: Arc<Shared>) {
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    while !shared.stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let worker_state = Arc::clone(&shared);
                let upstream = upstream.clone();
                match thread::Builder::new()
                    .name("agent-hop-ui-client".into())
                    .spawn(move || {
                        let _ = forward(stream, &upstream, worker_state);
                    }) {
                    Ok(worker) => workers.push(worker),
                    Err(_) => break,
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break,
        }
        let mut index = 0;
        while index < workers.len() {
            if workers[index].is_finished() {
                let _ = workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
    }
    shared.listening.store(false, Ordering::Release);
    shared.stop.store(true, Ordering::Release);
    let sockets = shared.sockets.lock().unwrap_or_else(|e| e.into_inner());
    for socket in sockets.values().flatten() {
        let _ = socket.shutdown(Shutdown::Both);
    }
    drop(sockets);
    for worker in workers {
        let _ = worker.join();
    }
}

struct Connection {
    shared: Arc<Shared>,
    id: u64,
    attached: bool,
    mutations: HashSet<String>,
    selections: HashSet<String>,
    expected_resumes: HashSet<String>,
    server_requests: HashSet<String>,
    blocked_replies: VecDeque<Message>,
    process_requests: HashMap<String, String>,
    processes: HashSet<String>,
    realtime_requests: HashMap<String, String>,
    realtime: HashSet<String>,
}

impl Connection {
    fn new(shared: Arc<Shared>, stream: &UnixStream) -> Result<Self, String> {
        let id = shared.next_connection.fetch_add(1, Ordering::Relaxed);
        let mut sockets = shared.sockets.lock().unwrap_or_else(|e| e.into_inner());
        if shared.stop.load(Ordering::Acquire) || sockets.len() >= 16 {
            return Err("Codex UI proxy is stopping or has too many clients".into());
        }
        sockets.insert(id, vec![stream.try_clone().map_err(super::error)?]);
        drop(sockets);
        let mut gate = shared.gate.lock().unwrap_or_else(|e| e.into_inner());
        gate.connections += 1;
        gate.ever_connected = true;
        drop(gate);
        Ok(Self {
            shared,
            id,
            attached: false,
            mutations: HashSet::new(),
            selections: HashSet::new(),
            expected_resumes: HashSet::new(),
            server_requests: HashSet::new(),
            blocked_replies: VecDeque::new(),
            process_requests: HashMap::new(),
            processes: HashSet::new(),
            realtime_requests: HashMap::new(),
            realtime: HashSet::new(),
        })
    }

    fn register(&self, stream: &UnixStream) -> Result<(), String> {
        let mut sockets = self
            .shared
            .sockets
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.shared.stop.load(Ordering::Acquire) {
            return Err("Codex UI proxy is stopping".into());
        }
        sockets
            .get_mut(&self.id)
            .ok_or("Codex UI proxy lost its connection")?
            .push(stream.try_clone().map_err(super::error)?);
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let mut sockets = self
            .shared
            .sockets
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(streams) = sockets.remove(&self.id) {
            for socket in streams {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
        drop(sockets);
        let mut gate = self.shared.gate.lock().unwrap_or_else(|e| e.into_inner());
        gate.connections -= 1;
        // A disconnected client may have queued a write without receiving its
        // acknowledgement. Do not let a second connection turn that into ready.
        gate.uncertain_mutation |= !self.mutations.is_empty()
            || !self.server_requests.is_empty()
            || !self.processes.is_empty()
            || !self.realtime.is_empty();
        gate.pending_mutation -= self.mutations.len();
        gate.pending_server_request -= self.server_requests.len();
        gate.active_processes -= self.processes.len();
        gate.active_realtime -= self.realtime.len();
        if self.attached {
            gate.attached -= 1;
        }
    }
}

fn timeout(stream: &UnixStream, read: Duration) -> Result<(), String> {
    // BSD/macOS accept() inherits O_NONBLOCK from our polling listener; Linux
    // does not. The WebSocket handshake and bounded writes below are blocking.
    // SO_RCVTIMEO/SO_SNDTIMEO alone do not clear the inherited descriptor flag.
    stream.set_nonblocking(false).map_err(super::error)?;
    stream.set_read_timeout(Some(read)).map_err(super::error)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(super::error)
}

fn forward(stream: UnixStream, upstream: &Path, shared: Arc<Shared>) -> Result<(), String> {
    let mut connection = Connection::new(shared, &stream)?;
    timeout(&stream, Duration::from_secs(3))?;
    let mut client = tungstenite::accept(stream).map_err(super::error)?;
    let stream = UnixStream::connect(upstream).map_err(super::error)?;
    connection.register(&stream)?;
    timeout(&stream, Duration::from_secs(3))?;
    let (mut server, _) = tungstenite::client("ws://localhost/", stream).map_err(super::error)?;
    timeout(client.get_ref(), Duration::from_millis(1))?;
    timeout(server.get_ref(), Duration::from_millis(1))?;
    while !connection.shared.stop.load(Ordering::Acquire) {
        let fenced = connection
            .shared
            .gate
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fenced;
        if !fenced && let Some(reply) = connection.blocked_replies.pop_front() {
            client_message(reply, &mut client, &mut server, &mut connection)?;
        }
        if let Some(message) = read(&mut client)? {
            client_message(message, &mut client, &mut server, &mut connection)?;
        }
        if let Some(message) = read(&mut server)? {
            server_message(message, &mut client, &mut connection)?;
        }
    }
    Ok(())
}

fn read(socket: &mut WebSocket<UnixStream>) -> Result<Option<Message>, String> {
    match socket.read() {
        Ok(Message::Close(_)) => Err("Codex UI connection closed".into()),
        Ok(message) => Ok(Some(message)),
        Err(tungstenite::Error::Io(e))
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(super::error(e)),
    }
}

fn read_only(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "initialized"
            | "account/read"
            | "account/rateLimits/read"
            | "app/list"
            | "config/read"
            | "configRequirements/read"
            | "externalAgentConfig/detect"
            | "experimentalFeature/list"
            | "model/list"
            | "mcpServerStatus/list"
            | "skills/list"
            | "thread/list"
            | "thread/loaded/list"
            | "thread/read"
            | "thread/turns/list"
            | "thread/items/list"
            | "thread/goal/get"
            | "thread/queue/list"
            | "thread/realtime/listVoices"
            | "thread/backgroundTerminals/list"
            | "remoteControl/status/read"
            | "remoteControl/clients/list"
            | "remoteControl/pairing/status"
    )
}

fn client_message(
    mut message: Message,
    client: &mut WebSocket<UnixStream>,
    server: &mut WebSocket<UnixStream>,
    connection: &mut Connection,
) -> Result<(), String> {
    let Message::Text(text) = &message else {
        // App-server JSON-RPC is text-only. Never tunnel an uninspected binary request.
        return match message {
            Message::Ping(_) | Message::Pong(_) => Ok(()),
            _ => Err("Codex UI sent a non-text RPC message".into()),
        };
    };
    let mut value: Value = serde_json::from_str(text).map_err(super::error)?;
    if !value.is_object() {
        return Err("Codex UI sent a non-object RPC message".into());
    }
    let mut gate = connection
        .shared
        .gate
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let method_owned = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let method = method_owned.as_deref();
    if value.get("method").is_some() && method.is_none() {
        return Err("Codex UI request has an invalid method".into());
    }
    if method.is_some()
        && value
            .get("id")
            .is_some_and(|id| connection.mutations.contains(&id.to_string()))
    {
        return Err("Codex UI reused an outstanding mutation request ID".into());
    }
    if method.is_none() {
        let id = value
            .get("id")
            .ok_or("Codex UI response has no ID")?
            .to_string();
        if !connection.server_requests.contains(&id)
            || (value.get("result").is_none() == value.get("error").is_none())
        {
            return Err("Codex UI response does not match an outstanding server request".into());
        }
        if gate.fenced {
            if connection.blocked_replies.len() >= connection.server_requests.len() {
                return Err("Codex UI duplicated a fenced server-request response".into());
            }
            connection.blocked_replies.push_back(message);
            return Ok(());
        }
        server.send(message).map_err(super::error)?;
        connection.server_requests.remove(&id);
        gate.pending_server_request -= 1;
        return Ok(());
    }
    let same_resume = method == Some("thread/resume")
        && value.pointer("/params/threadId").and_then(Value::as_str)
            == Some(gate.current_session.as_str())
        && value.pointer("/params/history").is_none_or(Value::is_null)
        && value.pointer("/params/path").is_none_or(Value::is_null);
    let token_refresh = method == Some("account/read")
        && value
            .pointer("/params/refreshToken")
            .and_then(Value::as_bool)
            == Some(true);
    let remote_control = matches!(
        method,
        Some("remoteControl/enable" | "remoteControl/pairing/start")
    );
    if remote_control
        || (gate.fenced
            && (token_refresh || method.is_some_and(|method| !read_only(method) && !same_resume)))
    {
        if let Some(id) = value.get("id") {
            let reason = if remote_control {
                "agent-hop: remote-control exposure bypasses managed handoff ownership; use an unmanaged Codex session for remote control"
            } else {
                "agent-hop: execution is fenced during session handoff; retry after takeover or recovery"
            };
            client
                .send(Message::Text(
                    json!({"id":id,"error":{"code":-32001,"message":reason}})
                        .to_string()
                        .into(),
                ))
                .map_err(super::error)?;
        }
        return Ok(());
    }
    if gate.fenced && same_resume {
        // The supervisor already opened this thread with destination-local settings.
        // Native attachment cannot override its cwd, tools, approval or sandbox policy
        // while the destination is staged. Preserve only the readback-size preference.
        value["params"] = json!({"threadId":gate.current_session,"excludeTurns":value.pointer("/params/excludeTurns").and_then(Value::as_bool).unwrap_or(false)});
        message = Message::Text(value.to_string().into());
    }
    let selection = matches!(
        method,
        Some("thread/start" | "thread/fork" | "thread/resume")
    );
    if token_refresh || method.is_some_and(|method| !read_only(method)) {
        let id = value
            .get("id")
            .filter(|id| id.is_number() || id.is_string())
            .ok_or("Codex UI mutation request has no valid ID")?
            .to_string();
        if !connection.mutations.insert(id.clone()) {
            return Err("Codex UI reused an outstanding mutation request ID".into());
        }
        gate.pending_mutation += 1;
        if selection {
            connection.selections.insert(id.clone());
        }
        if method == Some("process/spawn") {
            let handle = value
                .pointer("/params/processHandle")
                .and_then(Value::as_str)
                .ok_or("Codex UI process spawn has no handle")?;
            if !connection.processes.insert(handle.into()) {
                return Err("Codex UI reused a live process handle".into());
            }
            gate.active_processes += 1;
            connection
                .process_requests
                .insert(id.clone(), handle.into());
        }
        if method == Some("thread/realtime/start") {
            let thread = value
                .pointer("/params/threadId")
                .and_then(Value::as_str)
                .ok_or("Codex realtime start has no thread ID")?;
            if !connection.realtime.insert(thread.into()) {
                return Err("Codex UI restarted an active realtime session".into());
            }
            gate.active_realtime += 1;
            connection
                .realtime_requests
                .insert(id.clone(), thread.into());
        }
        if (method == Some("thread/resume")
            && value.pointer("/params/threadId").and_then(Value::as_str)
                == Some(connection.shared.expected.as_str()))
            || (method == Some("thread/start") && connection.shared.expected.is_empty())
        {
            connection.expected_resumes.insert(id);
        }
    }
    // The fence mutex covers the write: fence(true) cannot return before an earlier
    // frontend write is sent. In-flight mutation replies keep ready() false.
    server.send(message).map_err(super::error)
}

fn server_message(
    message: Message,
    client: &mut WebSocket<UnixStream>,
    connection: &mut Connection,
) -> Result<(), String> {
    match &message {
        Message::Text(text) => {
            let value: Value = serde_json::from_str(text).map_err(super::error)?;
            let mut gate = connection
                .shared
                .gate
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if value.get("method").and_then(Value::as_str).is_some()
                && let Some(id) = value.get("id")
            {
                if !connection.server_requests.insert(id.to_string()) {
                    return Err("Codex app-server reused an outstanding request ID".into());
                }
                gate.pending_server_request += 1;
            }
            // Readiness is published only after the successful response is forwarded.
            client.send(message).map_err(super::error)?;
            if value.get("method").and_then(Value::as_str) == Some("serverRequest/resolved")
                && let Some(id) = value.pointer("/params/requestId").map(Value::to_string)
                && connection.server_requests.remove(&id)
            {
                gate.pending_server_request -= 1;
                connection.blocked_replies.retain(|reply| {
                    let Message::Text(text) = reply else {
                        return false;
                    };
                    serde_json::from_str::<Value>(text)
                        .ok()
                        .and_then(|reply| reply.get("id").map(Value::to_string))
                        .is_some_and(|reply_id| reply_id != id)
                });
            }
            if value.get("method").and_then(Value::as_str) == Some("process/exited")
                && let Some(handle) = value
                    .pointer("/params/processHandle")
                    .and_then(Value::as_str)
                && connection.processes.remove(handle)
            {
                gate.active_processes -= 1;
            }
            if value.get("method").and_then(Value::as_str) == Some("thread/realtime/closed")
                && let Some(thread) = value.pointer("/params/threadId").and_then(Value::as_str)
                && connection.realtime.remove(thread)
            {
                gate.active_realtime -= 1;
            }
            if value.get("method").is_none()
                && let Some(id) = value.get("id").map(Value::to_string)
            {
                if connection.mutations.remove(&id) {
                    gate.pending_mutation -= 1;
                }
                if let Some(handle) = connection.process_requests.remove(&id)
                    && value.get("error").is_some()
                    && connection.processes.remove(&handle)
                {
                    gate.active_processes -= 1;
                }
                if let Some(thread) = connection.realtime_requests.remove(&id)
                    && value.get("error").is_some()
                    && connection.realtime.remove(&thread)
                {
                    gate.active_realtime -= 1;
                }
                if connection.selections.remove(&id) {
                    let expected_resume = connection.expected_resumes.remove(&id);
                    if value.get("error").is_none()
                        && let Some(session) =
                            value.pointer("/result/thread/id").and_then(Value::as_str)
                    {
                        crate::session::SessionId::new(session)?;
                        gate.current_session = session.into();
                        if expected_resume
                            && (connection.shared.expected.is_empty()
                                || session == connection.shared.expected)
                            && !connection.attached
                        {
                            connection.attached = true;
                            gate.attached += 1;
                        }
                    }
                }
            }
            Ok(())
        }
        Message::Ping(_) | Message::Pong(_) => Ok(()),
        _ => Err("Codex app-server sent a non-text RPC message".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Instant;

    const SESSION: &str = "01900000-0000-7000-8000-000000000001";
    const NEXT_SESSION: &str = "01900000-0000-7000-8000-000000000002";

    struct Fixture {
        proxy: Proxy,
        client: WebSocket<UnixStream>,
        requests: Receiver<Value>,
        server: Option<JoinHandle<()>>,
        _directory: TempDir,
    }

    impl Fixture {
        fn new(fenced: bool) -> Self {
            Self::with_session(fenced, SESSION)
        }

        fn with_session(fenced: bool, session: &str) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("s");
            let listener = UnixListener::bind(&path).unwrap();
            let (tx, requests) = mpsc::channel();
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let mut socket = tungstenite::accept(stream).unwrap();
                while let Ok(Message::Text(text)) = socket.read() {
                    let request: Value = serde_json::from_str(&text).unwrap();
                    if tx.send(request.clone()).is_err() {
                        break;
                    }
                    if request.get("method").is_none() {
                        continue;
                    }
                    if request["method"] == "test/requestApproval" {
                        socket.send(Message::Text(json!({"id":"approval-1","method":"item/commandExecution/requestApproval","params":{}}).to_string().into())).unwrap();
                    }
                    if let Some(delay) = request.pointer("/params/holdMs").and_then(Value::as_u64) {
                        thread::sleep(Duration::from_millis(delay));
                    }
                    if request
                        .pointer("/params/exitBeforeAck")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        socket.send(Message::Text(json!({"method":"process/exited","params":{"processHandle":request["params"]["processHandle"],"exitCode":0}}).to_string().into())).unwrap();
                    }
                    let result = match request["method"].as_str() {
                        Some("thread/resume") => {
                            json!({"thread":{"id":request["params"].get("replySession").unwrap_or(&request["params"]["threadId"])}})
                        }
                        Some("thread/start" | "thread/fork") => {
                            json!({"thread":{"id":NEXT_SESSION}})
                        }
                        _ => json!({"ok":true}),
                    };
                    let response = if request.pointer("/params/fail").and_then(Value::as_bool)
                        == Some(true)
                    {
                        json!({"id":request["id"],"error":{"code":-32000,"message":"resume failed"}})
                    } else {
                        json!({"id":request["id"],"result":result})
                    };
                    if request.get("id").is_some()
                        && socket
                            .send(Message::Text(response.to_string().into()))
                            .is_err()
                    {
                        break;
                    }
                    if request
                        .pointer("/params/resolveApproval")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        thread::sleep(Duration::from_millis(100));
                        socket.send(Message::Text(json!({"method":"serverRequest/resolved","params":{"requestId":"approval-1","threadId":SESSION}}).to_string().into())).unwrap();
                    }
                    if request
                        .pointer("/params/exitAfterAck")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        thread::sleep(Duration::from_millis(100));
                        socket.send(Message::Text(json!({"method":"process/exited","params":{"processHandle":request["params"]["processHandle"],"exitCode":0}}).to_string().into())).unwrap();
                    }
                    if request
                        .pointer("/params/closeRealtime")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        thread::sleep(Duration::from_millis(100));
                        socket.send(Message::Text(json!({"method":"thread/realtime/closed","params":{"threadId":request["params"]["threadId"]}}).to_string().into())).unwrap();
                    }
                }
            });
            let proxy =
                Proxy::start(&format!("unix://{}", path.display()), session, fenced).unwrap();
            let stream =
                UnixStream::connect(proxy.endpoint.strip_prefix("unix://").unwrap()).unwrap();
            timeout(&stream, Duration::from_secs(2)).unwrap();
            let (client, _) = tungstenite::client("ws://localhost/", stream).unwrap();
            Self {
                proxy,
                client,
                requests,
                server: Some(server),
                _directory: directory,
            }
        }

        fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
            self.client
                .send(Message::Text(
                    json!({"id":id,"method":method,"params":params})
                        .to_string()
                        .into(),
                ))
                .unwrap();
            loop {
                if let Message::Text(text) = self.client.read().unwrap() {
                    return serde_json::from_str(&text).unwrap();
                }
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.client.get_ref().shutdown(Shutdown::Both);
            if let Some(server) = self.server.take() {
                let _ = server.join();
            }
        }
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < until, "condition did not become true");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn accepted_nonblocking_transport_is_normalized_before_handshake_and_writes() {
        use std::io::Read;
        let (mut accepted, _peer) = UnixStream::pair().unwrap();
        accepted.set_nonblocking(true).unwrap();
        timeout(&accepted, Duration::from_millis(20)).unwrap();
        let started = Instant::now();
        let error = accepted.read(&mut [0; 1]).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
        assert!(
            started.elapsed() >= Duration::from_millis(10),
            "accepted socket remained nonblocking"
        );
    }

    #[test]
    fn new_native_ui_establishes_its_first_persisted_thread_without_a_resume_id() {
        let mut fixture = Fixture::with_session(false, "");
        assert!(!fixture.proxy.ready());
        fixture.request(1, "thread/start", json!({}));
        assert!(fixture.proxy.ready());
        assert_eq!(fixture.proxy.current_session(), NEXT_SESSION);
        assert!(Proxy::start("unix:///tmp/unused", "", true).is_err());
    }

    #[test]
    fn native_expected_resume_is_required_and_disconnect_revokes_readiness() {
        let mut fixture = Fixture::new(false);
        fixture.request(1, "initialize", json!({}));
        assert!(!fixture.proxy.ready());
        fixture.request(2, "thread/resume", json!({"threadId":NEXT_SESSION}));
        assert!(!fixture.proxy.ready());
        fixture.request(3, "thread/resume", json!({"threadId":SESSION}));
        assert!(fixture.proxy.ready());
        assert!(fixture.proxy.alive());
        fixture.client.get_ref().shutdown(Shutdown::Both).unwrap();
        wait_until(|| !fixture.proxy.alive());
        assert!(!fixture.proxy.ready());
    }

    #[test]
    fn failed_or_mismatched_resume_response_does_not_prove_readiness() {
        let mut fixture = Fixture::new(false);
        fixture.request(1, "thread/resume", json!({"threadId":SESSION,"fail":true}));
        assert!(!fixture.proxy.ready());
        assert_eq!(fixture.proxy.current_session(), SESSION);
        fixture.request(
            2,
            "thread/resume",
            json!({"threadId":SESSION,"replySession":NEXT_SESSION}),
        );
        assert!(!fixture.proxy.ready());
        fixture.request(3, "thread/resume", json!({"threadId":SESSION}));
        assert!(fixture.proxy.ready());
        fixture.request(4, "thread/start", json!({"fail":true}));
        assert!(fixture.proxy.ready());
        assert_eq!(fixture.proxy.current_session(), SESSION);
    }

    #[test]
    fn fence_rejects_unknown_and_execution_requests_but_allows_reads_and_expected_resume() {
        let mut fixture = Fixture::new(true);
        fixture.request(1, "thread/resume", json!({"threadId":SESSION,"cwd":"/outside","approvalPolicy":"never","sandbox":"danger-full-access","config":{"other":"override"}}));
        let received = fixture
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            received["params"],
            json!({"threadId":SESSION,"excludeTurns":false})
        );
        assert!(fixture.proxy.ready());
        for (index, method) in [
            "turn/start",
            "turn/steer",
            "review/start",
            "thread/start",
            "thread/fork",
            "thread/goal/set",
            "thread/compact/start",
            "thread/shellCommand",
            "command/exec",
            "process/spawn",
            "future/newMutation",
        ]
        .into_iter()
        .enumerate()
        {
            let reply = fixture.request(index as u64 + 10, method, json!({}));
            assert_eq!(reply["error"]["code"], -32001, "{method}");
            assert!(
                fixture.requests.try_recv().is_err(),
                "{method} reached the backend"
            );
        }
        let reply = fixture.request(30, "thread/resume", json!({"threadId":NEXT_SESSION}));
        assert_eq!(reply["error"]["code"], -32001);
        let reply = fixture.request(
            31,
            "thread/resume",
            json!({"threadId":SESSION,"path":"/untrusted/rollout"}),
        );
        assert_eq!(reply["error"]["code"], -32001);
        let reply = fixture.request(34, "account/read", json!({"refreshToken":true}));
        assert_eq!(reply["error"]["code"], -32001);
        let reply = fixture.request(32, "thread/read", json!({"threadId":SESSION}));
        assert_eq!(reply["result"]["ok"], true);
        assert_eq!(
            fixture
                .requests
                .recv_timeout(Duration::from_secs(2))
                .unwrap()["method"],
            "thread/read"
        );
        fixture.proxy.fence(false);
        let reply = fixture.request(33, "turn/start", json!({}));
        assert_eq!(reply["result"]["ok"], true);
        assert_eq!(
            fixture
                .requests
                .recv_timeout(Duration::from_secs(2))
                .unwrap()["method"],
            "turn/start"
        );
    }

    #[test]
    fn native_new_and_fork_update_selected_session_when_unfenced() {
        let mut fixture = Fixture::new(false);
        fixture.request(1, "thread/resume", json!({"threadId":SESSION}));
        assert!(fixture.proxy.ready());
        fixture.request(2, "thread/start", json!({}));
        assert_eq!(fixture.proxy.current_session(), NEXT_SESSION);
        assert!(fixture.proxy.ready());
        fixture.proxy.fence(true);
        fixture.request(3, "thread/resume", json!({"threadId":NEXT_SESSION}));
        assert!(fixture.proxy.ready());
        fixture.proxy.fence(false);
        fixture.request(4, "thread/resume", json!({"threadId":SESSION}));
        fixture.request(5, "thread/fork", json!({"threadId":SESSION}));
        assert_eq!(fixture.proxy.current_session(), NEXT_SESSION);
    }

    #[test]
    fn fencing_waits_for_previously_forwarded_mutation_and_selection_replies() {
        let mut fixture = Fixture::new(false);
        fixture.request(1, "thread/resume", json!({"threadId":SESSION}));
        fixture
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        for (index, method) in ["turn/start", "thread/start"].into_iter().enumerate() {
            fixture.proxy.fence(false);
            fixture
                .client
                .send(Message::Text(
                    json!({"id":index + 2,"method":method,"params":{"holdMs":100}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
            assert_eq!(
                fixture
                    .requests
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()["method"],
                method
            );
            fixture.proxy.fence(true);
            assert!(!fixture.proxy.ready(), "in-flight {method} was not fenced");
            fixture.client.read().unwrap();
            wait_until(|| fixture.proxy.ready());
        }
        assert_eq!(fixture.proxy.current_session(), NEXT_SESSION);
    }

    #[test]
    fn server_approval_replies_wait_behind_the_fence_and_keep_readiness_false() {
        let mut fixture = Fixture::new(false);
        fixture.request(1, "thread/resume", json!({"threadId":SESSION}));
        fixture
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        fixture
            .client
            .send(Message::Text(
                json!({"id":2,"method":"test/requestApproval","params":{}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        fixture
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        for _ in 0..2 {
            fixture.client.read().unwrap();
        }
        assert!(!fixture.proxy.ready());
        fixture.proxy.fence(true);
        fixture
            .client
            .send(Message::Text(
                json!({"id":"approval-1","result":{"decision":"accept"}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        assert!(
            fixture
                .requests
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );
        assert!(!fixture.proxy.ready());
        fixture.proxy.fence(false);
        assert_eq!(
            fixture
                .requests
                .recv_timeout(Duration::from_secs(2))
                .unwrap()["id"],
            "approval-1"
        );
        wait_until(|| fixture.proxy.ready());
    }

    #[test]
    fn server_resolved_approval_discards_a_buffered_native_reply() {
        let mut fixture = Fixture::new(false);
        fixture.request(1, "thread/resume", json!({"threadId":SESSION}));
        fixture
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        fixture
            .client
            .send(Message::Text(
                json!({"id":2,"method":"test/requestApproval","params":{"resolveApproval":true}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        fixture
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        for _ in 0..2 {
            fixture.client.read().unwrap();
        }
        fixture.proxy.fence(true);
        fixture
            .client
            .send(Message::Text(
                json!({"id":"approval-1","result":{"decision":"accept"}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        assert!(!fixture.proxy.ready());
        fixture.client.read().unwrap();
        wait_until(|| fixture.proxy.ready());
        fixture.proxy.fence(false);
        assert!(
            fixture
                .requests
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );
    }

    #[test]
    fn standalone_process_spawn_ack_is_not_execution_completion() {
        let mut fixture = Fixture::new(false);
        fixture.request(1, "thread/resume", json!({"threadId":SESSION}));
        fixture.request(
            2,
            "process/spawn",
            json!({"processHandle":"child","exitAfterAck":true}),
        );
        assert!(!fixture.proxy.ready());
        fixture.proxy.fence(true);
        fixture.client.read().unwrap();
        wait_until(|| fixture.proxy.ready());
        fixture.proxy.fence(false);
        fixture.request(
            3,
            "process/spawn",
            json!({"processHandle":"child","fail":true}),
        );
        assert!(fixture.proxy.ready());
        fixture.client.send(Message::Text(json!({"id":4,"method":"process/spawn","params":{"processHandle":"child","exitBeforeAck":true}}).to_string().into())).unwrap();
        for _ in 0..2 {
            fixture.client.read().unwrap();
        }
        wait_until(|| fixture.proxy.ready());
    }

    #[test]
    fn realtime_start_stays_busy_until_closed_and_remote_control_cannot_bypass_proxy() {
        let mut fixture = Fixture::new(false);
        fixture.request(1, "thread/resume", json!({"threadId":SESSION}));
        fixture.request(
            2,
            "thread/realtime/start",
            json!({"threadId":SESSION,"closeRealtime":true}),
        );
        assert!(!fixture.proxy.ready());
        fixture.client.read().unwrap();
        wait_until(|| fixture.proxy.ready());
        fixture.request(
            3,
            "thread/realtime/start",
            json!({"threadId":SESSION,"fail":true}),
        );
        assert!(fixture.proxy.ready());
        for (index, method) in ["remoteControl/enable", "remoteControl/pairing/start"]
            .into_iter()
            .enumerate()
        {
            let result = fixture.request(10 + index as u64, method, json!({}));
            assert_eq!(result["error"]["code"], -32001);
        }
    }

    #[test]
    fn drop_interrupts_an_incomplete_client_handshake_and_removes_socket() {
        let directory = tempfile::tempdir().unwrap();
        let proxy = Proxy::start(
            &format!("unix://{}/upstream", directory.path().display()),
            SESSION,
            true,
        )
        .unwrap();
        let path = PathBuf::from(proxy.endpoint.strip_prefix("unix://").unwrap());
        let _client = UnixStream::connect(&path).unwrap();
        wait_until(|| proxy.shared.gate.lock().unwrap().connections > 0);
        let start = Instant::now();
        drop(proxy);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!path.exists());
    }

    #[test]
    fn proxy_never_accepts_a_network_upstream() {
        assert!(Proxy::start("ws://example.com/", SESSION, true).is_err());
        assert!(Proxy::start("unix://relative/path", SESSION, true).is_err());
    }
}
