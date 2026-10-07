# Application runner preview launch (v0.2.0-alpha.1)

This brief records the first application preview. The current transport preview is
[v0.2.0-alpha.2](releases/v0.2.0-alpha.2.md); use its supported capabilities and
install commands when introducing the current version. The earlier verification
and discovery baseline below describe v0.2.0-alpha.1.

Introduce v0.2.0-alpha.1 to developers testing UDP, P2P, and service recovery.
Lead with a reproducible application failure, the retained evidence, and a CI
regression check. Invite users to try their own executable and report the largest
adoption obstacle. Stars let interested developers bookmark the project; successful
outside evaluations also move the [stable release gate](ROADMAP.md) forward.

The social post is a draft for maintainer review; the community sections are
factual briefs for a post written by the maintainer. External publication requires
approval of the message and destination, as specified in the roadmap. The alpha is available
for evaluation; the broader stable launch still depends on outside evaluation and
the planned transport comparisons.

## Repository discovery

Suggested GitHub description:

> Test application connectivity, NAT traversal, and service recovery in isolated Linux networks. Keep JSON/JUnit, logs, timelines, and optional packet captures.

Suggested topics: `nat-traversal`, `networking`, `p2p`, `network-testing`, `linux`,
`rust`, `udp`, `continuous-integration`.

Baseline on October 7, 2026: 0 stars, 0 forks, no topics, and 0 downloads of each
alpha binary before this launch verification. GitHub traffic views, clones, and
referrers require permissions unavailable to the current token. Release download
counts include verification downloads and are not counts of independent users.

## First distribution steps

1. Publish the README improvement after review and apply the repository settings.
2. Submit a Show HN when the maintainer can discuss the implementation and help
   people run it. This reaches a developer audience without depending on Twitter
   followers. The [Show HN guidelines](https://news.ycombinator.com/showhn.html)
   require runnable work and its creator's participation. HN's
   [general guidelines](https://news.ycombinator.com/newsguidelines.html) prohibit
   generated posts, AI-edited comments, and automated posting. Write the submission
   yourself, using the facts below rather than copying generated prose.
3. Share progress in r/rust's weekly project thread. Its
   [moderator announcement](https://www.reddit.com/r/rust/comments/1wkmzun/no_more_code_dumps/)
   directs application showcases there and prohibits generated announcement and
   article text. Use a personal account and write the comment yourself; check the
   current weekly thread and rules before posting.
4. Use the short social post as a secondary channel. Keep the alpha label and Linux
   requirement visible.
5. Record the published URLs and subsequent star count. Help evaluators get their
   first scenario passing, fix adoption blockers, and use the findings for the
   stable launch. Check stars, forks, outside evaluation reports, and release
   downloads after distribution; do not interpret changes as channel attribution
   without traffic evidence.

## Short social post draft

natbench alpha: reproduce UDP connectivity failures in isolated Linux networks, keep logs + packet captures, then make the scenario a CI regression test. Bring your own executable. Linux/root required. Try it: https://github.com/0xprames/natbench

## Show HN factual brief

Submission destination: https://news.ycombinator.com/submit

Project URL: https://github.com/0xprames/natbench

A Show HN title begins with `Show HN:`. Describe the concrete capability in your
own words: application connectivity regression tests in isolated Linux networks.
For the opening comment, describe your personal reason for building it and a
specific failure you wanted to catch. The technical facts to cover are:

- v0.2.0-alpha.1 runs ordinary executables in Linux network namespaces. Applications
  can use any language and perform their own discovery/traversal.
- JSON scenarios declare readiness, commands, deadlines, and delivery assertions.
  Evidence includes JSON/JUnit, stdout/stderr, timelines, and optional bounded PCAPs.
- The Go demo blocks forwarded UDP. The server is ready, the client times out, and
  delivery assertions fail. A request appears in the client capture but not on
  the WAN. Changing only router A's profile makes the same assertions pass twice.
- A separate Rust example tests fresh data on a persistent client after a service
  outage. Native x86-64/ARM64 Linux binaries and a copyable CI workflow are available.
- Requirements: Linux, root, namespace/network administration privileges, iproute2
  and nftables; capture requires tcpdump. Programs share filesystem/caller privileges.
- Current scope: fixed IPv4 roles and three kernel profiles. Transport comparison
  adapters and normalized performance metrics remain planned.

Walkthrough: https://github.com/0xprames/natbench/blob/main/docs/GETTING_STARTED.md

Useful feedback topics: failures users struggle to reproduce, difficulty adapting
an existing executable, and whether the artifacts explain a failed assertion.
Be available to answer implementation questions and help reproduce results.

## Rust weekly project thread brief

Use the current weekly project thread, linked from the subreddit. Write a short
personal update about what you built or learned. Mention Rust's role as the
implementation language, that applications can be independent executables, the
verified blocked-UDP example, and the Linux/root requirement. Link the repository
and invite an outside evaluation. The facts above support the technical claims;
the personal account should come from the maintainer's own experience.

## Demo verification

The published x86-64 v0.2.0-alpha.1 archive is the demo target. Verify its checksum,
build the included Go example, and run the shipped verifier:

```sh
go build -o examples/udp-echo/udp-echo examples/udp-echo/main.go
sudo scripts/check-application-demo.sh ./natbench ./demo-001
```

The verifier checks the expected assertion failure, outgoing client UDP with no
matching WAN UDP, complete capture manifests, and two passing corrected attempts.
On October 7, 2026, this verifier passed against the published x86-64 alpha on
Linux 6.1.0-53-amd64 after SHA256SUMS verification. Retain the actual artifact
directory locally. These observations describe the
controlled fixture, rather than an arbitrary user's network.
