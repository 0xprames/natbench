//! WebRTC data channel on the UDP socket used for STUN and hole punching.
//!
//! Role `a` is the ICE controlling agent. Role `b` is controlled. Both open one
//! negotiated data channel, exchange a payload, wait for the controller, then
//! exchange another after the TCP relay is gone.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};
use str0m::channel::{ChannelConfig, ChannelId};
use str0m::crypto::Fingerprint;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceCreds, Input, Output, Rtc};

const RELAY: &str = "198.18.0.1:9100";
const STUN_SERVER: &str = "198.18.0.1:9000";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Before,
    Wait,
    After,
    Hold,
}

pub fn run(role: &str) -> Result<()> {
    anyhow::ensure!(role == "a" || role == "b", "webrtc role must be a or b");
    let socket = UdpSocket::bind("0.0.0.0:10000")?;
    let mapped = learn_mapping(&socket)?;
    emit(&json!({"ready": true, "mapped": [mapped.ip().to_string(), mapped.port()]}))?;
    let peer = peer_mapping(role, mapped)?;
    punch(&socket, peer)?;
    // Both sides finish punching before ICE starts, then drop datagrams still
    // sitting on the socket so they are not parsed as STUN or DTLS.
    wait_for_peer(role, "punched")?;
    thread::sleep(Duration::from_millis(50));
    drain(&socket)?;
    session(role, &socket, mapped, peer)
}

fn session(role: &str, socket: &UdpSocket, mapped: SocketAddr, peer: SocketAddr) -> Result<()> {
    let mut rtc = Rtc::builder().build(Instant::now());
    let local = Candidate::host(mapped, "udp")?;
    anyhow::ensure!(
        rtc.add_local_candidate(local).is_some(),
        "local candidate was rejected"
    );
    rtc.add_remote_candidate(Candidate::host(peer, "udp")?);
    publish_description(role, &mut rtc)?;
    let remote = wait_description(role)?;
    let active = role == "a";
    {
        let fingerprint = remote
            .fingerprint
            .parse::<Fingerprint>()
            .map_err(|error| anyhow::anyhow!(error))?;
        let mut api = rtc.direct_api();
        api.set_remote_fingerprint(fingerprint);
        api.set_remote_ice_credentials(IceCreds {
            ufrag: remote.ufrag,
            pass: remote.pass,
        });
        api.set_ice_controlling(active);
        api.start_dtls(active)?;
        api.start_sctp(active);
        api.create_data_channel(ChannelConfig {
            label: "natbench".into(),
            negotiated: Some(1),
            ..ChannelConfig::default()
        });
    }
    drive(&mut rtc, socket, mapped, role)
}

fn drive(rtc: &mut Rtc, socket: &UdpSocket, mapped: SocketAddr, role: &str) -> Result<()> {
    let stdin = stdin_lines();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut phase = Phase::Before;
    let mut channel = None;
    let mut sent = false;
    let mut got = false;
    let mut buf = [0u8; 2048];
    loop {
        if !rtc.is_alive() {
            if phase == Phase::Hold {
                return Ok(());
            }
            bail!("webrtc connection closed");
        }
        let mut incoming = Vec::new();
        loop {
            match rtc.poll_output()? {
                Output::Timeout(_) => break,
                Output::Transmit(transmit) => {
                    socket.send_to(&transmit.contents, transmit.destination)?;
                }
                Output::Event(Event::ChannelOpen(id, _)) => channel = Some(id),
                Output::Event(Event::ChannelData(data)) => incoming.push(data.data),
                Output::Event(_) => {}
            }
        }
        step(
            rtc, role, &mut phase, channel, &mut sent, &mut got, &incoming,
        )?;
        match phase {
            Phase::Wait => {
                if let Some(line) = recv_line(&stdin)? {
                    let request: Value = serde_json::from_str(line.trim())?;
                    anyhow::ensure!(request["action"] == "again", "unexpected webrtc command");
                    sent = false;
                    got = false;
                    phase = Phase::After;
                } else if Instant::now() > deadline {
                    bail!("webrtc data channel did not complete");
                }
            }
            Phase::Hold => {
                if Instant::now() > deadline {
                    return Ok(());
                }
            }
            Phase::Before | Phase::After if Instant::now() > deadline => {
                bail!("webrtc data channel did not complete")
            }
            Phase::Before | Phase::After => {}
        }
        let pause = Duration::from_millis(20);
        socket.set_read_timeout(Some(pause))?;
        match socket.recv_from(&mut buf) {
            Ok((length, source)) => {
                if let Ok(received) = Receive::new(Protocol::Udp, source, mapped, &buf[..length]) {
                    rtc.handle_input(Input::Receive(Instant::now(), received))?;
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                rtc.handle_input(Input::Timeout(Instant::now()))?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn step(
    rtc: &mut Rtc,
    role: &str,
    phase: &mut Phase,
    channel: Option<ChannelId>,
    sent: &mut bool,
    got: &mut bool,
    incoming: &[Vec<u8>],
) -> Result<()> {
    let payload: &[u8] = match *phase {
        Phase::Before => b"webrtc-before",
        Phase::After => b"webrtc-after",
        Phase::Wait | Phase::Hold => return Ok(()),
    };
    for message in incoming {
        anyhow::ensure!(message == payload, "unexpected data channel payload");
        *got = true;
    }
    if let Some(id) = channel {
        if !*sent && (role == "a" || *got) {
            if let Some(mut open) = rtc.channel(id) {
                if open.write(false, payload)? {
                    *sent = true;
                }
            }
        }
    }
    if *sent && *got {
        let field = if *phase == Phase::Before {
            "data_before"
        } else {
            "data_after"
        };
        emit(&json!({ field: true }))?;
        *sent = false;
        *got = false;
        *phase = if *phase == Phase::Before {
            Phase::Wait
        } else {
            Phase::Hold
        };
    }
    Ok(())
}

fn recv_line(stdin: &Receiver<String>) -> Result<Option<String>> {
    match stdin.try_recv() {
        Ok(line) => Ok(Some(line)),
        Err(TryRecvError::Empty) => Ok(None),
        Err(TryRecvError::Disconnected) => bail!("stdin closed"),
    }
}

fn stdin_lines() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

struct RemoteDescription {
    ufrag: String,
    pass: String,
    fingerprint: String,
}

fn publish_description(role: &str, rtc: &mut Rtc) -> Result<()> {
    let creds = rtc.direct_api().local_ice_credentials();
    let fingerprint = rtc.direct_api().local_dtls_fingerprint().to_string();
    let other = if role == "a" { "b" } else { "a" };
    relay(&json!({
        "action": "put",
        "to": other,
        "data": {"ufrag": creds.ufrag, "pass": creds.pass, "fingerprint": fingerprint},
    }))?;
    Ok(())
}

fn wait_description(role: &str) -> Result<RemoteDescription> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let fetched = relay(&json!({"action": "fetch", "role": role}))?;
        if let Some(item) = fetched.as_array().and_then(|items| {
            items
                .iter()
                .rev()
                .find(|value| value.get("ufrag").and_then(Value::as_str).is_some())
        }) {
            return Ok(RemoteDescription {
                ufrag: item["ufrag"].as_str().context("ufrag")?.to_owned(),
                pass: item["pass"].as_str().context("pass")?.to_owned(),
                fingerprint: item["fingerprint"]
                    .as_str()
                    .context("fingerprint")?
                    .to_owned(),
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
    bail!("peer description did not arrive")
}

fn learn_mapping(socket: &UdpSocket) -> Result<SocketAddr> {
    socket.set_read_timeout(Some(Duration::from_millis(50)))?;
    let mut transaction = [0u8; 12];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut transaction)?;
    let request = crate::stun::binding_request(&transaction);
    let server: SocketAddr = STUN_SERVER.parse()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buf = [0u8; 1500];
    while Instant::now() < deadline {
        socket.send_to(&request, server)?;
        match socket.recv_from(&mut buf) {
            Ok((length, _)) => {
                if let Some((ip, port)) = crate::stun::mapped_address(&buf[..length], &transaction)
                {
                    socket.set_read_timeout(None)?;
                    return Ok(SocketAddr::from((Ipv4Addr::from(ip), port)));
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
    }
    bail!("STUN Binding response did not arrive")
}

fn punch(socket: &UdpSocket, peer: SocketAddr) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        socket.send_to(b"punch", peer)?;
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn drain(socket: &UdpSocket) -> Result<()> {
    socket.set_nonblocking(true)?;
    let mut buf = [0u8; 1500];
    loop {
        match socket.recv_from(&mut buf) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.into()),
        }
    }
    socket.set_nonblocking(false)?;
    Ok(())
}

fn wait_for_peer(role: &str, token: &str) -> Result<()> {
    let other = if role == "a" { "b" } else { "a" };
    relay(&json!({"action": "put", "to": other, "data": token}))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let fetched = relay(&json!({"action": "fetch", "role": role}))?;
        if fetched
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(token)))
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    bail!("peer did not finish punching")
}

fn peer_mapping(role: &str, mapped: SocketAddr) -> Result<SocketAddr> {
    let other = if role == "a" { "b" } else { "a" };
    relay(&json!({
        "action": "put",
        "to": other,
        "data": [mapped.ip().to_string(), mapped.port()],
    }))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let fetched = relay(&json!({"action": "fetch", "role": role}))?;
        if let Some(address) = fetched
            .as_array()
            .and_then(|items| items.iter().rev().find_map(socket_addr))
        {
            return Ok(address);
        }
        thread::sleep(Duration::from_millis(20));
    }
    bail!("peer mapping did not arrive")
}

fn socket_addr(value: &Value) -> Option<SocketAddr> {
    let ip: Ipv4Addr = value.get(0)?.as_str()?.parse().ok()?;
    let port = u16::try_from(value.get(1)?.as_u64()?).ok()?;
    Some(SocketAddr::from((ip, port)))
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

fn emit(value: &Value) -> Result<()> {
    println!("{value}");
    std::io::stdout().flush()?;
    Ok(())
}
