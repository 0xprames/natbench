//! Explain fixture prerequisites; active checks stay inside a disposable Lab.
use crate::lab::{Lab, Profile, RouterInput};
use anyhow::Result;
use serde::Serialize;
use std::{env, fs, os::unix::fs::PermissionsExt, path::PathBuf};

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passed,
    Failed,
    NotChecked,
}
#[derive(Serialize)]
pub struct Check {
    pub name: String,
    pub required: bool,
    pub status: Status,
    pub detail: String,
    pub remedy: Option<String>,
}
#[derive(Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub version: &'static str,
    pub kernel: Option<String>,
    pub active_probe: bool,
    pub checks: Vec<Check>,
}
impl Report {
    pub fn exit_code(&self) -> i32 {
        i32::from(
            self.checks
                .iter()
                .any(|c| c.required && c.status != Status::Passed),
        )
    }
    pub fn summary(&self) -> String {
        let mut text = format!("natbench {} prerequisites\n", self.version);
        for check in &self.checks {
            text.push_str(&format!(
                "{:?} {}{}: {}\n",
                check.status,
                check.name,
                if check.required { "" } else { " (optional)" },
                check.detail
            ));
            if let Some(remedy) = &check.remedy {
                text.push_str(&format!("  {remedy}\n"));
            }
        }
        if !self.active_probe {
            text.push_str(
                "Run sudo natbench doctor --probe to verify namespace and kernel capabilities.\n",
            );
        }
        text
    }
}
pub(crate) fn executable(tool: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(tool))
        .find(|path| {
            fs::metadata(path)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}
fn check(name: &str, required: bool, result: Result<String>, remedy: &str) -> Check {
    match result {
        Ok(detail) => Check {
            name: name.into(),
            required,
            status: Status::Passed,
            detail,
            remedy: None,
        },
        Err(error) => Check {
            name: name.into(),
            required,
            status: Status::Failed,
            detail: format!("{error:#}"),
            remedy: Some(remedy.into()),
        },
    }
}
pub fn inspect(probe: bool) -> Report {
    let mut checks = Vec::new();
    let linux = cfg!(target_os = "linux");
    checks.push(check(
        "linux",
        true,
        if linux {
            Ok("Linux fixture backend".into())
        } else {
            Err(anyhow::anyhow!("Linux is required"))
        },
        "Use a Linux host or VM.",
    ));
    // SAFETY: geteuid has no preconditions.
    let root = unsafe { libc::geteuid() } == 0;
    checks.push(Check { name: "root".into(), required: true,
        status: if root { Status::Passed } else { Status::Failed },
        detail: if root { "effective UID is 0; kernel capabilities still need probing" } else { "fixture commands require root" }.into(),
        remedy: (!root).then(|| "Rerun fixture commands with sudo. A restricted container may still lack required capabilities.".into()) });
    for (tool, required, package) in [
        ("ip", true, "iproute2"),
        ("nft", true, "nftables"),
        ("sysctl", true, "procps"),
        ("conntrack", false, "conntrack"),
        ("tc", false, "iproute2"),
    ] {
        checks.push(check(
            tool,
            required,
            executable(tool)
                .map(|p| p.display().to_string())
                .ok_or_else(|| anyhow::anyhow!("executable not found on PATH")),
            &format!("Install {package}; ensure {tool} is on root's PATH."),
        ));
    }
    let prerequisites = checks
        .iter()
        .all(|c| !c.required || c.status == Status::Passed);
    if probe && prerequisites {
        match Lab::create(Profile::Preserve, Profile::Preserve, RouterInput::Drop) {
            Err(error) => checks.push(check("kernel_fixture", true, Err(error), "Allow namespace creation, veth/bridge devices and network administration; use a Linux VM if the container restricts them.")),
            Ok(lab) => {
                checks.push(check("kernel_fixture", true, Ok("created namespaces, veths, bridge, forwarding and nftables NAT/filter rules".into()), ""));
                if executable("tc").is_some() {
                    checks.push(check("netem", false, lab.set_wan_impairment(1, 0).map(|_| "installed netem on disposable router WANs".into()), "Enable sch_netem in the host kernel for impair experiments."));
                }
                if executable("conntrack").is_some() {
                    checks.push(check("conntrack_table", false, lab.run("ra", &["conntrack", "-L"]).map(|_| "read the disposable router's conntrack table".into()), "Enable connection tracking and network administration for lifetime/collision experiments."));
                }
                let nfqueue = (|| -> Result<String> {
                    lab.run("ra", &["nft", "add", "rule", "ip", "firewall", "transit", "meta", "l4proto", "udp", "queue", "num", "42"])?;
                    let exe = env::current_exe()?;
                    lab.run("ra", &[exe.to_str().ok_or_else(|| anyhow::anyhow!("non-UTF-8 executable path"))?, "__nfqueue-probe", &lab.namespaces["ra"]])?;
                    Ok("installed queue rule and bound/configured NFQUEUE 42 in a disposable router".into())
                })();
                checks.push(check("nfqueue", false, nfqueue, "Enable nft_queue and nfnetlink_queue with network administration for translate experiments."));
            }
        }
    } else {
        checks.push(Check {
            name: "kernel_fixture".into(),
            required: probe,
            status: Status::NotChecked,
            detail: if probe {
                "required prerequisites failed; no active checks attempted"
            } else {
                "no network resources created"
            }
            .into(),
            remedy: Some(
                "Run sudo natbench doctor --probe after resolving required checks.".into(),
            ),
        });
    }
    Report {
        schema_version: 1,
        version: env!("CARGO_PKG_VERSION"),
        kernel: fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().into()),
        active_probe: probe,
        checks,
    }
}
