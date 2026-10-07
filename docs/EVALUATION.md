# Evaluate the application runner preview

M2's stable v0.2.0 launch requires at least one outside project or maintainer to
evaluate the workflow. v0.2.0-alpha.1 is a preview for collecting that evidence;
publication alone does not satisfy the gate. The expanded M2 deliverable also
requires [matched transport comparisons](TRANSPORT_COMPARISONS.md); the first direct-stream cohort is available in development builds. This alpha
predates its adapters and normalized metrics.

1. Follow [getting started](GETTING_STARTED.md) on a fresh Linux host. Record the
   natbench version, architecture, kernel and active doctor result. Confirm that
   the failure example exits 1 and the corrected repeated scenario exits 0.
2. Identify the failed process and phase from the report, logs and timeline.
   Check packet evidence at the client and WAN. Note anything unclear or missing.
3. Run one of your project's programs without editing natbench source. Declare
   readiness and a fresh-data assertion, record your program's version/build and
   run at least three attempts. Add a relevant negative condition or service outage.
4. Run the scenario in CI and download its artifact. Confirm that a failed
   requirement fails the job and that you can diagnose it from the retained bundle.
5. Share what you tested, actual versus expected behavior, reproducibility, and the
   largest adoption obstacle in a [repository issue](https://github.com/0xprames/natbench/issues/new).
   Link public CI evidence if suitable, or describe the results without uploading
   sensitive packet data or logs. Include whether the tool is useful enough to keep
   as a regression check in your project.

Current scope: fixed five-role IPv4 Linux fixtures, three kernel profiles, sequential
application steps, declared readiness, service stop/restart, and bounded collection.
Programs manage their own discovery and traversal. Application input versions 2
and 3 are supported; suite reports use version 2, repetition and capture reports
use version 1. Capture requires a separately installed tcpdump.

Scheduled network rebinding, composable impairments and IPv6 are M3 work. Broader
coturn/browser/libp2p interoperability is M4 work; portable real-network probes
are M5 work. The built-in relay is a test mailbox, not TURN. These capabilities
must not be inferred from a successful application runner demo.

Maintainer acceptance: record the evaluator and tested revision in a repository
evaluation issue, address blocking usability/reliability findings, and rerun the install and
CI walkthrough before tagging v0.2.0. Outreach messages require separate approval.
