"""Keep a non-reaping PID 1 alive until evidence is collected.

Popen.wait targets only the direct libtest child. No SIGCHLD handler or
waitpid(-1) is installed; Docker --init supplies the positive reaping case.
"""

import glob
import json
import os
import subprocess
import sys
import time


def processes():
    result = {}
    for filename in glob.glob('/proc/[0-9]*/stat'):
        try:
            with open(filename, encoding='utf-8') as stat_file:
                text = stat_file.read()
            pid = int(text.split(' ', 1)[0])
            fields = text.rsplit(')', 1)[1].split()
            result[pid] = {
                'pid': pid,
                'state': fields[0],
                'ppid': int(fields[1]),
                'pgid': int(fields[2]),
            }
        except (OSError, ValueError, IndexError):
            # /proc entries can disappear between listing and reading.
            continue
    return result


def packages():
    result = {}
    with open('/var/lib/dpkg/status', encoding='utf-8') as status:
        for paragraph in status.read().split('\n\n'):
            fields = dict(
                line.split(': ', 1) for line in paragraph.splitlines()
                if ': ' in line and not line.startswith(' ')
            )
            if fields.get('Package') in ('python3-minimal', 'libgcc-s1', 'ca-certificates'):
                result[fields['Package']] = fields.get('Version')
    return result


def main():
    test_name = sys.argv[1]
    before = processes()
    # A container-local file avoids output-pipe capacity blocking the child.
    with open('/tmp/libtest-output', 'w+', encoding='utf-8') as output:
        child = subprocess.Popen(
            ['/probe/nessa-sdk-tests', '--exact', test_name, '--nocapture', '--test-threads=1'],
            stdout=output,
            stderr=subprocess.STDOUT,
        )
        timed_out = False
        try:
            exit_code = child.wait(timeout=20)
        except subprocess.TimeoutExpired:
            timed_out = True
            child.kill()
            exit_code = child.wait(timeout=2)
        # Tini needs time to collect adopted descendants after the direct test exits.
        time.sleep(1)
        after = processes()
        output.seek(0)
        captured_output = output.read(64 * 1024 + 1)
        report = {
            'test': test_name,
            'supervisor_pid': os.getpid(),
            'supervisor_ppid': os.getppid(),
            'initial_processes': list(before.values()),
            'pid1_executable': os.readlink('/proc/1/exe'),
            'test_pid': child.pid,
            'test_exit': exit_code,
            'timed_out': timed_out,
            'output': captured_output[:64 * 1024],
            'output_truncated': len(captured_output) > 64 * 1024,
            'new_orphans': [p for pid, p in after.items() if pid not in before and p['ppid'] == 1],
            'retained_directories': sorted(glob.glob('/tmp/nessa-agent-*')),
            'packages': packages(),
        }
        print(json.dumps(report), flush=True)
        # The report was captured while PID 1 was still alive.
        time.sleep(1)


if __name__ == '__main__':
    main()
