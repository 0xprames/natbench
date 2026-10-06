//! TCP simultaneous open on a port the discovery connection does not hold.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    io::{BufRead, BufReader, ErrorKind, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    os::unix::io::AsRawFd,
    thread,
    time::{Duration, Instant},
};

const RELAY: &str = "198.18.0.1:9100";
const OBSERVER: &str = "198.18.0.1:9200";
const DISCOVERY_PORT: u16 = 10001;
const PUNCH_PORT: u16 = 10000;

pub fn observe() -> Result<()> {
    let listener = TcpListener::bind(OBSERVER).context("bind the tcp observer")?;
    emit(&json!({"ready": true}))?;
    for connection in listener.incoming() {
        let Ok(mut connection) = connection else {
            continue;
        };
        let Ok(SocketAddr::V4(address)) = connection.peer_addr() else {
            continue;
        };
        let _ = connection.set_write_timeout(Some(Duration::from_secs(2)));
        let _ = writeln!(
            connection,
            "{}",
            json!({"mapped": [address.ip().to_string(), address.port()]})
        );
    }
    Ok(())
}

pub fn run(role: &str) -> Result<()> {
    anyhow::ensure!(role == "a" || role == "b", "tcp role must be a or b");
    let (public_ip, discovery_port) = discover()?;
    emit(&json!({
        "ready": true,
        "public_ip": public_ip,
        "discovery_port": discovery_port,
    }))?;
    let remote = rendezvous(role, &public_ip)?;
    match open_punch(remote) {
        Punch::Connected(mut stream) => {
            stream.set_nodelay(true)?;
            let before = exchange(&mut stream, role, "tcp-before")?;
            emit(&json!({"connected": true, "data_before": before}))?;
            wait_again()?;
            let after = exchange(&mut stream, role, "tcp-after")?;
            emit(&json!({"data_after": after}))?;
            hold()?;
        }
        Punch::SynSent(socket) => {
            emit(&json!({"connected": false, "data_before": false}))?;
            wait_again()?;
            drop(socket);
            emit(&json!({"data_after": false}))?;
            hold()?;
        }
        Punch::Failed => {
            emit(&json!({"connected": false, "data_before": false}))?;
            wait_again()?;
            emit(&json!({"data_after": false}))?;
            hold()?;
        }
    }
    Ok(())
}

enum Punch {
    Connected(TcpStream),
    SynSent(Socket),
    Failed,
}

fn discover() -> Result<(String, u64)> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, None)?;
    let local = SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT));
    socket
        .bind(&SockAddr::from(local))
        .context("bind the tcp discovery socket")?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let observer: SocketAddr = OBSERVER.parse()?;
    socket
        .connect(&SockAddr::from(observer))
        .context("connect to the tcp observer")?;
    let stream = TcpStream::from(socket);
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let value: Value = serde_json::from_str(line.trim()).context("parse the observed mapping")?;
    let mapped = value
        .get("mapped")
        .and_then(Value::as_array)
        .context("observer omitted the mapped endpoint")?;
    let ip = mapped
        .first()
        .and_then(Value::as_str)
        .context("observer omitted the public address")?
        .to_string();
    let port = mapped
        .get(1)
        .and_then(Value::as_u64)
        .context("observer omitted the public port")?;
    Ok((ip, port))
}

fn open_punch(peer: SocketAddr) -> Punch {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let budget = deadline.saturating_duration_since(Instant::now());
        if budget.is_zero() {
            return Punch::Failed;
        }
        match connect_once(peer, budget) {
            Ok(punch) => return punch,
            Err(error) if retryable(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return Punch::Failed,
        }
    }
}

fn connect_once(peer: SocketAddr, budget: Duration) -> std::io::Result<Punch> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, None)?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    let local = SocketAddr::from((Ipv4Addr::UNSPECIFIED, PUNCH_PORT));
    socket.bind(&SockAddr::from(local))?;
    match socket.connect(&SockAddr::from(peer)) {
        Ok(()) => {}
        Err(error) if in_progress(&error) => {}
        Err(error) => return Err(error),
    }
    let mut poll = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    };
    let wait_ms = i32::try_from(budget.as_millis()).unwrap_or(i32::MAX);
    let ready = loop {
        // Safety: poll refers to `socket`, which stays open until this call returns.
        let ready = unsafe { libc::poll(&mut poll, 1, wait_ms) };
        if ready < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        break ready;
    };
    if ready < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if ready == 0 {
        return Ok(Punch::SynSent(socket));
    }
    if let Some(error) = socket.take_error()? {
        return Err(error);
    }
    socket.set_nonblocking(false)?;
    Ok(Punch::Connected(TcpStream::from(socket)))
}

fn in_progress(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::WouldBlock || error.raw_os_error() == Some(libc::EINPROGRESS)
}

fn retryable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ConnectionRefused | ErrorKind::AddrInUse
    ) || matches!(
        error.raw_os_error(),
        Some(libc::ECONNREFUSED | libc::EADDRINUSE)
    )
}

fn exchange(stream: &mut TcpStream, role: &str, payload: &str) -> Result<bool> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    if role == "a" {
        writeln!(stream, "{payload}")?;
        stream.flush()?;
    }
    let mut line = String::new();
    match BufReader::new(stream.try_clone()?).read_line(&mut line) {
        Ok(_) if line.trim() == payload => {
            if role != "a" {
                writeln!(stream, "{payload}")?;
                stream.flush()?;
            }
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::TimedOut
                    | ErrorKind::WouldBlock
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

fn rendezvous(role: &str, public_ip: &str) -> Result<SocketAddr> {
    let other = if role == "a" { "b" } else { "a" };
    relay(&json!({
        "action": "put",
        "to": other,
        "data": format!("{public_ip}:{PUNCH_PORT}"),
    }))?;
    let mut remote = None;
    let mut sent_go = false;
    let mut saw_go = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if remote.is_some() && !sent_go {
            relay(&json!({"action": "put", "to": other, "data": "go"}))?;
            sent_go = true;
        }
        let fetched = relay(&json!({"action": "fetch", "role": role}))?;
        if let Some(items) = fetched.as_array() {
            for item in items {
                if let Some(address) = item.as_str().and_then(peer_endpoint) {
                    remote = Some(address);
                }
                if item.as_str() == Some("go") {
                    saw_go = true;
                }
            }
        }
        if sent_go && saw_go {
            if let Some(remote) = remote {
                return Ok(remote);
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    bail!("tcp peer did not reach the barrier")
}

fn peer_endpoint(value: &str) -> Option<SocketAddr> {
    let SocketAddr::V4(address) = value.parse().ok()? else {
        return None;
    };
    let [198, 18, _, _] = address.ip().octets() else {
        return None;
    };
    if address.port() != PUNCH_PORT {
        return None;
    }
    Some(SocketAddr::V4(address))
}

fn relay(request: &Value) -> Result<Value> {
    let mut stream = TcpStream::connect_timeout(&RELAY.parse()?, Duration::from_millis(500))?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    serde_json::to_writer(&mut stream, request)?;
    writeln!(stream)?;
    let mut line = String::new();
    BufReader::new(stream).take(4096).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}

fn wait_again() -> Result<()> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if line.is_empty() {
        return Ok(());
    }
    let request: Value = serde_json::from_str(line.trim())?;
    anyhow::ensure!(request["action"] == "again", "unexpected tcp command");
    Ok(())
}

fn hold() -> Result<()> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(())
}

fn emit(value: &Value) -> Result<()> {
    println!("{value}");
    std::io::stdout().flush()?;
    Ok(())
}
