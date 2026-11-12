# CI and testing

## Budget

**A pull request finishes its checks in under 3 minutes with a warm cache.** This is a hard rule, measured as wall-clock time from push to the `ci-ok` check. Runners are GitHub's free hosted runners.

A PR that adds a slower step must make room for it or move it out of the PR path (to a scheduled or post-merge job).

## How it stays fast

1. **One workflow, one required check.** `ci.yml` ends in a `ci-ok` job that depends on every other job. Branch protection requires only `ci-ok`, so skipped jobs never block a merge.
2. **Run only what changed.** A first `changes` job maps changed paths to jobs. A docs-only PR runs nothing but `ci-ok`. On `master`, a push that touches code runs every job, which verifies the merge and refreshes the caches; a docs-only push runs nothing but `changes` and `ci-ok`.
3. **Jobs run in parallel,** never chained unless one needs another's output.
4. **Caches are written on `master` and read on PRs.** GitHub scopes caches by branch: a PR can read `master`'s cache but not another PR's. So `master` pushes save caches and PR runs only restore them. The `rust` and `e2e` jobs each cache the cargo registry and `target/`. `bun install` from the registry is faster than restoring a cache, and so is downloading Playwright's headless Chromium, which `e2e` does while it compiles.
5. **Cancel superseded runs.** A new push to a PR cancels the run in progress.
6. **No compiling tools in CI.** Tools are installed as prebuilt binaries.
7. **Slow waits overlap.** The `e2e` job starts the regtest node and the browser download in the background, then compiles while the node mines.
8. **Lean debug builds.** CI builds without full debug info to cut compile time and cache size.
9. **Every job has a timeout of 10 minutes.** A hung job fails fast.

## Jobs

| Job | Runs when | Does |
|---|---|---|
| `changes` | always | Decides which jobs below run. Pull requests run only the jobs their paths need; a `master` push runs every job when it touches code and none when it touches only docs |
| `rust` | Rust, lockfile or workflow changes | Format check, clippy with warnings denied, tests |
| `web` | `web/`, the files the frontend imports from `design/` (`tokens.css`, `logo.svg`, `fonts/`) or workflow changes | Type check (both tsconfigs), `oxlint` with type-aware rules and the `oxfmt` format check, unit tests, production build, bundle-size check |
| `e2e` | anything `rust` or `web` runs for, or `dev/regtest/` | `bun run e2e` ([`FRONTEND.md`](FRONTEND.md#browser-tests)): the server and frontend built from the PR, a regtest node with the known chain, Playwright in headless Chromium |
| `image` | anything `web` runs for, `Dockerfile` or `.dockerignore`; pull requests only | Builds the Docker image's frontend stage, so a Dockerfile that cannot build the frontend fails before merge. The Rust stage is left to the post-merge build |
| `ci-ok` | always | Fails if any job above failed |

Outside the PR path, daily and when a lockfile changes on `master`: the [dependency audit](#dependency-audit) (`audit.yml`). After merge to `master`: the full Docker image build on both architectures (`image.yml`).

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

## Test layers

| Layer | Tests | Speed |
|---|---|---|
| Unit | Pure logic: parsing, diffing, projected blocks, fee math, classification. No I/O | Milliseconds |
| Fixture | Decoding and mapping against recorded public chain data in `fixtures/` | Milliseconds |
| Integration | Node clients and the engine against rbitcoin on regtest in Docker ([`REGTEST.md`](REGTEST.md)) | Seconds |
| End to end | The built server and frontend in headless Chromium | Seconds |

Most coverage belongs in the first two layers. Integration and end-to-end tests cover the paths where the layers meet, not every case.

## Rules for tests

- **Tests never reach the public internet or a shared node.** CI has no access to either.
- **Tests bind port 0** or take their port from the environment.
- **A flaky test is a failing test.** Fix it or delete it; do not retry it.
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
