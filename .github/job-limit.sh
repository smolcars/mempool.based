#!/usr/bin/env bash
# Fails a CI job that ran longer than its JOB_LIMIT seconds, counted from JOB_STARTED (docs/CI.md
# "Budget"). Thirty merges each adding a little took the checks over their budget once; this names
# the job the moment it drifts, instead of the total slowly creeping past three minutes.
set -euo pipefail
took=$(($(date +%s) - JOB_STARTED))
echo "This job took ${took} s of its ${JOB_LIMIT} s limit." >> "$GITHUB_STEP_SUMMARY"
if [ "$took" -gt "$JOB_LIMIT" ]; then
  echo "::error::${GITHUB_JOB} took ${took} s, over its ${JOB_LIMIT} s limit. Make room before merging: docs/CI.md \"When a job goes over\"."
  exit 1
fi
