//! Declarative built-in experiments and explicit CI expectations.
use crate::{
    bench::{self, Options},
    lab::{Profile, RouterInput},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub schema_version: u32,
    pub cases: Vec<Case>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub name: String,
    pub a: Profile,
    pub b: Profile,
    pub router_input: RouterInput,
    pub timeout_seconds: f64,
    pub expect: Vec<Expectation>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    pub pointer: String,
    pub equals: Value,
}
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passed,
    AssertionFailed,
    Inconclusive,
    InfrastructureFailed,
}
#[derive(Serialize)]
pub struct CaseReport {
    pub name: String,
    pub status: Status,
    pub messages: Vec<String>,
    pub observation: Option<Value>,
}
#[derive(Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub scenario: String,
    pub cases: Vec<CaseReport>,
}
impl Report {
    pub fn exit_code(&self) -> i32 {
        if self
            .cases
            .iter()
            .any(|c| c.status == Status::InfrastructureFailed)
        {
            2
        } else if self.cases.iter().any(|c| c.status == Status::Inconclusive) {
            3
        } else if self
            .cases
            .iter()
            .any(|c| c.status == Status::AssertionFailed)
        {
            1
        } else {
            0
        }
    }
    pub fn junit(&self) -> String {
        let failures = self
            .cases
            .iter()
            .filter(|c| c.status == Status::AssertionFailed)
            .count();
        let errors = self
            .cases
            .iter()
            .filter(|c| {
                matches!(
                    c.status,
                    Status::InfrastructureFailed | Status::Inconclusive
                )
            })
            .count();
        let mut xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"natbench\" tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\">\n", self.cases.len());
        for case in &self.cases {
            xml.push_str(&format!("<testcase name=\"{}\">", escape(&case.name)));
            let tag = match case.status {
                Status::Passed => None,
                Status::AssertionFailed => Some("failure"),
                _ => Some("error"),
            };
            if let Some(tag) = tag {
                xml.push_str(&format!(
                    "<{tag} message=\"{}\"/>",
                    escape(&case.messages.join("; "))
                ));
            }
            xml.push_str("</testcase>\n");
        }
        xml.push_str("</testsuite>\n");
        xml
    }
}
fn escape(input: &str) -> String {
    input
        .chars()
        .filter(|c| *c == '\t' || *c == '\n' || *c == '\r' || *c >= ' ')
        .flat_map(|c| match c {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            '"' => "&quot;".chars().collect(),
            '\'' => "&apos;".chars().collect(),
            _ => vec![c],
        })
        .collect()
}
impl Suite {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == 1,
            "unsupported scenario schema_version"
        );
        anyhow::ensure!(
            !self.cases.is_empty() && self.cases.len() <= 100,
            "suite must have 1–100 cases"
        );
        let mut names = std::collections::HashSet::new();
        for case in &self.cases {
            anyhow::ensure!(
                !case.name.trim().is_empty() && names.insert(&case.name),
                "case names must be nonempty and unique"
            );
            anyhow::ensure!(
                case.timeout_seconds.is_finite()
                    && case.timeout_seconds > 0.
                    && case.timeout_seconds <= 3600.,
                "invalid timeout for {}",
                case.name
            );
            anyhow::ensure!(
                !case.expect.is_empty(),
                "{} must declare expectations",
                case.name
            );
            for expectation in &case.expect {
                anyhow::ensure!(
                    expectation.pointer.starts_with('/'),
                    "expectation must use an absolute JSON pointer"
                );
                let mut chars = expectation.pointer.chars();
                while let Some(c) = chars.next() {
                    if c == '~' {
                        anyhow::ensure!(
                            matches!(chars.next(), Some('0' | '1')),
                            "invalid JSON pointer escape"
                        );
                    }
                }
            }
        }
        Ok(())
    }
    pub fn evaluate(
        &self,
        source: String,
        mut experiment: impl FnMut(&Case) -> Result<Value>,
    ) -> Result<Report> {
        self.validate()?;
        let mut cases = Vec::new();
        for case in &self.cases {
            crate::check_cancelled()?;
            let report = match experiment(case) {
                Err(error) => CaseReport {
                    name: case.name.clone(),
                    status: Status::InfrastructureFailed,
                    messages: vec![format!("{error:#}")],
                    observation: None,
                },
                Ok(observation) => {
                    let mut messages = Vec::new();
                    let mut missing = false;
                    for expectation in &case.expect {
                        match observation.pointer(&expectation.pointer) {
                            None => {
                                missing = true;
                                messages
                                    .push(format!("missing observation {}", expectation.pointer));
                            }
                            Some(actual) if *actual != expectation.equals => {
                                messages.push(format!(
                                    "{}: expected {}, observed {}",
                                    expectation.pointer, expectation.equals, actual
                                ))
                            }
                            _ => {}
                        }
                    }
                    let status = if missing {
                        Status::Inconclusive
                    } else if messages.is_empty() {
                        Status::Passed
                    } else {
                        Status::AssertionFailed
                    };
                    CaseReport {
                        name: case.name.clone(),
                        status,
                        messages,
                        observation: Some(observation),
                    }
                }
            };
            cases.push(report);
        }
        Ok(Report {
            schema_version: 1,
            scenario: source,
            cases,
        })
    }
}
pub fn run(path: &Path, artifacts: &Path) -> Result<Report> {
    let suite: Suite = serde_json::from_slice(&fs::read(path).context("read scenario")?)
        .context("parse scenario")?;
    suite.validate()?;
    // Never silently replace the evidence from an earlier run.
    fs::create_dir(artifacts)
        .context("artifact directory must be new and its parent must exist")?;
    fs::copy(path, artifacts.join("scenario.json"))?;
    let report = suite.evaluate(path.display().to_string(), |case| {
        let mut options = Options::current_exe()?;
        options.a = case.a;
        options.b = case.b;
        options.router_input = case.router_input;
        options.timeout_seconds = case.timeout_seconds;
        bench::benchmark(&options)
    })?;
    fs::write(
        artifacts.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    fs::write(artifacts.join("junit.xml"), report.junit())?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn suite() -> Suite {
        serde_json::from_value(json!({"schema_version":1,"cases":[{"name":"direct & <peer>","a":"preserve","b":"preserve","router_input":"drop","timeout_seconds":1,"expect":[{"pointer":"/direct","equals":true}]}]})).unwrap()
    }
    #[test]
    fn distinguishes_verdicts_from_fixture_failures() {
        let s = suite();
        assert_eq!(
            s.evaluate("test".into(), |_| Ok(json!({"direct":true})))
                .unwrap()
                .exit_code(),
            0
        );
        assert_eq!(
            s.evaluate("test".into(), |_| Ok(json!({"direct":false})))
                .unwrap()
                .exit_code(),
            1
        );
        assert_eq!(
            s.evaluate("test".into(), |_| anyhow::bail!("fixture unavailable"))
                .unwrap()
                .exit_code(),
            2
        );
        let missing = s.evaluate("test".into(), |_| Ok(json!({}))).unwrap();
        assert_eq!(missing.exit_code(), 3);
        assert!(missing
            .junit()
            .contains("name=\"direct &amp; &lt;peer&gt;\""));
        assert!(missing.junit().contains("<error"));
    }
    #[test]
    fn rejects_ambiguous_or_unasserted_suites_before_execution() {
        let mut s = suite();
        s.cases[0].expect.clear();
        assert!(s
            .evaluate("test".into(), |_| panic!("must not execute"))
            .is_err());
        let mut s = suite();
        s.cases[0].expect[0].pointer = "/bad~2escape".into();
        assert!(s.validate().is_err());
    }
}
