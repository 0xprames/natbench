//! Standalone UDP application: no natbench library or test protocol dependency.
use std::{
    env,
    io::{self, ErrorKind},
    net::{SocketAddr, UdpSocket},
    process, thread,
    time::{Duration, Instant},
};

fn server(address: SocketAddr) -> io::Result<()> {
    let socket = UdpSocket::bind(address)?;
    println!("READY {}", socket.local_addr()?);
    let mut buffer = [0u8; 2048];
    loop {
        let (length, peer) = socket.recv_from(&mut buffer)?;
        socket.send_to(&buffer[..length], peer)?;
    }
}

fn exchange(socket: &UdpSocket, sequence: u64) -> io::Result<()> {
    let payload = format!("echo-{sequence}");
    socket.send(payload.as_bytes())?;
    let deadline = Instant::now() + Duration::from_millis(150);
    let mut buffer = [0u8; 2048];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(ErrorKind::TimedOut, "no matching reply"));
        }
        socket.set_read_timeout(Some(remaining))?;
        let length = socket.recv(&mut buffer)?;
        if buffer[..length] == *payload.as_bytes() {
            return Ok(());
        }
        // Ignore delayed replies to earlier requests; each success needs fresh data.
    }
}

fn client(address: SocketAddr) -> io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect(address)?;
    let local = socket.local_addr()?;
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut connected = false;
    let mut lost = false;
    let mut recovered_messages = 0;
    let mut sequence = 0;
    while Instant::now() < deadline {
        sequence += 1;
        match exchange(&socket, sequence) {
            Ok(()) => {
                if !connected {
                    println!("CONNECTED local={local}");
                    connected = true;
                }
                if lost {
                    recovered_messages += 1;
                    if recovered_messages == 1 {
                        println!("RECOVERED local={local}");
                    }
                    if recovered_messages == 3 {
                        println!(
                            "PASS recovered application data on the original socket local={local}"
                        );
                        return Ok(());
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::ConnectionRefused
                ) =>
            {
                if connected && !lost {
                    println!("DISCONNECTED local={local}");
                    lost = true;
                }
                recovered_messages = 0;
            }
            Err(error) => return Err(error),
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(io::Error::new(
        ErrorKind::TimedOut,
        "expected connection, outage and recovery within 12 seconds",
    ))
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if !(2..=3).contains(&args.len()) {
        return Err("usage: udp-recovery server|client [IPv4-address:port]".into());
    }
    let address: SocketAddr = args
        .get(2)
        .map(String::as_str)
        .unwrap_or("198.18.0.1:9999")
        .parse()?;
    if !address.is_ipv4() || address.port() == 0 {
        return Err("requires an IPv4 address with a nonzero port".into());
    }
    match args[1].as_str() {
        "server" => server(address)?,
        "client" => client(address)?,
        _ => return Err("mode must be server or client".into()),
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}
