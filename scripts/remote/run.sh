#!/usr/bin/env bash
# Runs on the build sandbox: one command for scripts/remote/boat.sh exec (#358).
#
# The SSH session has a terminal so that Ctrl-C and a dropped connection reach
# this side. The command must not inherit it as its controlling terminal: a
# shell it starts with -i then waits on that terminal, which is how the
# login-shell tests hung there and nowhere else. So the command runs in a
# session of its own (setsid, no controlling terminal), and this script forwards
# the interrupt, hangup, or termination to the whole of it. Its output goes
# through a pipe: a terminal that is not the process's controlling one leaves
# cargo-nextest printing nothing at all. Colour is asked for explicitly.
set -uo pipefail
export CARGO_TERM_COLOR=always FORCE_COLOR=1

setsid "$@" </dev/null > >(cat) 2>&1 &
command_pid=$!
trap 'kill -TERM -"${command_pid}" 2>/dev/null' HUP INT TERM

status=0
while kill -0 "${command_pid}" 2>/dev/null; do
  wait "${command_pid}"
  status=$?
done
exit "${status}"
