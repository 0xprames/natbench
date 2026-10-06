use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    process::{Child, Command, Output, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    Preserve,
    Random,
    UdpBlocked,
}
impl Profile {
    pub const ALL: [Self; 3] = [Self::Preserve, Self::Random, Self::UdpBlocked];
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum RouterInput {
    Drop,
    Accept,
}
impl RouterInput {
    fn policy(self) -> &'static str {
        match self {
            Self::Drop => "drop",
            Self::Accept => "accept",
        }
    }
}

/// A tracked child. Its owning Lab terminates it when the fixture is dropped.
#[derive(Clone)]
pub struct Process(pub(crate) Arc<Mutex<Child>>);
impl Process {
    pub fn wait(&self) -> Result<std::process::ExitStatus> {
        loop {
            crate::check_cancelled()?;
            if let Some(status) = self.0.lock().unwrap().try_wait()? {
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn stop(&self) -> Result<()> {
        let mut child = self.0.lock().unwrap();
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        child.wait()?;
        Ok(())
    }
}

/// Owns five network namespaces and every process launched through it.
pub struct Lab {
    pub namespaces: BTreeMap<String, String>,
    created: Vec<String>,
    links: Vec<String>,
    processes: Vec<Process>,
}
impl Lab {
    pub fn create(a: Profile, b: Profile, input: RouterInput) -> Result<Self> {
        // SAFETY: geteuid has no preconditions and does not mutate memory.
        if unsafe { libc::geteuid() } != 0 {
            bail!("network namespaces require root");
        }
        let mut random = [0u8; 5];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let tag = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut lab = Self {
            namespaces: ["wan", "a", "b", "ra", "rb"]
                .into_iter()
                .map(|r| (r.into(), format!("nb{tag}-{r}")))
                .collect(),
            created: Vec::new(),
            links: Vec::new(),
            processes: Vec::new(),
        };
        lab.setup(a, b, input)?;
        Ok(lab)
    }
    /// Add a second client behind router A on 10.3.0.2. It uses that router's masquerade.
    pub fn add_lan_client(&mut self) -> Result<()> {
        anyhow::ensure!(
            !self.namespaces.contains_key("a2"),
            "lan client already added"
        );
        let suffix = self.namespaces["wan"]
            .trim_start_matches("nb")
            .trim_end_matches("-wan")
            .to_owned();
        let name = format!("nb{suffix}-a2");
        host(&["ip", "netns", "add", &name])?;
        self.created.push(name.clone());
        self.namespaces.insert("a2".into(), name);
        self.run("a2", &["ip", "link", "set", "lo", "up"])?;
        self.link("a2", "eth0", "ra", "lan2")?;
        self.run("a2", &["ip", "addr", "add", "10.3.0.2/24", "dev", "eth0"])?;
        self.run("ra", &["ip", "addr", "add", "10.3.0.1/24", "dev", "lan2"])?;
        self.run("a2", &["ip", "route", "add", "default", "via", "10.3.0.1"])?;
        // The fixture only forwards the original LAN interface. This client needs the same path.
        self.run(
            "ra",
            &[
                "nft", "add", "rule", "ip", "firewall", "transit", "iifname", "lan2", "oifname",
                "wan", "accept",
            ],
        )?;
        Ok(())
    }
    /// Put a second client on router A's LAN, 10.1.0.3, by turning the point-to-point link into a bridge.
    /// The bridge keeps the name `lan`, so the existing forward rule still matches.
    pub fn add_same_lan_client(&mut self) -> Result<()> {
        anyhow::ensure!(
            !self.namespaces.contains_key("a2"),
            "lan client already added"
        );
        self.run("ra", &["ip", "link", "set", "dev", "lan", "down"])?;
        self.run("ra", &["ip", "addr", "del", "10.1.0.1/24", "dev", "lan"])?;
        self.run("ra", &["ip", "link", "set", "dev", "lan", "name", "lan0"])?;
        self.run("ra", &["ip", "link", "add", "lan", "type", "bridge"])?;
        self.run(
            "ra",
            &[
                "ip",
                "link",
                "set",
                "dev",
                "lan",
                "type",
                "bridge",
                "stp_state",
                "0",
            ],
        )?;
        self.run("ra", &["ip", "link", "set", "dev", "lan0", "master", "lan"])?;
        self.run("ra", &["ip", "addr", "add", "10.1.0.1/24", "dev", "lan"])?;
        self.run("ra", &["ip", "link", "set", "dev", "lan0", "up"])?;
        self.run("ra", &["ip", "link", "set", "dev", "lan", "up"])?;
        let suffix = self.namespaces["wan"]
            .trim_start_matches("nb")
            .trim_end_matches("-wan")
            .to_owned();
        let name = format!("nb{suffix}-a2");
        host(&["ip", "netns", "add", &name])?;
        self.created.push(name.clone());
        self.namespaces.insert("a2".into(), name);
        self.run("a2", &["ip", "link", "set", "lo", "up"])?;
        self.link("a2", "eth0", "ra", "lan1")?;
        self.run("ra", &["ip", "link", "set", "dev", "lan1", "master", "lan"])?;
        self.run("a2", &["ip", "addr", "add", "10.1.0.3/24", "dev", "eth0"])?;
        self.run("a2", &["ip", "route", "add", "default", "via", "10.1.0.1"])?;
        Ok(())
    }
    /// Shape every packet leaving both routers' WAN interfaces.
    pub fn set_wan_impairment(&self, delay_ms: u64, loss_percent: u64) -> Result<()> {
        let delay = format!("{delay_ms}ms");
        let loss = format!("{loss_percent}%");
        for router in ["ra", "rb"] {
            self.run(
                router,
                &[
                    "tc", "qdisc", "replace", "dev", "wan", "root", "netem", "delay", &delay,
                    "loss", &loss,
                ],
            )?;
        }
        Ok(())
    }
    fn command(&self, role: &str, args: &[&str]) -> Result<Command> {
        let ns = self
            .namespaces
            .get(role)
            .with_context(|| format!("unknown role {role}"))?;
        let mut command = Command::new("ip");
        command.args(["netns", "exec", ns]).args(args);
        Ok(command)
    }
    pub fn run(&self, role: &str, args: &[&str]) -> Result<Output> {
        crate::check_cancelled()?;
        checked(self.command(role, args)?.output()?, args)
    }
    pub fn spawn(&mut self, role: &str, args: &[&str]) -> Result<Process> {
        self.spawn_command(role, args, false)
    }
    pub(crate) fn spawn_piped(&mut self, role: &str, args: &[&str]) -> Result<Process> {
        self.spawn_command(role, args, true)
    }
    fn spawn_command(&mut self, role: &str, args: &[&str], piped: bool) -> Result<Process> {
        crate::check_cancelled()?;
        let mut cmd = self.command(role, args)?;
        if piped {
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
        }
        let process = Process(Arc::new(Mutex::new(
            cmd.spawn().context("launch namespace command")?,
        )));
        self.processes.push(process.clone());
        Ok(process)
    }
    fn link(&mut self, left: &str, li: &str, right: &str, ri: &str) -> Result<()> {
        let suffix = self.namespaces["wan"]
            .trim_start_matches("nb")
            .trim_end_matches("-wan");
        let x = format!("v{suffix}{}", self.links.len());
        let y = format!("v{suffix}{}", self.links.len() + 1);
        host(&["ip", "link", "add", &x, "type", "veth", "peer", "name", &y])?;
        self.links.extend([x.clone(), y.clone()]);
        for (name, role, device) in [(&x, left, li), (&y, right, ri)] {
            host(&[
                "ip",
                "link",
                "set",
                name,
                "netns",
                &self.namespaces[role],
                "name",
                device,
            ])?;
            self.run(role, &["ip", "link", "set", "dev", device, "up"])?;
        }
        Ok(())
    }
    fn setup(&mut self, a: Profile, b: Profile, input: RouterInput) -> Result<()> {
        for role in ["wan", "a", "b", "ra", "rb"] {
            let ns = self.namespaces[role].clone();
            host(&["ip", "netns", "add", &ns])?;
            self.created.push(ns);
            self.run(role, &["ip", "link", "set", "lo", "up"])?;
        }
        self.run("wan", &["ip", "link", "add", "br0", "type", "bridge"])?;
        self.run("wan", &["ip", "link", "set", "br0", "up"])?;
        for ip in ["198.18.0.1/24", "198.18.0.2/24"] {
            self.run("wan", &["ip", "addr", "add", ip, "dev", "br0"])?;
        }
        for (index, side, router, public, profile) in [
            (1, "a", "ra", "198.18.0.10/24", a),
            (2, "b", "rb", "198.18.0.20/24", b),
        ] {
            self.link(side, "eth0", router, "lan")?;
            self.link(router, "wan", "wan", side)?;
            self.run("wan", &["ip", "link", "set", "dev", side, "master", "br0"])?;
            for (role, device, address) in [
                (side, "eth0", format!("10.{index}.0.2/24")),
                (router, "lan", format!("10.{index}.0.1/24")),
                (router, "wan", public.into()),
            ] {
                self.run(role, &["ip", "addr", "add", &address, "dev", device])?;
            }
            self.run(
                side,
                &[
                    "ip",
                    "route",
                    "add",
                    "default",
                    "via",
                    &format!("10.{index}.0.1"),
                ],
            )?;
            self.run(router, &["sysctl", "-q", "-w", "net.ipv4.ip_forward=1"])?;
            self.apply_nat(router, profile, input)?;
        }
        Ok(())
    }
    /// Insert a second NAT between router A and the public bridge.
    /// Router A's WAN becomes 10.8.0.2; the new router publishes 198.18.0.10.
    pub fn add_outer_nat(&mut self, profile: Profile, input: RouterInput) -> Result<()> {
        anyhow::ensure!(
            !self.namespaces.contains_key("oa"),
            "outer nat already added"
        );
        let suffix = self.namespaces["wan"]
            .trim_start_matches("nb")
            .trim_end_matches("-wan")
            .to_owned();
        let name = format!("nb{suffix}-oa");
        host(&["ip", "netns", "add", &name])?;
        self.created.push(name.clone());
        self.namespaces.insert("oa".into(), name.clone());
        self.run("oa", &["ip", "link", "set", "lo", "up"])?;
        // The public veth is currently router A's WAN. Move that end onto the outer NAT
        // and use it as the outer LAN, then give the outer NAT a new public port.
        self.run("ra", &["ip", "addr", "del", "198.18.0.10/24", "dev", "wan"])?;
        self.run("wan", &["ip", "link", "set", "dev", "a", "nomaster"])?;
        self.run("wan", &["ip", "link", "set", "dev", "a", "down"])?;
        self.run("wan", &["ip", "link", "set", "dev", "a", "netns", &name])?;
        self.run("oa", &["ip", "link", "set", "dev", "a", "name", "lan"])?;
        self.run("oa", &["ip", "link", "set", "dev", "lan", "up"])?;
        self.run("ra", &["ip", "link", "set", "dev", "wan", "up"])?;
        self.run("ra", &["ip", "addr", "add", "10.8.0.2/24", "dev", "wan"])?;
        self.run("oa", &["ip", "addr", "add", "10.8.0.1/24", "dev", "lan"])?;
        self.run("ra", &["ip", "route", "add", "default", "via", "10.8.0.1"])?;
        self.link("oa", "wan", "wan", "a")?;
        self.run("wan", &["ip", "link", "set", "dev", "a", "master", "br0"])?;
        self.run("oa", &["ip", "addr", "add", "198.18.0.10/24", "dev", "wan"])?;
        self.run("oa", &["sysctl", "-q", "-w", "net.ipv4.ip_forward=1"])?;
        self.apply_nat("oa", profile, input)?;
        Ok(())
    }
    fn apply_nat(&self, router: &str, profile: Profile, input: RouterInput) -> Result<()> {
        let random = if profile == Profile::Random {
            " fully-random"
        } else {
            ""
        };
        let blocked = if profile == Profile::UdpBlocked {
            "meta l4proto udp drop;"
        } else {
            ""
        };
        let rules = format!(
            r#"table ip translation {{
 chain outbound {{ type nat hook postrouting priority srcnat; oifname "wan" masquerade{random}; }}
}}
table ip firewall {{
 chain router {{ type filter hook input priority filter; policy {};
 ct state established,related accept; iifname {{ "lo", "lan" }} accept; }}
 chain transit {{ type filter hook forward priority filter; policy drop;
 {blocked}
 ct state established,related accept; iifname "lan" oifname "wan" accept; }}
}}
"#,
            input.policy()
        );
        self.nft(router, &rules)
    }
    /// Queue forwarded UDP to an external translator and leave TCP on kernel masquerade.
    /// The program is executed as `nat --lan-interface lan --wan-interface wan` in each router.
    pub fn use_userspace_translator(&mut self, translator: &std::path::Path) -> Result<()> {
        let program = translator
            .to_str()
            .context("translator path is not valid unicode")?;
        let rules = r#"delete table ip translation
table ip translation {
 chain outbound { type nat hook postrouting priority srcnat; oifname "wan" meta l4proto tcp masquerade; }
}
table ip userspace {
 chain from_wan { type filter hook prerouting priority raw; policy accept;
  iifname "wan" meta l4proto udp queue flags bypass to 42; }
 chain to_wan { type filter hook postrouting priority raw; policy accept;
  oifname "wan" meta l4proto udp queue flags bypass to 42; }
}
add rule ip firewall transit meta l4proto udp accept
"#;
        for router in ["ra", "rb"] {
            self.nft(router, rules)?;
            let process = self.spawn(
                router,
                &[
                    program,
                    "nat",
                    "--lan-interface",
                    "lan",
                    "--wan-interface",
                    "wan",
                ],
            )?;
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                if process.0.lock().unwrap().try_wait()?.is_some() {
                    bail!("translator in {router} exited before it bound NFQUEUE 42");
                }
                if let Ok(output) = self
                    .command(router, &["cat", "/proc/net/netfilter/nfnetlink_queue"])?
                    .output()
                {
                    let ready = String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .any(|line| line.split_whitespace().next() == Some("42"));
                    if ready {
                        break;
                    }
                }
                if std::time::Instant::now() > deadline {
                    bail!("translator in {router} did not bind NFQUEUE 42");
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(())
    }
    fn nft(&self, router: &str, rules: &str) -> Result<()> {
        let mut child = self
            .command(router, &["nft", "-f", "-"])?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let write_result = child.stdin.take().unwrap().write_all(rules.as_bytes());
        let output = child.wait_with_output()?;
        checked(output, &["nft", "-f", "-"])?;
        write_result?;
        Ok(())
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        for process in &self.processes {
            let _ = process.stop();
        }
        for namespace in self.created.iter().rev() {
            if let Ok(output) = Command::new("ip")
                .args(["netns", "pids", namespace])
                .output()
            {
                for pid in String::from_utf8_lossy(&output.stdout).split_whitespace() {
                    if let Ok(pid) = pid.parse::<i32>() {
                        if pid > 0 {
                            // SAFETY: kill accepts a positive PID and a valid signal.
                            unsafe {
                                libc::kill(pid, libc::SIGKILL);
                            }
                        }
                    }
                }
            }
            let _ = Command::new("ip")
                .args(["netns", "del", namespace])
                .output();
        }
        for link in &self.links {
            let _ = Command::new("ip").args(["link", "del", link]).output();
        }
    }
}
fn checked(output: Output, args: &[&str]) -> Result<Output> {
    if !output.status.success() {
        bail!(
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}
fn host(args: &[&str]) -> Result<Output> {
    crate::check_cancelled()?;
    checked(
        Command::new(args[0])
            .args(&args[1..])
            .output()
            .with_context(|| format!("run {}", args[0]))?,
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires root and Linux network namespaces"]
    fn partial_setup_failure_removes_created_namespace() {
        let before = Command::new("ip")
            .args(["netns", "list"])
            .output()
            .unwrap()
            .stdout;
        let name = format!("nb-partial-{}", std::process::id());
        {
            // Reusing the first name forces setup to fail at its second namespace.
            let mut lab = Lab {
                namespaces: ["wan", "a", "b", "ra", "rb"]
                    .into_iter()
                    .map(|role| (role.into(), name.clone()))
                    .collect(),
                created: Vec::new(),
                links: Vec::new(),
                processes: Vec::new(),
            };
            assert!(lab
                .setup(Profile::Preserve, Profile::Preserve, RouterInput::Drop)
                .is_err());
            assert_eq!(lab.created.len(), 1);
        }
        let after = Command::new("ip")
            .args(["netns", "list"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(before, after);
    }
}
