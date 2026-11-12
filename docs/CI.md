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
| `web` | `web/`, the files the frontend imports from `design/` (`tokens.css`, `logo.svg`, `fonts/`) or workflow changes | Type check, lint, unit tests, production build, bundle-size check |
| `e2e` | anything `rust` or `web` runs for, or `dev/regtest/` | `bun run e2e` ([`FRONTEND.md`](FRONTEND.md#browser-tests)): the server and frontend built from the PR, a regtest node with the known chain, Playwright in headless Chromium |
| `image` | anything `web` runs for, `Dockerfile` or `.dockerignore`; pull requests only | Builds the Docker image's frontend stage, so a Dockerfile that cannot build the frontend fails before merge. The Rust stage is left to the post-merge build |
| `ci-ok` | always | Fails if any job above failed |

Outside the PR path, on a schedule and when lockfiles change: dependency audit against the advisory database. After merge to `master`: the full Docker image build on both architectures (`image.yml`).

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

Runs vary by about 10 s, more than the caches save. The longest job is `e2e`, at 35 to 55 s. Inside it, a warm build is ready after 5 s and a cold one after about 20 s, but the regtest node is ready only after about 20 s, pulling its image and building the scenario's chain. The node is the critical path, then `rustup toolchain install` (7 s) and restoring the cargo cache (4 s). `rust` takes about 30 s, `image` 15 s and `web` 12 s.
