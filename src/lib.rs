#![doc = include_str!("../README.md")]

//! Network fixtures and measured NAT experiments for Linux.
pub mod bench;
pub mod lab;
mod translate;
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
