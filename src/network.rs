//! Declared, bounded link conditions and retained kernel evidence for application fixtures.
use crate::{lab::Lab, scenario};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::Ordering,
};

const QUEUE_LIMIT: u32 = 4096;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Conditions {
    pub delay_ms: u32,
    pub loss_percent: f64,
}
impl Conditions {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.delay_ms <= 1000, "delay_ms must be 0–1000 per link");
        ensure!(
            self.loss_percent.is_finite() && (0.0..=100.0).contains(&self.loss_percent),
            "loss_percent must be finite and 0–100"
        );
        Ok(())
    }
    fn enabled(&self) -> bool {
        self.delay_ms != 0 || self.loss_percent != 0.0
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Egress {
    ClientA,
    RouterALan,
    RouterAWan,
    WanRouterA,
    ClientB,
    RouterBLan,
    RouterBWan,
    WanRouterB,
}
impl Egress {
    fn binding(self) -> (&'static str, &'static str) {
        match self {
            Self::ClientA => ("a", "eth0"),
            Self::RouterALan => ("ra", "lan"),
            Self::RouterAWan => ("ra", "wan"),
            Self::WanRouterA => ("wan", "a"),
            Self::ClientB => ("b", "eth0"),
            Self::RouterBLan => ("rb", "lan"),
            Self::RouterBWan => ("rb", "wan"),
            Self::WanRouterB => ("wan", "b"),
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Link {
    pub egress: Egress,
    pub conditions: Conditions,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub links: Vec<Link>,
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=8).contains(&self.links.len()),
            "network needs 1–8 directed links"
        );
        let mut names = BTreeSet::new();
        for link in &self.links {
            ensure!(
                names.insert(link.egress),
                "network egress links must be unique"
            );
            link.conditions.validate()?;
        }
        Ok(())
    }
    pub fn prerequisites(&self) -> Result<()> {
        self.validate()?;
        crate::doctor::executable("tc")
            .context("declared network conditions require tc from iproute2 on root's PATH")?;
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectConfig {
    pub client_to_server: Conditions,
    pub server_to_client: Conditions,
}
impl DirectConfig {
    pub fn links(&self) -> Config {
        Config {
            links: vec![
                Link {
                    egress: Egress::RouterAWan,
                    conditions: self.client_to_server.clone(),
                },
                Link {
                    egress: Egress::WanRouterA,
                    conditions: self.server_to_client.clone(),
                },
            ],
        }
    }
}
#[derive(Clone, Serialize)]
struct LinkEvidence {
    egress: Egress,
    role: &'static str,
    device: &'static str,
    #[serde(skip)]
    namespace: String,
    requested: Conditions,
    queue_limit_packets: Option<u32>,
    configure_argv: Vec<String>,
    installed: bool,
    before: Option<Value>,
    configured: Option<Value>,
    after: Option<Value>,
}
#[derive(Clone, Serialize)]
pub(crate) struct Manifest {
    schema_version: u32,
    kind: &'static str,
    backend: &'static str,
    randomness: &'static str,
    tc_version: String,
    configured: bool,
    pub complete: bool,
    interrupted: bool,
    links: Vec<LinkEvidence>,
    errors: Vec<String>,
}
/// Dropped before the lab, preserving final counters on failure and cancellation.
pub(crate) struct Evidence {
    manifest: Manifest,
    path: PathBuf,
    tc: PathBuf,
    ip: PathBuf,
    finished: bool,
}
impl Evidence {
    fn save(&self) -> Result<()> {
        scenario::atomic_write(&self.path, &serde_json::to_vec_pretty(&self.manifest)?)
    }
    fn snapshot(&self, link: &LinkEvidence) -> Result<Value> {
        // Final read-only snapshots remain allowed after cancellation, while the
        // fixture still owns the namespace. No cleanup command changes host state.
        let output = Command::new(&self.ip)
            .args(["netns", "exec", &link.namespace])
            .arg(&self.tc)
            .args(["-j", "-s", "qdisc", "show", "dev", link.device])
            .output()?;
        ensure!(
            output.status.success(),
            "read qdisc state for {:?}: {}",
            link.egress,
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            output.stdout.len() <= 1_048_576,
            "qdisc evidence exceeds budget"
        );
        let value: Value =
            serde_json::from_slice(&output.stdout).context("parse tc JSON qdisc evidence")?;
        ensure!(
            value.as_array().is_some_and(|qdiscs| qdiscs
                .iter()
                .all(|q| q.is_object() && q["kind"].is_string() && q["handle"].is_string())),
            "tc qdisc evidence must be an array of qdisc objects"
        );
        Ok(value)
    }
    pub fn start(lab: &Lab, directory: &Path, config: &Config) -> Result<Self> {
        config.prerequisites()?;
        let tc = crate::doctor::executable("tc").unwrap();
        let version = Command::new(&tc).arg("-V").output()?;
        ensure!(version.status.success(), "read tc version");
        let mut evidence = Self {
            path: directory.join("network.json"),
            tc, ip: crate::doctor::executable("ip").context("ip executable missing")?,
            finished: false,
            manifest: Manifest { schema_version:1, kind:"network_conditions", backend:"linux_tc_netem",
                randomness:"kernel PRNG; no seed configured; repeated runs do not have identical loss schedules",
                tc_version:String::from_utf8_lossy(&version.stdout).trim().into(),
                configured:false, complete:false, interrupted:false, errors:Vec::new(),
                links:config.links.iter().map(|link| {
                    let (role,device)=link.egress.binding();
                    let enabled=link.conditions.enabled();
                    let configure_argv=if enabled { vec!["qdisc".into(),"add".into(),"dev".into(),device.into(),"root".into(),"handle".into(),"1:".into(),"netem".into(),"limit".into(),QUEUE_LIMIT.to_string(),"delay".into(),format!("{}ms",link.conditions.delay_ms),"loss".into(),"random".into(),format!("{}%",link.conditions.loss_percent)] } else { Vec::new() };
                    LinkEvidence { egress:link.egress,role,device,namespace:lab.namespaces[role].clone(),requested:link.conditions.clone(),queue_limit_packets:enabled.then_some(QUEUE_LIMIT),configure_argv,installed:false,before:None,configured:None,after:None }
                }).collect() },
        };
        evidence.save()?;
        let result = (|| -> Result<()> {
            for index in 0..evidence.manifest.links.len() {
                crate::check_cancelled()?;
                let before = evidence.snapshot(&evidence.manifest.links[index])?;
                evidence.manifest.links[index].before = Some(before);
                evidence.save()?;
                let link = &evidence.manifest.links[index];
                if link.requested.enabled() {
                    let tc_name = evidence.tc.to_str().context("tc path is not UTF-8")?;
                    let args = std::iter::once(tc_name)
                        .chain(link.configure_argv.iter().map(String::as_str))
                        .collect::<Vec<_>>();
                    lab.run(link.role, &args)
                        .with_context(|| format!("configure {:?}", link.egress))?;
                    evidence.manifest.links[index].installed = true;
                }
                let observed = evidence.snapshot(&evidence.manifest.links[index])?;
                if evidence.manifest.links[index].installed {
                    ensure!(
                        observed
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|q| q["kind"] == "netem"
                                && q["handle"] == "1:"
                                && q["root"] == true),
                        "configured netem root qdisc missing from kernel evidence"
                    );
                }
                evidence.manifest.links[index].configured = Some(observed);
                evidence.save()?;
            }
            evidence.manifest.configured = true;
            evidence.save()
        })();
        if let Err(error) = result {
            evidence.manifest.errors.push(format!("{error:#}"));
            let _ = evidence.save();
            return Err(error);
        }
        Ok(evidence)
    }
    pub fn finish(&mut self) -> Result<Manifest> {
        if !self.finished {
            for index in 0..self.manifest.links.len() {
                match self.snapshot(&self.manifest.links[index]) {
                    Ok(value) => {
                        let link = &self.manifest.links[index];
                        if let Some(configured) = &link.configured {
                            if configuration(configured) != configuration(&value) {
                                self.manifest.errors.push(format!(
                                    "qdisc configuration changed for {:?} during the attempt",
                                    link.egress
                                ));
                            }
                        }
                        self.manifest.links[index].after = Some(value);
                    }
                    Err(error) => self.manifest.errors.push(format!("{error:#}")),
                }
            }
            self.manifest.interrupted = crate::cancellation().load(Ordering::Relaxed);
            self.manifest.complete = self.manifest.configured && self.manifest.errors.is_empty();
            self.finished = true;
            self.save()?;
        }
        ensure!(
            self.manifest.complete,
            "network evidence incomplete; inspect network.json"
        );
        Ok(self.manifest.clone())
    }
}
impl Drop for Evidence {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.finish();
        }
    }
}

fn configuration(snapshot: &Value) -> Value {
    Value::Array(snapshot.as_array().unwrap().iter().map(|q|serde_json::json!({
        "kind":q["kind"],"handle":q["handle"],"root":q["root"],"parent":q["parent"],"options":q["options"]
    })).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn link_inputs_reject_invalid_bounds_duplicates_and_unknown_fields() {
        for bad in [
            Conditions {
                delay_ms: 1001,
                loss_percent: 0.,
            },
            Conditions {
                delay_ms: 0,
                loss_percent: 100.01,
            },
            Conditions {
                delay_ms: 0,
                loss_percent: -0.1,
            },
            Conditions {
                delay_ms: 0,
                loss_percent: f64::NAN,
            },
        ] {
            assert!(bad.validate().is_err());
        }
        let json = serde_json::json!({"links":[{"egress":"wan_router_a","conditions":{"delay_ms":0,"loss_percent":0}}]});
        let cfg: Config = serde_json::from_value(json.clone()).unwrap();
        cfg.validate().unwrap();
        let mut bad = json.clone();
        bad["links"][0]["conditions"]["typo"] = true.into();
        assert!(serde_json::from_value::<Config>(bad).is_err());
        let mut bad = json.clone();
        bad["links"][0]["egress"] = "host".into();
        assert!(serde_json::from_value::<Config>(bad).is_err());
        let mut duplicate = cfg.clone();
        duplicate.links.push(cfg.links[0].clone());
        assert!(duplicate.validate().is_err());
        assert!(Config { links: vec![] }.validate().is_err());
    }
}
