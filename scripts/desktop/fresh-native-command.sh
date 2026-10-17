#!/usr/bin/env bash
# Temporary #943 qualification: run GitHub's original command file unchanged.
# Retain its exact exit status even when diagnostic publication fails.
set +e
diagnostic_path="$RUNNER_TEMP/nessa-fresh-native-command.log"
bash --noprofile --norc -e -o pipefail "$1" 2>&1 | tee "$diagnostic_path"
command_status=${PIPESTATUS[0]}
if [ "$command_status" -ne 0 ]; then
  if [ "$RUNNER_OS" = "Windows" ]; then
    python scripts/desktop/fresh-native-diagnostic.py "$diagnostic_path"
  else
    python3 scripts/desktop/fresh-native-diagnostic.py "$diagnostic_path"
  fi
fi
exit "$command_status"
