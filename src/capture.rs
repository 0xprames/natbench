//! Bounded packet evidence from the application fixture's namespaces.
use crate::lab::{Lab, Process};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Serialize)]
pub(crate) struct Options {
    pub packets_per_role: u32,
    pub snaplen_bytes: u32,
    pub roles: [&'static str; 5],
    #[serde(skip)]
    executable: PathBuf,
}
impl Options {
    pub fn new(packets: u32) -> Result<Self> {
        anyhow::ensure!(
            (1..=100_000).contains(&packets),
            "capture packet budget must be 1–100000 per role"
        );
        let executable = crate::doctor::executable("tcpdump").context(
            "packet capture requires tcpdump; install tcpdump and ensure it is on root's PATH",
        )?;
        Ok(Self {
            packets_per_role: packets,
            snaplen_bytes: 256,
            roles: ["a", "b", "ra", "rb", "wan"],
            executable: fs::canonicalize(executable)?,
        })
    }
}
#[derive(Serialize)]
pub(crate) struct FileReport {
    role: String,
    pcap_file: String,
    stderr_file: String,
    complete: bool,
    forced_stop: bool,
    exit_code: Option<i32>,
    packets: Option<u32>,
    truncated_packets: Option<u32>,
    link_type: Option<u32>,
    packet_limit_reached: bool,
    kernel_dropped_packets: Option<u64>,
    error: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct Manifest {
    schema_version: u32,
    kind: &'static str,
    options: Options,
    pub complete: bool,
    files: Vec<FileReport>,
    error: Option<String>,
}
struct Participant {
    role: String,
    process: Process,
    pcap: PathBuf,
    stderr: PathBuf,
    budget_finished: Cell<bool>,
}
pub(crate) struct Group {
    options: Options,
    participants: Vec<Participant>,
    directory: PathBuf,
    finished: bool,
    error: Option<String>,
}
fn private_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}
fn stderr(path: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?.take(16_384).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
impl Group {
    pub fn start(lab: &mut Lab, directory: &Path, options: &Options) -> Result<Self> {
        let mut group = Self {
            options: options.clone(),
            participants: Vec::new(),
            directory: directory.to_owned(),
            finished: false,
            error: None,
        };
        let result = (|| -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(5);
            for role in options.roles {
                crate::check_cancelled()?;
                let pcap = directory.join(format!("{role}.pcap"));
                let log = directory.join(format!("{role}.tcpdump.log"));
                let argv = vec![
                    options
                        .executable
                        .to_str()
                        .context("tcpdump path is not UTF-8")?
                        .to_owned(),
                    "-i".into(),
                    "any".into(),
                    "-p".into(),
                    "-n".into(),
                    "-U".into(),
                    "--immediate-mode".into(),
                    "-s".into(),
                    options.snaplen_bytes.to_string(),
                    "-c".into(),
                    options.packets_per_role.to_string(),
                    "-Z".into(),
                    "root".into(),
                    "-w".into(),
                    "-".into(),
                ];
                let process = lab.spawn_application(
                    role,
                    &argv,
                    &std::env::current_dir()?,
                    &BTreeMap::from([("LC_ALL".into(), "C".into())]),
                    private_file(&pcap)?,
                    private_file(&log)?,
                )?;
                group.participants.push(Participant {
                    role: role.into(),
                    process,
                    pcap,
                    stderr: log,
                    budget_finished: Cell::new(false),
                });
                let participant = group.participants.last().unwrap();
                loop {
                    crate::check_cancelled()?;
                    if stderr(&participant.stderr)?.contains("listening on ") {
                        break;
                    }
                    if let Some(status) = participant.process.status()? {
                        bail!(
                            "capture {role} exited before readiness: {status}; {}",
                            stderr(&participant.stderr)?
                        );
                    }
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "capture readiness timeout for {role}"
                    );
                    thread::sleep(Duration::from_millis(20));
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            group.error = Some(format!("{error:#}"));
            return Err(error);
        }
        Ok(group)
    }
    pub fn check(&self) -> Result<()> {
        for participant in &self.participants {
            if participant.budget_finished.get() {
                continue;
            }
            if let Some(status) = participant.process.status()? {
                let stats = inspect(&participant.pcap, &self.options).with_context(|| {
                    format!("capture {} stopped unexpectedly", participant.role)
                })?;
                anyhow::ensure!(
                    status.success() && stats.packets == self.options.packets_per_role,
                    "capture {} exited before its packet budget: {status}",
                    participant.role
                );
                participant.budget_finished.set(true);
            }
        }
        Ok(())
    }
    pub fn finish(&mut self) -> Result<Manifest> {
        let deadline = Instant::now() + Duration::from_secs(2);
        for participant in &self.participants {
            if participant.process.status()?.is_none() {
                participant.process.interrupt()?;
            }
        }
        let mut files = Vec::new();
        for participant in &self.participants {
            let mut forced_stop = false;
            let status = loop {
                if let Some(status) = participant.process.status()? {
                    break status;
                }
                if Instant::now() >= deadline {
                    forced_stop = true;
                    participant.process.stop()?;
                    break participant
                        .process
                        .status()?
                        .context("capture status unavailable after stop")?;
                }
                thread::sleep(Duration::from_millis(10));
            };
            let result = inspect(&participant.pcap, &self.options);
            let error = result.as_ref().err().map(|error| format!("{error:#}"));
            let kernel_dropped_packets = stderr(&participant.stderr).ok().and_then(|log| {
                log.lines().find_map(|line| {
                    line.strip_suffix(" packets dropped by kernel")
                        .and_then(|count| count.trim().parse().ok())
                })
            });
            files.push(FileReport {
                role: participant.role.clone(),
                pcap_file: participant
                    .pcap
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                stderr_file: participant
                    .stderr
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                complete: result.is_ok() && status.success() && !forced_stop,
                forced_stop,
                exit_code: status.code(),
                packets: result.as_ref().ok().map(|stats| stats.packets),
                truncated_packets: result.as_ref().ok().map(|stats| stats.truncated_packets),
                link_type: result.as_ref().ok().map(|stats| stats.link_type),
                packet_limit_reached: result
                    .as_ref()
                    .is_ok_and(|stats| stats.packets == self.options.packets_per_role),
                kernel_dropped_packets,
                error,
            });
        }
        let manifest = Manifest {
            schema_version: 1,
            kind: "packet_capture",
            options: self.options.clone(),
            complete: self.error.is_none()
                && files.len() == self.options.roles.len()
                && files.iter().all(|file| file.complete),
            files,
            error: self.error.clone(),
        };
        crate::scenario::atomic_write(
            &self.directory.join("capture.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        self.finished = true;
        Ok(manifest)
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.finish();
        }
    }
}
struct Stats {
    packets: u32,
    truncated_packets: u32,
    link_type: u32,
}
fn inspect(path: &Path, options: &Options) -> Result<Stats> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    anyhow::ensure!(
        length
            <= 24 + u64::from(options.packets_per_role) * (16 + u64::from(options.snaplen_bytes)),
        "pcap exceeded its packet/snapshot budget"
    );
    let mut header = [0u8; 24];
    file.read_exact(&mut header)
        .context("pcap global header is incomplete")?;
    let little = match header[..4] {
        [0xd4, 0xc3, 0xb2, 0xa1] | [0x4d, 0x3c, 0xb2, 0xa1] => true,
        [0xa1, 0xb2, 0xc3, 0xd4] | [0xa1, 0xb2, 0x3c, 0x4d] => false,
        _ => bail!("unsupported capture format; expected classic pcap"),
    };
    let version = if little {
        (
            u16::from_le_bytes(header[4..6].try_into().unwrap()),
            u16::from_le_bytes(header[6..8].try_into().unwrap()),
        )
    } else {
        (
            u16::from_be_bytes(header[4..6].try_into().unwrap()),
            u16::from_be_bytes(header[6..8].try_into().unwrap()),
        )
    };
    anyhow::ensure!(version == (2, 4), "unsupported pcap version");
    let number = |bytes: &[u8]| {
        let bytes: [u8; 4] = bytes.try_into().unwrap();
        if little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        }
    };
    anyhow::ensure!(
        number(&header[16..20]) == options.snaplen_bytes,
        "unexpected capture snapshot length"
    );
    let mut stats = Stats {
        packets: 0,
        truncated_packets: 0,
        link_type: number(&header[20..24]),
    };
    while file.stream_position()? < length {
        let mut record = [0u8; 16];
        file.read_exact(&mut record)
            .context("pcap record header is incomplete")?;
        let included = number(&record[8..12]);
        let original = number(&record[12..16]);
        anyhow::ensure!(
            included <= options.snaplen_bytes
                && included <= original
                && file.stream_position()? + u64::from(included) <= length,
            "invalid or incomplete pcap record"
        );
        file.seek(SeekFrom::Current(i64::from(included)))?;
        stats.packets += 1;
        stats.truncated_packets += u32::from(included < original);
    }
    anyhow::ensure!(
        stats.packets <= options.packets_per_role,
        "pcap exceeded packet budget"
    );
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pcap_validation_counts_truncation_and_rejects_bad_lengths_and_budgets() {
        let options = Options {
            packets_per_role: 1,
            snaplen_bytes: 256,
            roles: ["a", "b", "ra", "rb", "wan"],
            executable: PathBuf::new(),
        };
        let path = std::env::temp_dir().join(format!("natbench-pcap-codec-{}", std::process::id()));
        for little in [true, false] {
            let number = |n: u32| {
                if little {
                    n.to_le_bytes()
                } else {
                    n.to_be_bytes()
                }
            };
            let mut bytes = if little {
                vec![0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0]
            } else {
                vec![0xa1, 0xb2, 0xc3, 0xd4, 0, 2, 0, 4]
            };
            bytes.extend([0u8; 8]);
            bytes.extend(number(256));
            bytes.extend(number(276));
            bytes.extend([0u8; 8]);
            bytes.extend(number(2));
            bytes.extend(number(4));
            bytes.extend([0, 1]);
            fs::write(&path, &bytes).unwrap();
            let stats = inspect(&path, &options).unwrap();
            assert_eq!(
                (stats.packets, stats.truncated_packets, stats.link_type),
                (1, 1, 276)
            );
            fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
            assert!(inspect(&path, &options).is_err());
            bytes.extend([0u8; 8]);
            bytes.extend(number(0));
            bytes.extend(number(0));
            fs::write(&path, &bytes).unwrap();
            assert!(inspect(&path, &options).is_err());
        }
        fs::remove_file(path).unwrap();
    }
}
