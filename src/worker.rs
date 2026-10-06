use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn address(value: &Value) -> Result<SocketAddr> {
    let ip = value[0].as_str().context("endpoint IP missing")?;
    let port = value[1].as_u64().context("endpoint port missing")?;
    anyhow::ensure!(port <= u16::MAX as u64, "invalid port");
    Ok(format!("{ip}:{port}").parse()?)
}
fn emit(value: &Value) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}
pub fn endpoint(binds: &str) -> Result<()> {
    let binds: Vec<Value> = serde_json::from_str(binds)?;
    let sockets = binds
        .iter()
        .map(|bind| -> Result<_> {
            let socket = UdpSocket::bind(address(bind)?)?;
            socket.set_nonblocking(true)?;
            Ok(socket)
        })
        .collect::<Result<Vec<_>>>()?;
    let ready = sockets
        .iter()
        .map(|s| {
            let a = s.local_addr().unwrap();
            json!([a.ip().to_string(), a.port()])
        })
        .collect::<Vec<_>>();
    emit(&json!({"ready": ready}))?;
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let response = match request["action"].as_str() {
            Some("send") => {
                let index = request["socket"].as_u64().unwrap_or(0) as usize;
                let socket = sockets.get(index).context("unknown socket index")?;
                socket.send_to(
                    request["data"]
                        .as_str()
                        .context("payload missing")?
                        .as_bytes(),
                    address(&request["to"])?,
                )?;
                json!({"sent": true})
            }
            Some("stun") => stun_exchange(
                sockets
                    .get(request["socket"].as_u64().unwrap_or(0) as usize)
                    .context("unknown socket index")?,
                &request,
            )?,
            Some("receive") => {
                let seconds = request["timeout"].as_f64().unwrap_or(0.5);
                anyhow::ensure!(
                    seconds.is_finite() && seconds > 0.,
                    "invalid receive timeout"
                );
                let deadline = Instant::now() + Duration::from_secs_f64(seconds);
                let mut result = Value::Null;
                let mut buf = [0u8; 65535];
                'receive: while Instant::now() < deadline {
                    for (index, socket) in sockets.iter().enumerate() {
                        match socket.recv_from(&mut buf) {
                            Ok((length, source)) => {
                                let data = String::from_utf8_lossy(&buf[..length]);
                                if request.get("match").is_none()
                                    || request["match"].as_str() == Some(data.as_ref())
                                {
                                    result = json!({"from": [source.ip().to_string(), source.port()], "data": data, "socket": index});
                                    break 'receive;
                                }
                            }
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                            Err(e) => return Err(e.into()),
                        }
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                result
            }
            Some("relay") => match relay_request(&request["request"]) {
                Ok(response) => json!({"ok": true, "response": response}),
                Err(error) => json!({"ok": false, "error": error.to_string()}),
            },
            _ => bail!("unknown endpoint action"),
        };
        emit(&response)?;
    }
    Ok(())
}
fn stun_exchange(socket: &UdpSocket, request: &Value) -> Result<Value> {
    let seconds = request["timeout"].as_f64().unwrap_or(0.5);
    anyhow::ensure!(
        seconds.is_finite() && seconds > 0.,
        "invalid receive timeout"
    );
    let mut transaction = [0u8; 12];
    File::open("/dev/urandom")?.read_exact(&mut transaction)?;
    socket.send_to(
        crate::stun::binding_request(&transaction).as_slice(),
        address(&request["to"])?,
    )?;
    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    let mut buf = [0u8; 1500];
    let mut mapped = Value::Null;
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((length, _)) => {
                if let Some((ip, port)) = crate::stun::mapped_address(&buf[..length], &transaction)
                {
                    mapped = json!([std::net::Ipv4Addr::from(ip).to_string(), port]);
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(json!({"mapped": mapped, "socket": request["socket"].as_u64().unwrap_or(0)}))
}
pub fn stun_server(binds: &str) -> Result<()> {
    let binds: Vec<Value> = serde_json::from_str(binds)?;
    let sockets = Arc::new(Mutex::new(Vec::new()));
    {
        let mut guard = sockets.lock().unwrap();
        for bind in &binds {
            let socket = UdpSocket::bind(address(bind)?)?;
            socket.set_nonblocking(true)?;
            guard.push(socket);
        }
        let ready = guard
            .iter()
            .map(|socket| {
                let bound = socket.local_addr().unwrap();
                json!([bound.ip().to_string(), bound.port()])
            })
            .collect::<Vec<_>>();
        emit(&json!({"ready": ready}))?;
    }
    let inbox = Arc::new(Mutex::new(VecDeque::new()));
    let reader_sockets = Arc::clone(&sockets);
    let reader_inbox = Arc::clone(&inbox);
    thread::spawn(move || serve_binding_requests(reader_sockets, reader_inbox));
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let response = match request["action"].as_str() {
            Some("send") => {
                let index = request["socket"].as_u64().unwrap_or(0) as usize;
                let guard = sockets.lock().unwrap();
                let socket = guard.get(index).context("unknown socket index")?;
                socket.send_to(
                    request["data"]
                        .as_str()
                        .context("payload missing")?
                        .as_bytes(),
                    address(&request["to"])?,
                )?;
                json!({"sent": true})
            }
            Some("receive") => queued_receive(&inbox, &request)?,
            _ => bail!("unknown endpoint action"),
        };
        emit(&response)?;
    }
    Ok(())
}
fn serve_binding_requests(sockets: Arc<Mutex<Vec<UdpSocket>>>, inbox: Arc<Mutex<VecDeque<Value>>>) {
    let mut buf = [0u8; 65535];
    loop {
        let guard = sockets.lock().unwrap();
        for (index, socket) in guard.iter().enumerate() {
            match socket.recv_from(&mut buf) {
                Ok((length, source)) => {
                    if let Some(transaction) = crate::stun::transaction_id(&buf[..length]) {
                        if let SocketAddr::V4(peer) = source {
                            let response = crate::stun::binding_success(
                                &transaction,
                                peer.ip().octets(),
                                peer.port(),
                            );
                            let _ = socket.send_to(&response, source);
                        }
                    } else {
                        inbox.lock().unwrap().push_back(json!({
                            "from": [source.ip().to_string(), source.port()],
                            "data": String::from_utf8_lossy(&buf[..length]),
                            "socket": index
                        }));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => return,
            }
        }
        drop(guard);
        thread::sleep(Duration::from_millis(2));
    }
}
fn queued_receive(inbox: &Mutex<VecDeque<Value>>, request: &Value) -> Result<Value> {
    let seconds = request["timeout"].as_f64().unwrap_or(0.5);
    anyhow::ensure!(
        seconds.is_finite() && seconds > 0.,
        "invalid receive timeout"
    );
    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < deadline {
        let mut queued = inbox.lock().unwrap();
        if let Some(position) = queued.iter().position(|packet| {
            request.get("match").is_none() || request["match"].as_str() == packet["data"].as_str()
        }) {
            return Ok(queued.remove(position).unwrap_or(Value::Null));
        }
        drop(queued);
        thread::sleep(Duration::from_millis(2));
    }
    Ok(Value::Null)
}
fn relay_request(request: &Value) -> Result<Value> {
    let mut stream =
        TcpStream::connect_timeout(&"198.18.0.1:9100".parse()?, Duration::from_millis(500))?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    serde_json::to_writer(&mut stream, request)?;
    writeln!(stream)?;
    let mut line = String::new();
    BufReader::new(stream).take(4096).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}
pub fn relay() -> Result<()> {
    // No authentication or durability: this server is only a local test fixture.
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None)?;
    socket.set_reuse_address(true)?;
    socket.bind(&"198.18.0.1:9100".parse::<SocketAddr>()?.into())?;
    socket.listen(16)?;
    let listener: TcpListener = socket.into();
    let mut inbox: BTreeMap<String, Vec<Value>> = ["a", "b"]
        .into_iter()
        .map(|side| (side.into(), Vec::new()))
        .collect();
    emit(&json!({"ready": true}))?;
    for connection in listener.incoming() {
        let mut stream = connection?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let result = (|| -> Result<()> {
            let mut line = String::new();
            BufReader::new(stream.try_clone()?)
                .take(4096)
                .read_line(&mut line)?;
            let request: Value = serde_json::from_str(&line)?;
            let response = match request["action"].as_str() {
                Some("put") => {
                    let recipient = request["to"].as_str().context("recipient missing")?;
                    inbox
                        .get_mut(recipient)
                        .context("unknown recipient")?
                        .push(request["data"].clone());
                    json!({"accepted": true})
                }
                Some("fetch") => {
                    let role = request["role"].as_str().context("role missing")?;
                    json!(std::mem::take(inbox.get_mut(role).context("unknown role")?))
                }
                _ => json!({"error": "unknown action"}),
            };
            serde_json::to_writer(&mut stream, &response)?;
            writeln!(stream)?;
            Ok(())
        })();
        if result.is_err() {
            continue;
        }
    }
    Ok(())
}
