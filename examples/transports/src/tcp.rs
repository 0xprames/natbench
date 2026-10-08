//! Plain Linux TCP baseline: one connection, sequential length-prefixed exchanges.
use anyhow::{ensure, Context, Result};
use natbench_transport_protocol::{Request, FRAME_HEADER};
use serde_json::json;
use socket2::SockRef;
use std::{collections::BTreeMap, fs};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpSocket, TcpStream},
};

pub fn settings() -> Result<(String, BTreeMap<String, String>)> {
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")?
        .trim()
        .to_owned();
    ensure!(!kernel.is_empty(), "missing Linux kernel release");
    let socket = TcpSocket::new_v4()?;
    let congestion = congestion(&SockRef::from(&socket))?;
    Ok((
        kernel,
        BTreeMap::from([
            ("transport_engine".into(), "Linux kernel TCP".into()),
            ("authentication".into(), "none".into()),
            ("encryption".into(), "none; plain TCP".into()),
            ("tcp_nodelay".into(), "true; both endpoints".into()),
            ("tcp_congestion_control".into(), congestion),
            (
                "transport_settings".into(),
                "kernel defaults except TCP_NODELAY; buffers not overridden".into(),
            ),
            (
                "stream_strategy".into(),
                "one fresh connection; sequential exchanges".into(),
            ),
            (
                "application_framing".into(),
                "4-byte big-endian length prefix per request and response".into(),
            ),
        ]),
    ))
}

pub fn socket() -> Result<TcpSocket> {
    let socket = TcpSocket::new_v4()?;
    socket.set_nodelay(true)?;
    ensure!(socket.nodelay()?, "TCP_NODELAY was not applied");
    Ok(socket)
}

fn congestion(socket: &SockRef<'_>) -> Result<String> {
    // Linux returns a fixed-width NUL-padded TCP_CONGESTION name.
    let name = String::from_utf8(socket.tcp_congestion()?)?
        .trim_end_matches('\0')
        .to_owned();
    ensure!(
        !name.is_empty() && !name.contains('\0'),
        "invalid TCP congestion control name"
    );
    Ok(name)
}

pub fn snapshot(socket: &SockRef<'_>) -> Result<serde_json::Value> {
    ensure!(socket.tcp_nodelay()?, "TCP_NODELAY changed");
    Ok(json!({
        "source": "getsockopt and socket addresses",
        "local": socket.local_addr()?.as_socket().context("non-IP socket")?.to_string(),
        "remote": socket.peer_addr().ok().and_then(|a|a.as_socket()).map(|a|a.to_string()),
        "tcp_nodelay": socket.tcp_nodelay()?,
        "tcp_congestion_control": congestion(socket)?,
        "send_buffer_bytes": socket.send_buffer_size()?,
        "receive_buffer_bytes": socket.recv_buffer_size()?,
        "keepalive": socket.keepalive()?,
        "buffer_scope": "snapshot; kernel autotuning can change buffer sizes",
    }))
}

pub async fn read_frame(reader: &mut (impl AsyncRead + Unpin), limit: usize) -> Result<Vec<u8>> {
    let length = reader.read_u32().await.context("read TCP frame length")? as usize;
    // Check before allocating or waiting for a peer-controlled payload.
    ensure!(
        length > 0 && length <= limit,
        "TCP frame length {length} outside 1..={limit}"
    );
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .await
        .context("read TCP frame body")?;
    Ok(bytes)
}

pub async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> Result<()> {
    let length = u32::try_from(bytes.len()).context("TCP frame exceeds u32 length")?;
    ensure!(length > 0, "empty TCP frame");
    // One buffer avoids separate tiny prefix and body writes. Framing overhead
    // remains inside exchange timing and outside the verified-byte numerator.
    let mut framed = Vec::with_capacity(bytes.len() + 4);
    framed.extend_from_slice(&length.to_be_bytes());
    framed.extend_from_slice(bytes);
    writer.write_all(&framed).await?;
    Ok(())
}

pub async fn exchange(stream: &mut TcpStream, bytes: &[u8], limit: usize) -> Result<Vec<u8>> {
    write_frame(stream, bytes).await?;
    read_frame(stream, limit).await
}

pub async fn serve_frames<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    request: &Request,
    mut on_verified: impl FnMut(&S) -> Result<()>,
) -> Result<()> {
    let limit = request
        .workload
        .bulk_bytes
        .max(request.workload.payload_bytes) as usize
        + FRAME_HEADER;
    for sequence in 0..request.workload.exchanges() {
        let bytes = read_frame(stream, limit).await?;
        let response = natbench_transport_protocol::response(request, sequence, &bytes)?;
        if sequence + 1 == request.workload.exchanges() {
            // Retain receiver evidence before the final ACK: the controller may
            // stop the server immediately after client completion.
            on_verified(stream)?;
        }
        write_frame(stream, &response).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn kernel_options_are_read_back_without_nul_padding() {
        let (kernel, settings) = settings().unwrap();
        assert!(!kernel.is_empty());
        let socket = socket().unwrap();
        let name = congestion(&SockRef::from(&socket)).unwrap();
        assert!(!name.is_empty() && !name.contains('\0'));
        assert_eq!(name, settings["tcp_congestion_control"]);
        assert!(socket.nodelay().unwrap());
    }

    #[tokio::test]
    async fn rejects_unbounded_empty_and_truncated_frames() {
        for length in [0u32, 65, u32::MAX] {
            let bytes = length.to_be_bytes();
            let error = read_frame(&mut bytes.as_slice(), 64).await.unwrap_err();
            assert!(error.to_string().contains("outside"));
        }
        let bytes = [0, 0, 0, 4, 1, 2];
        assert!(read_frame(&mut bytes.as_slice(), 64).await.is_err());
        assert!(read_frame(&mut [0, 0].as_slice(), 64).await.is_err());
    }

    #[tokio::test]
    async fn fragmented_frames_preserve_boundaries() {
        let (mut writer, mut reader) = tokio::io::duplex(1);
        let task = tokio::spawn(async move {
            write_frame(&mut writer, b"first").await.unwrap();
            write_frame(&mut writer, b"second").await.unwrap();
        });
        assert_eq!(read_frame(&mut reader, 6).await.unwrap(), b"first");
        assert_eq!(read_frame(&mut reader, 6).await.unwrap(), b"second");
        task.await.unwrap();
    }

    fn request() -> Request {
        serde_json::from_value(json!({
            "schema_version":1,"kind":"transport_request","run_id":"0123456789abcdef0123456789abcdef",
            "role":"server","listen_address":"127.0.0.1:9443","peer_address":"127.0.0.1:9443",
            "peer_file":"/tmp/peer.json","deadline_ms":1000,
            "workload":{"payload_bytes":8,"warmup_messages":0,"measured_messages":1,"bulk_bytes":1024}
        })).unwrap()
    }

    #[tokio::test]
    async fn refuses_stale_reordered_and_corrupted_payloads_before_ack() {
        for corruption in [4, 39, FRAME_HEADER] {
            let request = request();
            let mut bytes = natbench_transport_protocol::frame(&request, 0).unwrap();
            bytes[corruption] ^= 1;
            let (mut client, mut server) = tokio::io::duplex(4096);
            let task =
                tokio::spawn(async move { serve_frames(&mut server, &request, |_| Ok(())).await });
            write_frame(&mut client, &bytes).await.unwrap();
            assert!(read_frame(&mut client, bytes.len()).await.is_err());
            assert!(task.await.unwrap().is_err());
        }
    }

    #[tokio::test]
    async fn bulk_ack_requires_every_verified_byte() {
        let request = request();
        let server_request = request.clone();
        let (mut client, mut server) = tokio::io::duplex(64);
        let task =
            tokio::spawn(
                async move { serve_frames(&mut server, &server_request, |_| Ok(())).await },
            );
        for sequence in 0..request.workload.exchanges() {
            let bytes = natbench_transport_protocol::frame(&request, sequence).unwrap();
            let expected =
                natbench_transport_protocol::response(&request, sequence, &bytes).unwrap();
            write_frame(&mut client, &bytes).await.unwrap();
            assert_eq!(
                read_frame(&mut client, bytes.len()).await.unwrap(),
                expected
            );
        }
        task.await.unwrap().unwrap();
    }
}
