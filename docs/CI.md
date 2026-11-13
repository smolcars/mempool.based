# CI and testing

## Budget

**A pull request finishes its checks in under 3 minutes with a warm cache.** This is a hard rule, measured as wall-clock time from push to the `ci-ok` check. Runners are GitHub's free hosted runners.

A PR that adds a slower step must make room for it or move it out of the PR path (to a scheduled or post-merge job).

**Each job has a limit of its own,** a little above its time on the day it was set, so a job that drifts is named the moment it does, not after thirty merges have each added a little:

| Job | Limit | Warm, 2026-11-13 |
|---|---|---|
| `rust` | 120 s | JOB_RUST |
| `e2e` | 120 s | JOB_E2E |
| `web` | 60 s | JOB_WEB |

The job's last step, [`.github/job-limit.sh`](../.github/job-limit.sh), writes the job's time to the run's summary and fails a pull request's job that went over, when its Rust cache was a warm, exact hit. A cold run (a new lockfile or toolchain) and a `master` push only warn: failing them would stop `rust-cache` from saving the cache that makes the next run warm. The `rust` and `e2e` jobs also list their ten slowest tests in the summary ([`.github/slowest.py`](../.github/slowest.py), from nextest's and Playwright's JUnit reports).

### When a job goes over

Make room in the same pull request; never raise a limit to pass. In order: shrink or delete a test that re-proves something slowly (a random test runs a few steps in the suite and its long form behind `#[ignore]`, with the command beside it); replace a real wait with an event or a paused clock; stop building what no test needs. Read the summary's slowest tests first. Raise a limit only in a pull request that does nothing else and says why the time is worth it.

## How it stays fast

1. **One workflow, one required check.** `ci.yml` ends in a `ci-ok` job that depends on every other job. Branch protection requires only `ci-ok`, so skipped jobs never block a merge.
2. **Run only what changed.** A first `changes` job maps changed paths to jobs. A docs-only PR runs nothing but `ci-ok`. On `master`, a push that touches code runs every job, which verifies the merge and refreshes the caches; a docs-only push runs nothing but `changes` and `ci-ok`.
3. **Jobs run in parallel,** never chained unless one needs another's output.
4. **Caches are written on `master` and read on PRs.** GitHub scopes caches by branch: a PR can read `master`'s cache but not another PR's. So `master` pushes save caches and PR runs only restore them. The `rust` and `e2e` jobs each cache the cargo registry and `target/`. `rust-cache`'s key leaves out the `[profile]` sections of `Cargo.toml`, so its `key` input names the profile: change it when a profile changes, or every run rebuilds what the profile touches and no run saves it. `bun install` from the registry is faster than restoring a cache, and so is downloading Playwright's headless Chromium, which `e2e` does while it compiles.
5. **Cancel superseded runs.** A new push to a PR cancels the run in progress.
6. **No compiling tools in CI.** Tools are installed as prebuilt binaries.
7. **Slow waits overlap.** The `e2e` job starts the regtest node and the browser download in the background, then compiles while the node mines.
8. **Lean debug builds.** CI builds without full debug info to cut compile time and cache size. One dependency is optimised even there: secp256k1's C library, which at `-O0` made signature checks most of the Lightning tests' time (a test of 1,500 announcements took 6.9 s; optimised, 0.3 s). It builds once into the cache.
9. **The test binaries run at once.** [cargo-nextest](https://nexte.st) runs every test as its own process, eight at a time on the runner's four cores, since most slow tests wait on a socket or a timer ([`.config/nextest.toml`](../.config/nextest.toml)). `cargo test` ran the 28 test binaries one after another, each waiting for its slowest test. nextest is a prebuilt binary checked against a pinned checksum; it adds nothing to the product or the lockfile. Doc tests, which nextest does not run, run after it with `cargo test --doc`. Examples are not built for the tests: clippy already checks they compile.
10. **Browser tests use the runner's four cores:** four Playwright workers.
11. **Every job has a timeout of 10 minutes.** A hung job fails fast; a test past 30 s fails too (nextest's `slow-timeout`).

## Jobs

| Job | Runs when | Does |
|---|---|---|
| `changes` | always | Decides which jobs below run. Pull requests run only the jobs their paths need; a `master` push runs every job when it touches code and none when it touches only docs |
| `rust` | Rust (`crates/`, and the load generator in `dev/load/`), lockfile, `.config/nextest.toml` or workflow changes | Format check, clippy with warnings denied, tests with nextest, doc tests |
| `web` | `web/`, the files the frontend imports from `design/` (`tokens.css`, `logo.svg`, `fonts/`) or workflow changes | Type check (both tsconfigs), `oxlint` with type-aware rules and the `oxfmt` format check, unit tests, production build, bundle-size check |
| `e2e` | anything `rust` or `web` runs for, or `dev/regtest/` | `bun run e2e` ([`FRONTEND.md`](FRONTEND.md#browser-tests)): the server and frontend built from the PR, a regtest node with the known chain, Playwright in headless Chromium |
| `image` | anything `web` runs for, `Dockerfile` or `.dockerignore`; pull requests only | Builds the Docker image's frontend stage, so a Dockerfile that cannot build the frontend fails before merge. The Rust stage is left to the post-merge build |
| `ci-ok` | always | Fails if any job above failed |

Outside the PR path, daily and when a lockfile changes on `master`: the [dependency audit](#dependency-audit) (`audit.yml`). Weekly: the browser tests in [other browsers](#other-browsers) (`browsers.yml`). After merge to `master`, and on a version tag: the [release workflow](#releases) (`release.yml`).

## Dependency audit

`audit.yml` checks what we ship against the advisory databases ([`SECURITY.md`](SECURITY.md) rule 20). It runs daily, on manual dispatch, and on a push to `master` that changes `Cargo.lock`, `web/bun.lock`, `dev/audit/` or the workflow. Never on a pull request: a new advisory can land on any day, whether or not anyone opens a PR, and the PR budget has no room for it.

[`dev/audit/audit.py`](../dev/audit/audit.py) runs both checks:

- `Cargo.lock` against RustSec with `cargo-audit`, a pinned release binary whose checksum the workflow verifies. Every RustSec finding fails it: vulnerabilities, and the `unsound`, `unmaintained` and `yanked` warnings.
- `web/bun.lock`, dev dependencies included, with `bun audit`, at the version `web/package.json` pins. It asks the npm registry, which serves GitHub's advisories; the fixed versions, which `bun audit` does not report, come from GitHub's advisory API.

A clean run takes about 30 s, 13 of them for the audit itself, most of that fetching the advisory database. That is too little to be worth a cache.

**On a finding** the run fails and its `report` job opens one issue, *Dependency audit found advisories*. The issue names each advisory, the package and version, how we depend on it (direct, or the chain of packages that pulls it in), and the fixed versions. While that issue is open, a later run edits it and adds one comment only when the findings change; otherwise it says nothing. A clean run says nothing and closes nothing: close the issue once its findings are fixed or accepted. Closing it with a finding still open brings it back on the next run. The `audit` job holds only a read token; `report`, which runs no third-party code, alone may write issues. If a tool itself fails (the database unreachable, say), the run fails without an issue, and GitHub's failed-run notification is the signal.

**To accept a finding on purpose,** add it to [`dev/audit/ignore.toml`](../dev/audit/ignore.toml) with its id, the reason it does not apply to how we use the package, and the date it was reviewed. An entry without all three fails the audit, so an ignore is never silent. Remove the entry once the dependency is upgraded. A `yanked` release has no advisory id: upgrade it.

**Locally,** from the repository root, with Python 3.11 or later, and [`cargo-audit`](https://github.com/rustsec/rustsec/releases) and `bun` on `PATH`:

```sh
python3 -I dev/audit/audit.py   # exit 0: clean; 1: the issue's Markdown on stdout; 2: a tool failed
```

`CARGO_LOCK=<file>` and `WEB_DIR=<directory with package.json and bun.lock>` point it at a temporary copy, for trying a change to the script.

## Other browsers

`browsers.yml` runs `bun run e2e` in Playwright's WebKit and Firefox, each at desktop and phone width: four jobs at once ([`FRONTEND.md`](FRONTEND.md#other-engines)). It runs every Monday and on manual dispatch (`gh workflow run browsers.yml`), never on a pull request, whose budget has no room for it. Its token reads only. Its jobs restore the cargo cache the `e2e` job saves on `master` and save none of their own; each engine's browser build is cached under the Playwright version, and its system libraries come from apt on every run. Each job times out at 20 minutes. Firefox runs under Xvfb, still headless: without a display on Linux it finds no GL driver and has no WebGL at all, so the transaction grid would only ever show its stand-in.

A failure here means a page broke in an engine Chromium hides, or a test assumed Chromium. It is not Safari on a Mac or an iPhone: Playwright builds its own WebKit, here on Linux, so a green run does not replace a look in real Safari.

## Releases

`release.yml` builds the server's image ([`DEPLOY.md`](DEPLOY.md#reference-the-image)) for linux/amd64 and linux/arm64, each natively on a runner of its architecture, and joins them into one multi-architecture image. Never on a pull request.

| Trigger | Does |
|---|---|
| A push to `master` touching anything that goes into the image, `deploy/` or the workflow; a manual dispatch, on any ref, a tag's included | A dry run: build, check and join both architectures. Publishes nothing |
| A pushed tag `vX.Y.Z` | The same, then publishes the image and creates the release |

| Job | Permissions | Does |
|---|---|---|
| `build`, once per architecture | `contents: read` | Fails a tag that is not `v` and the version in `Cargo.toml` (the server reports that version). `docker build` with the OCI `source`, `revision`, `version` and `licenses` labels. Checks the image runs as 65532, holds no program but the server and no `.env`, `rpc.cookie` or `.git`, and turns healthy started read-only with no capabilities, and that `deploy/compose.yml` validates. Saves the image as an artifact |
| `join` | none | With `skopeo` and `jq`, which GitHub's Ubuntu runners have: copies both images into one OCI layout, gzipping their layers, and writes an image index over them, annotated like the labels. Takes the image for its own platform back out of the index and runs it until healthy. Writes each platform's compressed size to the run's summary, and uploads the layout |
| `publish`, pushed tags only | `packages: write`, `id-token: write`, `attestations: write` | One at a time across all tags. Refuses a version already in the registry, so re-running an old release changes nothing, and stops on any registry error other than "not there". Pushes exactly that layout to `ghcr.io/<owner>/<repo>` as `X.Y.Z`, and as `latest` when it is the newest version there, with its digests unchanged, checks the registry's digest is the joined index's, and attests its provenance with GitHub's `attest-build-provenance` (signed through Sigstore, pushed beside the image) |
| `release`, pushed tags only | `contents: write` | Creates the GitHub release: the image and its digest, how to verify the attestation, a pointer to the upgrade steps, then GitHub's generated list of changes |

The only token is the built-in `GITHUB_TOKEN`; third-party actions are pinned by commit. Every job has a timeout. There is no layer cache between runs: the Dockerfile's cache mounts live only as long as the runner, and a cold build of both architectures fits in a few minutes.

Measured on 2026-11-12, a dry run with no cache: `build` 1 min 47 s on amd64 and 2 min 0 s on arm64, side by side, `join` 13 s, 2 min 22 s from start to end. Each image is about 17 MB compressed.

**To release:** set `version` in the root `Cargo.toml` to the new version in a PR, merge it, then tag that commit `vX.Y.Z` on `master` and push the tag. A pushed tag cannot be taken back quietly: the release is public at once. After the first release, check in the package's settings on GitHub that the image is public and linked to this repository, since a new package can start private, and operators pull it without logging in.

## Test layers

| Layer | Tests | Speed |
|---|---|---|
| Unit | Pure logic: parsing, diffing, projected blocks, fee math, classification. No I/O | Milliseconds |
| Fixture | Decoding and mapping against recorded public chain data in `fixtures/` | Milliseconds |
| Integration | Node clients and the engine against rbitcoin on regtest in Docker ([`REGTEST.md`](REGTEST.md)) | Seconds |
| End to end | The built server and frontend in headless Chromium; WebKit and Firefox weekly | Seconds |

Most coverage belongs in the first two layers. Integration and end-to-end tests cover the paths where the layers meet, not every case.

## Rules for tests

- **Tests never reach the public internet or a shared node.** CI has no access to either.
- **Tests bind port 0** or take their port from the environment.
- **A flaky test is a failing test.** Fix it or delete it; do not retry it.
- **A test never passes or fails on how long something took on the machine that runs it.** CI's runner is shared and slow, and a busy laptop is slower: a bound that held on an idle machine fails there for no fault of the code. Wait for the event the test means (a reply, a log line, a gate the test opens), or run it on tokio's paused clock (`#[tokio::test(start_paused = true)]`), where a timer's spacing is exact. A time budget the code must meet is a benchmark: put it behind `#[ignore]` with the command to run it, and keep a test of the same work's answers, without the clock, in the suite. A lower bound on a real clock (it waited at least the timeout) is fine; an upper bound is a budget.
- **Browser tests run at the server's production limits** and fail on any refusal, naming the limit and the request ([`FRONTEND.md`](FRONTEND.md#browser-tests)). A test that trips one has found a page that asks for too much, or a test that does; never raise a limit to make it pass.
- **New logic with a branch, a loop or a parser ships with a test.**

## Measuring

When a PR changes the workflow, its description states the cold and warm wall-clock time of the run.

Measured 2026-11-12, from the start of the run to `ci-ok`. Warm reads the caches a `master` push wrote; cold has no `e2e` cache. The `rust` job's cache came from `master` in both.

| A PR that touches | Jobs | Warm | Cold |
|---|---|---|---|
| Docs only | `changes`, `ci-ok` | 10 s | 10 s (no cache involved) |
| Rust only | `rust`, `e2e` | 63 s | 50 s |
| The frontend only | `web`, `e2e`, `image` | 46 s | 58 s |
| Everything | `rust`, `web`, `e2e`, `image` | 60 s | 67 s |

Runs vary by about 10 s, more than the caches save. The table predates the mempool history page; measured again on 2026-11-12, a warm pull request that touched everything took 125 s. `rust` was the longest job at 111 s, then `e2e` at 77 s, `web` at 35 s and `image` at 20 s. Inside `e2e`, `rustup toolchain install` took 8 s and restoring the cargo cache 2 s, then `bun run e2e` took 61 s. Within that, the build was ready at 16 s, the regtest node (pulling its image and building the scenario's chain) at 19 s, and the 67 tests took 40 s on the runner's 2 workers. The node is the critical path before the tests. No browser test waits for the server to record a live minute: the store starts with history in it ([`FRONTEND.md`](FRONTEND.md#adding-a-test)).
