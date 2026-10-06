//! Userspace IPv4 UDP NAT for packets queued on NFQUEUE 42.
//!
//! Mapping is endpoint-independent and the original source port is kept when it
//! is free. Filtering is endpoint-independent: once a public port exists, any
//! remote address may send to it. This is intentionally not Linux masquerade,
//! whose filtering is address-and-port-dependent. The command matches the
//! `nat --lan-interface --wan-interface` invocation used by an external translator.

use anyhow::{bail, Context, Result};
use std::{collections::HashMap, io::ErrorKind, process::Command};

const QUEUE: u16 = 42;
const NFNL_SUBSYS_QUEUE: u16 = 3;
const NFQNL_MSG_PACKET: u16 = 0;
const NFQNL_MSG_VERDICT: u16 = 1;
const NFQNL_MSG_CONFIG: u16 = 2;
const NFQA_PACKET_HDR: u16 = 1;
const NFQA_VERDICT_HDR: u16 = 2;
const NFQA_IFINDEX_INDEV: u16 = 5;
const NFQA_PAYLOAD: u16 = 10;
const NFQA_CFG_CMD: u16 = 1;
const NFQA_CFG_PARAMS: u16 = 2;
const NFQA_CFG_QUEUE_MAXLEN: u16 = 3;
const NFQNL_CFG_CMD_BIND: u8 = 1;
const NFQNL_CFG_CMD_PF_BIND: u8 = 3;
const NFQNL_CFG_CMD_PF_UNBIND: u8 = 4;
const NFQNL_COPY_PACKET: u8 = 2;
const NF_DROP: u32 = 0;
const NF_ACCEPT: u32 = 1;
const NLMSG_ERROR: u16 = 0x2;
const NLM_F_REQUEST: u16 = 0x01;
const NLM_F_ACK: u16 = 0x04;

struct Table {
    by_lan: HashMap<([u8; 4], u16), u16>,
    by_wan: HashMap<u16, ([u8; 4], u16)>,
}
impl Table {
    fn new() -> Self {
        Self {
            by_lan: HashMap::new(),
            by_wan: HashMap::new(),
        }
    }
    fn assign(&mut self, lan_ip: [u8; 4], lan_port: u16) -> Option<u16> {
        if let Some(port) = self.by_lan.get(&(lan_ip, lan_port)) {
            return Some(*port);
        }
        let port = if lan_port >= 1024 && !self.by_wan.contains_key(&lan_port) {
            lan_port
        } else {
            (1024..=u16::MAX).find(|port| !self.by_wan.contains_key(port))?
        };
        self.by_lan.insert((lan_ip, lan_port), port);
        self.by_wan.insert(port, (lan_ip, lan_port));
        Some(port)
    }
}

/// Rewrite one queued IPv4 UDP packet. `from_lan` selects outbound translation.
/// Returns whether the packet should be accepted.
fn rewrite(packet: &mut [u8], from_lan: bool, wan_ip: [u8; 4], table: &mut Table) -> bool {
    if packet.first().copied().unwrap_or(0) >> 4 != 4 {
        return true;
    }
    let header = ((packet[0] & 0x0f) as usize) * 4;
    if header < 20 || packet.len() < header + 8 || packet[9] != 17 {
        return true;
    }
    let fragment = u16::from_be_bytes([packet[6], packet[7]]);
    if fragment & 0x3fff != 0 {
        return false;
    }
    let udp = header;
    if from_lan {
        let lan_ip = packet[12..16].try_into().unwrap();
        let lan_port = u16::from_be_bytes([packet[udp], packet[udp + 1]]);
        let Some(wan_port) = table.assign(lan_ip, lan_port) else {
            return false;
        };
        packet[12..16].copy_from_slice(&wan_ip);
        packet[udp..udp + 2].copy_from_slice(&wan_port.to_be_bytes());
    } else {
        let wan_port = u16::from_be_bytes([packet[udp + 2], packet[udp + 3]]);
        let Some((lan_ip, lan_port)) = table.by_wan.get(&wan_port).copied() else {
            return false;
        };
        packet[16..20].copy_from_slice(&lan_ip);
        packet[udp + 2..udp + 4].copy_from_slice(&lan_port.to_be_bytes());
    }
    packet[udp + 6] = 0;
    packet[udp + 7] = 0;
    write_ipv4_checksum(packet);
    true
}

fn write_ipv4_checksum(packet: &mut [u8]) {
    let header = ((packet[0] & 0x0f) as usize) * 4;
    packet[10] = 0;
    packet[11] = 0;
    let mut sum = 0u32;
    for offset in (0..header).step_by(2) {
        sum += u16::from_be_bytes([packet[offset], packet[offset + 1]]) as u32;
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    let checksum = !(sum as u16);
    packet[10..12].copy_from_slice(&checksum.to_be_bytes());
}

struct Netlink(i32);
impl Drop for Netlink {
    fn drop(&mut self) {
        // SAFETY: the file descriptor was returned by socket and is still open.
        unsafe { libc::close(self.0) };
    }
}

pub fn run(lan: &str, wan: &str) -> Result<()> {
    let lan_index = ifindex(lan)?;
    let wan_index = ifindex(wan)?;
    let wan_ip = ipv4(wan)?;
    let socket = Netlink(open_socket()?);
    configure(&socket)?;
    let mut table = Table::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let received = unsafe { libc::recv(socket.0, buffer.as_mut_ptr().cast(), buffer.len(), 0) };
        if received < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("read NFQUEUE");
        }
        dispatch(
            &socket,
            &buffer[..received as usize],
            lan_index,
            wan_index,
            wan_ip,
            &mut table,
        )?;
    }
}

fn ifindex(name: &str) -> Result<u32> {
    let output = Command::new("ip")
        .args(["-o", "link", "show", "dev", name])
        .output()
        .with_context(|| format!("look up interface {name}"))?;
    anyhow::ensure!(output.status.success(), "interface {name} is missing");
    let text = String::from_utf8(output.stdout)?;
    text.split(':')
        .next()
        .unwrap_or("")
        .trim()
        .parse()
        .with_context(|| format!("interface index for {name}"))
}

fn ipv4(name: &str) -> Result<[u8; 4]> {
    let output = Command::new("ip")
        .args(["-4", "-o", "addr", "show", "dev", name])
        .output()
        .with_context(|| format!("look up address of {name}"))?;
    anyhow::ensure!(
        output.status.success(),
        "interface {name} has no IPv4 address"
    );
    let text = String::from_utf8(output.stdout)?;
    let address = text
        .split_whitespace()
        .skip_while(|word| *word != "inet")
        .nth(1)
        .context("inet address missing")?;
    let mut parts = address.split('/').next().unwrap_or("").split('.');
    let bytes = [
        parts.next().context("address")?.parse()?,
        parts.next().context("address")?.parse()?,
        parts.next().context("address")?.parse()?,
        parts.next().context("address")?.parse()?,
    ];
    Ok(bytes)
}

fn open_socket() -> Result<i32> {
    // SAFETY: socket arguments are constants and the descriptor is checked below.
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            libc::NETLINK_NETFILTER,
        )
    };
    if fd < 0 {
        bail!("open NFQUEUE socket: {}", std::io::Error::last_os_error());
    }
    // SAFETY: sockaddr_nl is a plain C struct and every field is written before bind.
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    // SAFETY: address points at a sockaddr_nl and fd is the socket just opened.
    let bound = unsafe {
        libc::bind(
            fd,
            (&address as *const libc::sockaddr_nl).cast(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if bound < 0 {
        let error = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        bail!("bind NFQUEUE socket: {error}");
    }
    Ok(fd)
}

/// Verify NFQUEUE configuration, called only inside a disposable router namespace.
pub fn probe(namespace: &str) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    anyhow::ensure!(
        namespace.starts_with("nb")
            && namespace
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "NFQUEUE probe requires a natbench namespace"
    );
    let expected = std::fs::metadata(format!("/var/run/netns/{namespace}"))?;
    let current = std::fs::metadata("/proc/self/ns/net")?;
    anyhow::ensure!(
        expected.ino() == current.ino() && expected.dev() == current.dev(),
        "NFQUEUE probe must run inside the specified namespace"
    );
    let socket = Netlink(open_socket()?);
    let timeout = libc::timeval {
        tv_sec: 2,
        tv_usec: 0,
    };
    // SAFETY: the descriptor is live and timeout has the expected socket option layout.
    let result = unsafe {
        libc::setsockopt(
            socket.0,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&timeout as *const libc::timeval).cast(),
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
    anyhow::ensure!(
        result == 0,
        "set NFQUEUE probe timeout: {}",
        std::io::Error::last_os_error()
    );
    configure(&socket)
}

fn configure(socket: &Netlink) -> Result<()> {
    // Nothing is bound on the first start, so an unbind failure is expected.
    let _ = transact(
        socket,
        config_command(0, NFQNL_CFG_CMD_PF_UNBIND, libc::AF_INET as u16),
    );
    transact(
        socket,
        config_command(0, NFQNL_CFG_CMD_PF_BIND, libc::AF_INET as u16),
    )?;
    transact(socket, config_command(QUEUE, NFQNL_CFG_CMD_BIND, 0))?;
    let mut params = (65535u32).to_be_bytes().to_vec();
    params.push(NFQNL_COPY_PACKET);
    transact(socket, config_attr(QUEUE, NFQA_CFG_PARAMS, &params))?;
    transact(
        socket,
        config_attr(QUEUE, NFQA_CFG_QUEUE_MAXLEN, &(1024u32).to_be_bytes()),
    )?;
    Ok(())
}

fn config_command(queue: u16, command: u8, family: u16) -> Vec<u8> {
    let body = [command, 0, (family >> 8) as u8, family as u8];
    config_attr(queue, NFQA_CFG_CMD, &body)
}

fn config_attr(queue: u16, attr: u16, value: &[u8]) -> Vec<u8> {
    message(
        NFQNL_MSG_CONFIG,
        queue,
        NLM_F_REQUEST | NLM_F_ACK,
        &attribute(attr, value),
    )
}

fn message(kind: u16, queue: u16, flags: u16, attributes: &[u8]) -> Vec<u8> {
    let mut body = vec![libc::AF_UNSPEC as u8, 0, (queue >> 8) as u8, queue as u8];
    body.extend_from_slice(attributes);
    let mut packet = Vec::with_capacity(16 + body.len());
    let length = (16 + body.len()) as u32;
    packet.extend_from_slice(&length.to_ne_bytes());
    packet.extend_from_slice(&(NFNL_SUBSYS_QUEUE << 8 | kind).to_ne_bytes());
    packet.extend_from_slice(&flags.to_ne_bytes());
    packet.extend_from_slice(&0u32.to_ne_bytes());
    packet.extend_from_slice(&0u32.to_ne_bytes());
    packet.extend_from_slice(&body);
    packet
}

fn attribute(kind: u16, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + value.len() + 3);
    out.extend_from_slice(&((4 + value.len()) as u16).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(value);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

fn transact(socket: &Netlink, packet: Vec<u8>) -> Result<()> {
    let sent = unsafe { libc::send(socket.0, packet.as_ptr().cast(), packet.len(), 0) };
    if sent < 0 {
        bail!("NFQUEUE config: {}", std::io::Error::last_os_error());
    }
    let mut buffer = [0u8; 4096];
    let received = unsafe { libc::recv(socket.0, buffer.as_mut_ptr().cast(), buffer.len(), 0) };
    if received < 0 {
        bail!("NFQUEUE config reply: {}", std::io::Error::last_os_error());
    }
    let buffer = &buffer[..received as usize];
    if buffer.len() < 20 {
        bail!("short NFQUEUE config reply");
    }
    let kind = u16::from_ne_bytes([buffer[4], buffer[5]]);
    if kind != NLMSG_ERROR {
        return Ok(());
    }
    let error = i32::from_ne_bytes([buffer[16], buffer[17], buffer[18], buffer[19]]);
    if error != 0 {
        bail!(
            "NFQUEUE config: {}",
            std::io::Error::from_raw_os_error(-error)
        );
    }
    Ok(())
}

fn dispatch(
    socket: &Netlink,
    mut buffer: &[u8],
    lan_index: u32,
    wan_index: u32,
    wan_ip: [u8; 4],
    table: &mut Table,
) -> Result<()> {
    while buffer.len() >= 16 {
        let length = u32::from_ne_bytes(buffer[0..4].try_into().unwrap()) as usize;
        if length < 16 || length > buffer.len() {
            break;
        }
        let kind = u16::from_ne_bytes([buffer[4], buffer[5]]);
        if kind == (NFNL_SUBSYS_QUEUE << 8 | NFQNL_MSG_PACKET) && length >= 20 {
            verdict(
                socket,
                &buffer[20..length],
                lan_index,
                wan_index,
                wan_ip,
                table,
            )?;
        }
        let step = (length + 3) & !3;
        if step > buffer.len() {
            break;
        }
        buffer = &buffer[step..];
    }
    Ok(())
}

fn verdict(
    socket: &Netlink,
    attributes: &[u8],
    lan_index: u32,
    wan_index: u32,
    wan_ip: [u8; 4],
    table: &mut Table,
) -> Result<()> {
    let mut rest = attributes;
    let mut packet_id = None;
    let mut indev = None;
    let mut payload = None;
    while let Some((kind, value, next)) = next_attr(rest) {
        match kind {
            NFQA_PACKET_HDR if value.len() >= 4 => packet_id = Some(value[..4].to_vec()),
            NFQA_IFINDEX_INDEV if value.len() >= 4 => {
                indev = Some(u32::from_be_bytes(value[..4].try_into().unwrap()));
            }
            NFQA_PAYLOAD => payload = Some(value.to_vec()),
            _ => {}
        }
        rest = next;
    }
    let Some(packet_id) = packet_id else {
        return Ok(());
    };
    let mut accept = true;
    let mut modified = None;
    if let Some(mut payload) = payload {
        let from_lan = indev == Some(lan_index);
        let from_wan = indev == Some(wan_index);
        if from_lan || from_wan {
            accept = rewrite(&mut payload, from_lan, wan_ip, table);
            if accept {
                modified = Some(payload);
            }
        }
    }
    let verdict = if accept { NF_ACCEPT } else { NF_DROP };
    let mut header = verdict.to_be_bytes().to_vec();
    header.extend_from_slice(&packet_id);
    let mut attributes = attribute(NFQA_VERDICT_HDR, &header);
    if let Some(payload) = modified.as_deref() {
        attributes.extend_from_slice(&attribute(NFQA_PAYLOAD, payload));
    }
    let packet = message(NFQNL_MSG_VERDICT, QUEUE, NLM_F_REQUEST, &attributes);
    let sent = unsafe { libc::send(socket.0, packet.as_ptr().cast(), packet.len(), 0) };
    if sent < 0 {
        bail!("NFQUEUE verdict: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

fn next_attr(buffer: &[u8]) -> Option<(u16, &[u8], &[u8])> {
    if buffer.len() < 4 {
        return None;
    }
    let length = u16::from_ne_bytes([buffer[0], buffer[1]]) as usize;
    let kind = u16::from_ne_bytes([buffer[2], buffer[3]]) & 0x3fff;
    if length < 4 || length > buffer.len() {
        return None;
    }
    let aligned = (length + 3) & !3;
    let rest = if aligned > buffer.len() {
        &[]
    } else {
        &buffer[aligned..]
    };
    Some((kind, &buffer[4..length], rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(src: [u8; 4], sport: u16, dst: [u8; 4], dport: u16) -> Vec<u8> {
        let mut packet = vec![0u8; 28];
        packet[0] = 0x45;
        packet[2..4].copy_from_slice(&28u16.to_be_bytes());
        packet[8] = 64;
        packet[9] = 17;
        packet[12..16].copy_from_slice(&src);
        packet[16..20].copy_from_slice(&dst);
        packet[20..22].copy_from_slice(&sport.to_be_bytes());
        packet[22..24].copy_from_slice(&dport.to_be_bytes());
        packet[24..26].copy_from_slice(&8u16.to_be_bytes());
        write_ipv4_checksum(&mut packet);
        packet
    }

    #[test]
    fn preserves_the_port_and_accepts_any_return_source() {
        let mut table = Table::new();
        let wan = [198, 18, 0, 10];
        let mut outbound = packet([10, 1, 0, 2], 10000, [198, 18, 0, 1], 9000);
        assert!(rewrite(&mut outbound, true, wan, &mut table));
        assert_eq!(&outbound[12..16], &wan);
        assert_eq!(u16::from_be_bytes([outbound[20], outbound[21]]), 10000);
        let mut other = packet([198, 18, 0, 2], 9000, wan, 10000);
        assert!(rewrite(&mut other, false, wan, &mut table));
        assert_eq!(&other[16..20], &[10, 1, 0, 2]);
        assert_eq!(u16::from_be_bytes([other[22], other[23]]), 10000);
        let mut second = packet([10, 1, 0, 2], 10000, [198, 18, 0, 2], 9000);
        assert!(rewrite(&mut second, true, wan, &mut table));
        assert_eq!(u16::from_be_bytes([second[20], second[21]]), 10000);
    }
}
