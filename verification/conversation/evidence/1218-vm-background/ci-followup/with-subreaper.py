"""Linux test-process init: reap only this command's orphaned descendants.

Never signal children while the command runs. Forward cancellation to its own
session. After command exit, allow five seconds to drain, then report a leak
and clean only children adopted by this wrapper. Preserve command failures.
"""
import ctypes
import os
import pathlib
import signal
import sys
import time

assert sys.platform == 'linux' and len(sys.argv) > 1
libc = ctypes.CDLL(None, use_errno=True)
if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER, linux/prctl.h
    raise OSError(ctypes.get_errno(), 'PR_SET_CHILD_SUBREAPER')
child = os.fork()
if child == 0:
    try:
        os.setsid()
        os.execvp(sys.argv[1], sys.argv[1:])
    except BaseException:
        os._exit(127)

def forward(signum, _frame):
    try:
        os.killpg(child, signum)
    except ProcessLookupError:
        pass

signal.signal(signal.SIGTERM, forward)
signal.signal(signal.SIGINT, forward)
reaped = 0
while True:
    pid, status = os.waitpid(-1, 0)
    if pid == child:
        code = os.waitstatus_to_exitcode(status)
        break
    reaped += 1

def drain():
    global reaped
    while True:
        try:
            pid, _ = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return True
        if pid == 0:
            return False
        reaped += 1

until = time.monotonic() + 5
while not drain() and time.monotonic() < until:
    time.sleep(.02)
if not drain():
    print('subreaper: live descendants after command exit; qualification failed', file=sys.stderr)
    code = code or 1
    for signum in [signal.SIGTERM, signal.SIGKILL]:
        children = pathlib.Path(f'/proc/self/task/{os.getpid()}/children').read_text().split()
        for value in children:
            pid = int(value)
            assert pid > 1 and pid != os.getpid()
            try:
                os.kill(pid, signum)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 2
        while not drain() and time.monotonic() < deadline:
            time.sleep(.02)
print(f'subreaper: reaped {reaped} orphaned descendants', file=sys.stderr)
sys.exit(code if code >= 0 else 128 - code)
