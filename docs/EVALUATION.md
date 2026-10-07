# Evaluate the application and transport preview

M2's stable v0.2.0 launch requires at least one outside project or maintainer to
evaluate the workflow. v0.2.0-alpha.2 is the preview for collecting that evidence;
publication alone does not satisfy the gate. It includes application scenarios,
[matched direct-stream comparisons](ADAPTERS.md), and [real iroh connectivity
coverage](IROH_CONNECTIVITY.md). Native x86-64/ARM64 bundles include the example
executables and their separately buildable source.

## Check the install and evidence

1. Follow [getting started](GETTING_STARTED.md) on a fresh Linux host. Record the
   natbench version, architecture, kernel and active doctor result. Verify the
   download checksum and confirm all three bundled executables report alpha.2.
2. Choose an evaluation path:
   - **Application regression:** reproduce the blocked-UDP example (exit 1), locate
     the failed process/phase in its report and logs, and confirm the corrected
     repeated scenario exits 0. Run one of your project's programs without editing
     natbench source; declare readiness and a fresh-data assertion, repeat at least
     three times and add a relevant negative condition or service outage.
   - **Transport comparison:** run the shipped direct configuration, inspect counts
     alongside conditional RTT/first-data/goodput samples, and inspect the raw
     attempt evidence. Run the iroh verifier and distinguish direct/relay delivery,
     fresh data after relay shutdown, and the two expected relay-dependent outages.
     To test another implementation, supply an external executable implementing
     [the adapter contract](ADAPTERS.md#implement-an-external-adapter).
3. Run the chosen workflow in CI and download its artifact. The copyable
   [application](../examples/github-actions/application.yml) and
   [transport](../examples/github-actions/transports.yml) workflows pin the preview.
   Confirm that a failed delivery requirement fails the job and the retained bundle
   identifies its phase. An expected negative case passing its assertions is
   different from a transport successfully delivering the full workload.
4. Share your project/use case, tested version, actual versus expected behavior,
   reproducibility and largest adoption obstacle in a
   [repository issue](https://github.com/0xprames/natbench/issues/new).
   Link suitable public CI evidence or describe the results without uploading
   sensitive packet data or logs. Include whether you would keep this workflow as
   a regression check. Installation friction and unclear evidence are useful findings.

These checks are a guide for evaluation, not a claim that an outside user has
completed them. Maintainer acceptance records the evaluator and tested revision
in a repository evaluation issue, addresses blocking findings and reruns the install
and CI walkthrough before tagging stable v0.2.0. Outreach messages require separate
approval.

## Current scope

The fixture has fixed five-role IPv4 Linux networks, three kernel profiles,
sequential application steps, declared readiness, service stop/restart and bounded
collection. Programs manage their own discovery and traversal. Application input
versions 2/3, suite report 2 and repetition/capture report 1 remain supported.
Capture requires a separately installed tcpdump; verification scripts use Python 3.

`compare` measures direct reliable streams from a NATed client to a WAN receiver.
The iroh example runs two NATed peers with a real local relay and address discovery;
it has explicit identity/relay bootstrap and disables public lookup/relay services.
These connectivity cases remain separate from direct-stream performance summaries.
The built-in test mailbox relay and the real iroh relay are both distinct from TURN.

Scheduled rebinding, composable impairments and IPv6 are M3 work. Broader
coturn/browser/libp2p interoperability is M4 work; portable real-network probes
are M5 work. A passing synthetic test does not establish connectivity or performance
on an arbitrary user's network.
