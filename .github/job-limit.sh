#!/usr/bin/env bash
# Fails a pull request's CI job that ran longer than its JOB_LIMIT seconds, counted from
# JOB_STARTED (docs/CI.md "Budget"). Thirty merges each adding a little took the checks over their
# budget once; this names the job the moment it drifts, instead of letting the total creep.
# The budget holds with a warm cache (WARM=true), so a cold run, or a master push, only warns:
# failing those would also stop rust-cache from saving the cache that makes the next run warm.
set -euo pipefail
took=$(($(date +%s) - JOB_STARTED))
echo "This job took ${took} s of its ${JOB_LIMIT} s limit." >> "$GITHUB_STEP_SUMMARY"
[ "$took" -le "$JOB_LIMIT" ] && exit 0
msg="${GITHUB_JOB} took ${took} s, over its ${JOB_LIMIT} s limit"
if [ "$GITHUB_EVENT_NAME" = pull_request ] && [ "${WARM:-}" = true ]; then
  echo "::error::${msg}. Make room before merging: docs/CI.md \"When a job goes over\"."
  exit 1
fi
echo "::warning::${msg}, with a cold cache or on a push: not enforced (docs/CI.md \"Budget\")."
