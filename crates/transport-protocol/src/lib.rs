//! A small executable boundary, independent of any transport implementation.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};

pub const MAX_EVENT_BYTES: usize = 1_048_576;
pub const FRAME_HEADER: usize = 40;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub payload_bytes: u32,
    pub warmup_messages: u32,
    pub measured_messages: u32,
    pub bulk_bytes: u32,
}
impl Workload {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=65_536).contains(&self.payload_bytes),
            "payload_bytes must be 1–65536"
        );
        ensure!(self.warmup_messages <= 100, "warmup_messages must be 0–100");
        ensure!(
            (1..=1000).contains(&self.measured_messages),
            "measured_messages must be 1–1000"
        );
        ensure!(
            (1024..=16_777_216).contains(&self.bulk_bytes),
            "bulk_bytes must be 1024–16777216"
        );
        Ok(())
    }
    pub fn exchanges(&self) -> u32 {
        2 + self.warmup_messages + self.measured_messages
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Server,
    Client,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub kind: String,
    pub run_id: String,
    pub role: Role,
    pub listen_address: SocketAddr,
    pub peer_address: SocketAddr,
    pub peer_file: PathBuf,
    pub deadline_ms: u64,
    pub workload: Workload,
}
impl Request {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1 && self.kind == "transport_request",
            "unsupported transport request"
        );
        ensure!(
            self.run_id.len() == 32 && self.run_id.bytes().all(|b| b.is_ascii_hexdigit()),
            "run_id must be 32 hexadecimal bytes"
        );
        ensure!(
            self.listen_address.is_ipv4()
                && self.peer_address.is_ipv4()
                && self.peer_address.port() != 0
                && !self.peer_address.ip().is_unspecified(),
            "requests require literal IPv4 addresses and a nonzero peer port"
        );
        ensure!(
            self.role != Role::Server || self.listen_address.port() != 0,
            "server listen port cannot be zero"
        );
        ensure!(self.peer_file.is_absolute(), "peer_file must be absolute");
        ensure!(
            (1..=60_000).contains(&self.deadline_ms),
            "deadline_ms must be 1–60000"
        );
        self.workload.validate()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Implementation {
    pub name: String,
    pub version: String,
    pub settings: BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
pub struct Bootstrap {
    pub schema_version: u32,
    pub kind: String,
    pub run_id: String,
    pub implementation: Implementation,
    pub details: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measurement {
    #[serde(deserialize_with = "read_workload_snapshot")]
    pub workload: Workload,
    pub path: String,
    pub path_evidence: Value,
    pub first_data_seconds: f64,
    pub message_rtt_seconds: Vec<f64>,
    pub bulk_verified_bytes: u32,
    pub bulk_seconds: f64,
}
fn read_workload_snapshot<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Workload, D::Error> {
    let mut value = Value::deserialize(deserializer)?;
    if let Some(object) = value.as_object_mut() {
        object.retain(|key, _| {
            [
                "payload_bytes",
                "warmup_messages",
                "measured_messages",
                "bulk_bytes",
            ]
            .contains(&key.as_str())
        });
    }
    serde_json::from_value(value).map_err(serde::de::Error::custom)
}
impl Measurement {
    pub fn validate(&self, request: &Request) -> Result<()> {
        ensure!(
            self.workload == request.workload,
            "measurement workload differs from request"
        );
        ensure!(
            self.path == "direct",
            "this workload requires a direct path"
        );
        ensure!(
            self.message_rtt_seconds.len() == request.workload.measured_messages as usize,
            "wrong number of message RTT samples"
        );
        let deadline = request.deadline_ms as f64 / 1000.;
        for sample in std::iter::once(&self.first_data_seconds)
            .chain(&self.message_rtt_seconds)
            .chain(std::iter::once(&self.bulk_seconds))
        {
            ensure!(
                sample.is_finite() && *sample > 0. && *sample <= deadline,
                "invalid application timing sample"
            );
        }
        ensure!(
            self.bulk_verified_bytes == request.workload.bulk_bytes,
            "bulk verification is incomplete"
        );
        ensure!(
            (f64::from(self.bulk_verified_bytes) / self.bulk_seconds).is_finite(),
            "derived bulk goodput is not finite"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventData {
    Ready { capabilities: Vec<String> },
    Completed { measurement: Measurement },
    Failed { phase: String, message: String },
    Unsupported { reason: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub schema_version: u32,
    pub kind: String,
    pub run_id: String,
    pub implementation: Implementation,
    #[serde(flatten)]
    pub data: EventData,
}
impl Event {
    pub fn new(request: &Request, implementation: Implementation, data: EventData) -> Self {
        Self {
            schema_version: 1,
            kind: "transport_event".into(),
            run_id: request.run_id.clone(),
            implementation,
            data,
        }
    }
    pub fn validate(&self, request: &Request) -> Result<()> {
        ensure!(
            self.schema_version == 1
                && self.kind == "transport_event"
                && self.run_id == request.run_id,
            "event version/kind/run_id differs from request"
        );
        ensure!(
            !self.implementation.name.trim().is_empty()
                && !self.implementation.version.trim().is_empty(),
            "implementation identity/version is missing"
        );
        if let EventData::Completed { measurement } = &self.data {
            measurement.validate(request)?;
        }
        match &self.data {
            EventData::Ready { capabilities } => ensure!(
                capabilities.iter().all(|c| !c.trim().is_empty()),
                "capability names must be nonempty"
            ),
            EventData::Failed { phase, message } => ensure!(
                !phase.trim().is_empty() && !message.trim().is_empty(),
                "failure phase/message must be nonempty"
            ),
            EventData::Unsupported { reason } => ensure!(
                !reason.trim().is_empty(),
                "unsupported reason must be nonempty"
            ),
            EventData::Completed { .. } => {}
        }
        Ok(())
    }
}
/// Every exchange is tied to this attempt, its sequence and deterministic payload bytes.
pub fn frame(request: &Request, sequence: u32) -> Result<Vec<u8>> {
    request.validate()?;
    ensure!(
        sequence < request.workload.exchanges(),
        "exchange sequence exceeds workload"
    );
    let bulk = sequence + 1 == request.workload.exchanges();
    let length = if bulk {
        request.workload.bulk_bytes
    } else {
        request.workload.payload_bytes
    } as usize;
    let mut bytes = Vec::with_capacity(FRAME_HEADER + length);
    bytes.extend_from_slice(b"NB01");
    bytes.extend_from_slice(request.run_id.as_bytes());
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.extend((0..length).map(|i| {
        request.run_id.as_bytes()[i % 32] ^ ((i as u64 * 31 + u64::from(sequence) * 17) % 251) as u8
    }));
    Ok(bytes)
}
/// The receiver verifies every byte before echoing a message or acknowledging bulk delivery.
pub fn response(request: &Request, sequence: u32, bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        bytes == frame(request, sequence)?,
        "received payload identity, sequence or bytes differ"
    );
    if sequence + 1 == request.workload.exchanges() {
        let mut ack = bytes[..FRAME_HEADER].to_vec();
        ack[..4].copy_from_slice(b"NBOK");
        ack.extend_from_slice(&request.workload.bulk_bytes.to_be_bytes());
        Ok(ack)
    } else {
        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn request() -> Request {
        Request {
            schema_version: 1,
            kind: "transport_request".into(),
            run_id: "0123456789abcdef0123456789abcdef".into(),
            role: Role::Client,
            listen_address: "0.0.0.0:0".parse().unwrap(),
            peer_address: "198.18.0.1:9443".parse().unwrap(),
            peer_file: "/tmp/peer.json".into(),
            deadline_ms: 5000,
            workload: Workload {
                payload_bytes: 128,
                warmup_messages: 2,
                measured_messages: 3,
                bulk_bytes: 1024,
            },
        }
    }
    #[test]
    fn receiver_rejects_corruption_wrong_sequence_and_stale_attempts() {
        let request = request();
        let mut bytes = frame(&request, 0).unwrap();
        assert_eq!(response(&request, 0, &bytes).unwrap(), bytes);
        bytes[FRAME_HEADER] ^= 1;
        assert!(response(&request, 0, &bytes).is_err());
        assert!(response(&request, 1, &frame(&request, 0).unwrap()).is_err());
        let mut stale = request.clone();
        stale.run_id.replace_range(..1, "f");
        assert!(response(&request, 0, &frame(&stale, 0).unwrap()).is_err());
        let sequence = request.workload.exchanges() - 1;
        let bulk = frame(&request, sequence).unwrap();
        let ack = response(&request, sequence, &bulk).unwrap();
        assert_eq!(ack.len(), FRAME_HEADER + 4);
        assert!(frame(&request, sequence + 1).is_err());
    }
    #[test]
    fn contract_rejects_unknown_inputs_and_incomplete_or_invalid_metrics() {
        let request = request();
        request.validate().unwrap();
        let mut json = serde_json::to_value(&request).unwrap();
        json["typo"] = true.into();
        assert!(serde_json::from_value::<Request>(json).is_err());
        let mut measurement = Measurement {
            workload: request.workload.clone(),
            path: "direct".into(),
            path_evidence: Value::Null,
            first_data_seconds: 0.1,
            message_rtt_seconds: vec![0.01; 3],
            bulk_verified_bytes: 1024,
            bulk_seconds: 0.1,
        };
        measurement.validate(&request).unwrap();
        measurement.bulk_verified_bytes -= 1;
        assert!(measurement.validate(&request).is_err());
        measurement.bulk_verified_bytes += 1;
        measurement.message_rtt_seconds.pop();
        assert!(measurement.validate(&request).is_err());
        measurement.message_rtt_seconds.push(f64::NAN);
        assert!(measurement.validate(&request).is_err());
    }
}
