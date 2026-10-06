#![doc = include_str!("../README.md")]

//! Network fixtures and measured NAT experiments for Linux.
pub mod bench;
pub mod lab;
mod quic;
mod stun;
mod tcp;
mod translate;
mod webrtc;
mod worker;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};

pub fn cancellation() -> &'static Arc<AtomicBool> {
    static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

pub fn check_cancelled() -> anyhow::Result<()> {
    anyhow::ensure!(!cancellation().load(Ordering::Relaxed), "interrupted");
    Ok(())
}

#[doc(hidden)]
pub fn run_endpoint(binds: &str) -> anyhow::Result<()> {
    worker::endpoint(binds)
}
#[doc(hidden)]
pub fn run_relay() -> anyhow::Result<()> {
    worker::relay()
}
#[doc(hidden)]
pub fn run_nat(lan: &str, wan: &str) -> anyhow::Result<()> {
    translate::run(lan, wan)
}
#[doc(hidden)]
pub fn run_stun(binds: &str) -> anyhow::Result<()> {
    worker::stun_server(binds)
}
#[doc(hidden)]
pub fn run_quic(role: &str) -> anyhow::Result<()> {
    quic::run(role)
}
#[doc(hidden)]
pub fn run_webrtc(role: &str) -> anyhow::Result<()> {
    webrtc::run(role)
}
#[doc(hidden)]
pub fn run_tcp(role: &str) -> anyhow::Result<()> {
    tcp::run(role)
}
#[doc(hidden)]
pub fn run_tcp_observer() -> anyhow::Result<()> {
    tcp::observe()
}
