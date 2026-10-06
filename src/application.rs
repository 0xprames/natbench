//! External programs run their own protocol inside the fixture.
use crate::{
    lab::{Lab, Process, Profile, RouterInput},
    scenario::{CaseReport, Report, Status},
};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Suite {
    schema_version: u32,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    a: Profile,
    b: Profile,
    router_input: RouterInput,
    processes: Vec<Program>,
    steps: Vec<Step>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Program {
    name: String,
    role: String,
    argv: Vec<String>,
    timeout_seconds: f64,
    #[serde(default = "default_cwd")]
    cwd: PathBuf,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    ready: Option<Ready>,
}
fn default_cwd() -> PathBuf {
    PathBuf::from(".")
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Ready {
    StdoutContains { text: String },
    Tcp { address: SocketAddr },
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    Start {
        process: String,
    },
    Run {
        process: String,
        #[serde(default)]
        expect_exit: i32,
        #[serde(default)]
        stdout_contains: Option<String>,
    },
    Stop {
        process: String,
    },
    Restart {
        process: String,
    },
}
impl Step {
    fn process(&self) -> &str {
        match self {
            Self::Start { process }
            | Self::Run { process, .. }
            | Self::Stop { process }
            | Self::Restart { process } => process,
        }
    }
}
impl Suite {
    fn validate(&self, base: &Path) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == 2 && !self.cases.is_empty() && self.cases.len() <= 100,
            "application schema 2 needs 1–100 cases"
        );
        let mut case_names = HashSet::new();
        for case in &self.cases {
            anyhow::ensure!(
                !case.name.trim().is_empty() && case_names.insert(&case.name),
                "case names must be nonempty and unique"
            );
            anyhow::ensure!(
                !case.processes.is_empty()
                    && case.processes.len() <= 32
                    && !case.steps.is_empty()
                    && case.steps.len() <= 100,
                "case {} needs 1–32 processes and 1–100 steps",
                case.name
            );
            let mut names = HashSet::new();
            for program in &case.processes {
                anyhow::ensure!(
                    !program.name.is_empty()
                        && program.name.len() <= 64
                        && program
                            .name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                        && names.insert(&program.name),
                    "process names must be unique safe identifiers"
                );
                anyhow::ensure!(
                    ["a", "b", "ra", "rb", "wan"].contains(&program.role.as_str()),
                    "unknown role {}",
                    program.role
                );
                anyhow::ensure!(
                    !program.argv.is_empty()
                        && !program.argv[0].is_empty()
                        && program.argv.iter().all(|a| !a.contains('\0')),
                    "process {} requires valid argv",
                    program.name
                );
                anyhow::ensure!(
                    program.timeout_seconds.is_finite()
                        && program.timeout_seconds > 0.
                        && program.timeout_seconds <= 3600.,
                    "invalid timeout for {}",
                    program.name
                );
                anyhow::ensure!(
                    base.join(&program.cwd).is_dir(),
                    "working directory missing for {}",
                    program.name
                );
                for (key, value) in &program.env {
                    anyhow::ensure!(
                        !key.is_empty() && !key.contains(['=', '\0']) && !value.contains('\0'),
                        "invalid environment for {}",
                        program.name
                    );
                }
                match &program.ready {
                    Some(Ready::StdoutContains { text }) => validate_pattern(text)?,
                    Some(Ready::Tcp { address }) => anyhow::ensure!(
                        address.is_ipv4() && address.port() != 0,
                        "TCP readiness requires IPv4 and a nonzero port"
                    ),
                    None => {}
                }
            }
            let mut live = HashSet::new();
            for step in &case.steps {
                let program = case
                    .processes
                    .iter()
                    .find(|p| p.name == step.process())
                    .context("step refers to an unknown process")?;
                match step {
                    Step::Start { process } => {
                        anyhow::ensure!(live.insert(process), "process {process} already started");
                        anyhow::ensure!(
                            program.ready.is_some(),
                            "start requires declared readiness for {process}"
                        );
                    }
                    Step::Restart { process } => {
                        anyhow::ensure!(
                            live.contains(process),
                            "restart requires a started process"
                        );
                        anyhow::ensure!(
                            program.ready.is_some(),
                            "restart requires declared readiness"
                        );
                    }
                    Step::Stop { process } => {
                        anyhow::ensure!(live.remove(process), "stop requires a started process")
                    }
                    Step::Run {
                        process,
                        stdout_contains,
                        expect_exit,
                    } => {
                        anyhow::ensure!(
                            (0..=255).contains(expect_exit),
                            "expected exit must be 0–255"
                        );
                        anyhow::ensure!(!live.contains(process), "cannot run a started service");
                        if let Some(pattern) = stdout_contains {
                            validate_pattern(pattern)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
fn validate_pattern(pattern: &str) -> Result<()> {
    anyhow::ensure!(
        !pattern.is_empty() && pattern.len() <= 4096,
        "stdout patterns must be 1–4096 bytes"
    );
    Ok(())
}
struct Matcher {
    file: File,
    tail: Vec<u8>,
    pattern: Vec<u8>,
}
impl Matcher {
    fn open(path: &Path, text: &str) -> Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            tail: Vec::new(),
            pattern: text.as_bytes().to_vec(),
        })
    }
    fn found(&mut self) -> Result<bool> {
        let mut buffer = [0u8; 8192];
        // Bound work per poll so continuous output cannot starve deadlines or signals.
        for _ in 0..16 {
            let length = self.file.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            self.tail.extend_from_slice(&buffer[..length]);
            if self
                .tail
                .windows(self.pattern.len())
                .any(|w| w == self.pattern)
            {
                return Ok(true);
            }
            let keep = self.pattern.len() - 1;
            if self.tail.len() > keep {
                self.tail.drain(..self.tail.len() - keep);
            }
        }
        Ok(false)
    }
}
struct Active {
    process: Process,
    stdout: PathBuf,
}
struct Runner<'a> {
    lab: Lab,
    case: &'a Case,
    base: &'a Path,
    artifacts: &'a Path,
    active: BTreeMap<String, Active>,
    generation: BTreeMap<String, u32>,
    events: Vec<Value>,
    timeline: File,
    started: Instant,
}
impl<'a> Runner<'a> {
    fn event(&mut self, process: &str, state: &str) -> Result<()> {
        let event = json!({"elapsed_ms":self.started.elapsed().as_millis(),"process":process,"state":state});
        serde_json::to_writer(&mut self.timeline, &event)?;
        writeln!(self.timeline)?;
        self.timeline.flush()?;
        self.events.push(event);
        Ok(())
    }
    fn launch(&mut self, program: &Program) -> Result<Active> {
        use std::os::unix::fs::PermissionsExt;
        let cwd = fs::canonicalize(self.base.join(&program.cwd))?;
        let executable = if program.argv[0].contains('/') {
            cwd.join(&program.argv[0])
        } else {
            let search = program
                .env
                .get("PATH")
                .map(std::ffi::OsString::from)
                .or_else(|| std::env::var_os("PATH"))
                .unwrap_or_default();
            std::env::split_paths(&search)
                .map(|directory| cwd.join(directory).join(&program.argv[0]))
                .find(|path| {
                    fs::metadata(path)
                        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                })
                .context("application executable not found on PATH")?
        };
        anyhow::ensure!(
            fs::metadata(&executable)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0),
            "application executable missing or not executable: {}",
            executable.display()
        );
        let mut argv = program.argv.clone();
        argv[0] = executable
            .to_str()
            .context("application path is not UTF-8")?
            .to_owned();
        let generation = self.generation.entry(program.name.clone()).or_default();
        *generation += 1;
        let prefix = format!("{}-{}", program.name, generation);
        let stdout = self.artifacts.join(format!("{prefix}.stdout.log"));
        let stderr = self.artifacts.join(format!("{prefix}.stderr.log"));
        let process = self.lab.spawn_application(
            &program.role,
            &argv,
            &cwd,
            &program.env,
            File::create(&stdout)?,
            File::create(&stderr)?,
        )?;
        self.event(&program.name, "started")?;
        Ok(Active { process, stdout })
    }
    fn ready(&mut self, program: &Program, active: &Active) -> Result<()> {
        let mut matcher = match program.ready.as_ref().unwrap() {
            Ready::StdoutContains { text } => Some(Matcher::open(&active.stdout, text)?),
            _ => None,
        };
        let deadline = Instant::now() + Duration::from_secs_f64(program.timeout_seconds);
        loop {
            crate::check_cancelled()?;
            if let Some(status) = active.process.status()? {
                bail!("{} exited before readiness: {status}", program.name);
            }
            let ready = match program.ready.as_ref().unwrap() {
                Ready::StdoutContains { .. } => matcher.as_mut().unwrap().found()?,
                Ready::Tcp { address } => {
                    let exe = std::env::current_exe()?;
                    self.lab
                        .run(
                            &program.role,
                            &[
                                exe.to_str().context("executable path is not UTF-8")?,
                                "__tcp-ready",
                                &address.to_string(),
                            ],
                        )
                        .is_ok()
                }
            };
            if ready {
                self.event(&program.name, "ready")?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.event(&program.name, "readiness_timeout")?;
                bail!("readiness timeout for {}", program.name);
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
    fn stop(&mut self, name: &str) -> Result<()> {
        let active = self
            .active
            .remove(name)
            .context("service was not started")?;
        active.process.stop()?;
        self.event(name, "stopped")
    }
    fn check_services(&self) -> Result<()> {
        for (name, active) in &self.active {
            if let Some(status) = active.process.status()? {
                bail!("service {name} exited unexpectedly: {status}");
            }
        }
        Ok(())
    }
    fn execute(mut self) -> Result<CaseReport> {
        let mut messages = Vec::new();
        for (index, step) in self.case.steps.iter().enumerate() {
            let step_result = (|| -> Result<()> {
                crate::check_cancelled()?;
                self.check_services()?;
                let program = self
                    .case
                    .processes
                    .iter()
                    .find(|p| p.name == step.process())
                    .unwrap();
                match step {
                    Step::Start { process } | Step::Restart { process } => {
                        if matches!(step, Step::Restart { .. }) {
                            self.stop(process)?;
                        }
                        let active = self.launch(program)?;
                        self.ready(program, &active)?;
                        self.active.insert(process.clone(), active);
                    }
                    Step::Stop { process } => self.stop(process)?,
                    Step::Run {
                        expect_exit,
                        stdout_contains,
                        ..
                    } => {
                        let active = self.launch(program)?;
                        let deadline =
                            Instant::now() + Duration::from_secs_f64(program.timeout_seconds);
                        let status = loop {
                            crate::check_cancelled()?;
                            self.check_services()?;
                            if let Some(status) = active.process.status()? {
                                break status;
                            }
                            if Instant::now() >= deadline {
                                self.event(&program.name, "execution_timeout")?;
                                bail!("execution timeout for {}", program.name);
                            }
                            thread::sleep(Duration::from_millis(25));
                        };
                        self.event(&program.name, "exited")?;
                        if status.code() != Some(*expect_exit) {
                            messages.push(format!(
                                "{}: expected exit {expect_exit}, observed {status}",
                                program.name
                            ));
                        }
                        if let Some(text) = stdout_contains {
                            let mut matcher = Matcher::open(&active.stdout, text)?;
                            let mut found = false;
                            // Scan captured output in bounded chunks while respecting the command deadline.
                            loop {
                                if matcher.found()? {
                                    found = true;
                                    break;
                                }
                                if matcher.file.metadata()?.len()
                                    == std::io::Seek::stream_position(&mut matcher.file)?
                                {
                                    break;
                                }
                                crate::check_cancelled()?;
                                anyhow::ensure!(
                                    Instant::now() < deadline,
                                    "stdout assertion timeout for {}",
                                    program.name
                                );
                            }
                            if !found {
                                messages.push(format!(
                                    "{}: expected stdout text was absent",
                                    program.name
                                ));
                            }
                        }
                    }
                }
                Ok(())
            })();
            step_result
                .with_context(|| format!("step {} for process {}", index + 1, step.process()))?;
            if !messages.is_empty() {
                break;
            }
        }
        let names = self.active.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.stop(&name)?;
        }
        Ok(CaseReport {
            name: self.case.name.clone(),
            status: if messages.is_empty() {
                Status::Passed
            } else {
                Status::AssertionFailed
            },
            messages,
            observation: Some(
                json!({"schema_version":1,"experiment":"application","events":self.events,"logs_directory":self.artifacts.file_name().unwrap().to_string_lossy()}),
            ),
        })
    }
}
pub fn run(path: &Path, artifacts: &Path, input: &[u8]) -> Result<Report> {
    let suite: Suite = serde_json::from_slice(input).context("parse application scenario")?;
    let source = fs::canonicalize(path)?;
    let base = source.parent().unwrap();
    suite.validate(base)?;
    fs::create_dir(artifacts)
        .context("artifact directory must be new and its parent must exist")?;
    fs::copy(path, artifacts.join("scenario.json"))?;
    crate::scenario::run_application_cases(
        path.display().to_string(),
        suite.cases.iter().map(|c| c.name.clone()).collect(),
        |index| {
            let case = &suite.cases[index];
            let directory = artifacts.join(format!("case-{index:03}"));
            fs::create_dir(&directory)?;
            let timeline = File::create(directory.join("timeline.jsonl"))?;
            let runner = Runner {
                lab: Lab::create(case.a, case.b, case.router_input)?,
                case,
                base,
                artifacts: &directory,
                active: BTreeMap::new(),
                generation: BTreeMap::new(),
                events: Vec::new(),
                timeline,
                started: Instant::now(),
            };
            runner.execute()
        },
        artifacts,
    )
}
