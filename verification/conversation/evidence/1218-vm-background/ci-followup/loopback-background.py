"""Disposable Linux OpenSSH/helper qualification; never connects off loopback."""
import contextlib
import json
import os
import pathlib
import shlex
import signal
import socket
import subprocess
import tempfile
import sys
import time

BINARY = pathlib.Path(sys.argv[1]).resolve()
ROOT = pathlib.Path(sys.argv[2]).resolve()

def wait_for(predicate, seconds=15):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        if predicate():
            return
        time.sleep(.05)
    raise AssertionError('condition did not hold within deadline')

def live(pid):
    try:
        return pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0] != 'Z'
    except FileNotFoundError:
        return False

@contextlib.contextmanager
def owned_process(argv, **kwargs):
    child = subprocess.Popen(argv, **kwargs)
    try:
        yield child
    finally:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=10)

@contextlib.contextmanager
def recorded_processes(*files):
    """Fallback cleanup happens after assertions, never as their evidence."""
    try:
        yield
    finally:
        pids = []
        for file in files:
            if file.exists():
                pids.extend(int(value) for value in file.read_text().split())
        if not pids:
            print('cleanup unconfirmed: remote process identity was never recorded', file=sys.stderr)
        for pid in pids:
            assert pid > 1
            if live(pid):
                os.kill(pid, signal.SIGKILL)
            wait_for(lambda: not live(pid), seconds=5)

ROOT.mkdir(mode=0o700)
try:
    with tempfile.TemporaryDirectory(prefix='background-', dir=ROOT) as directory:
        p = pathlib.Path(directory)
        for name in ['host', 'client']:
            subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(p/name)], check=True)
        (p/'authorized_keys').write_bytes((p/'client.pub').read_bytes())
        (p/'authorized_keys').chmod(0o600)
        user = subprocess.check_output(['id', '-un'], text=True).strip()
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        (p/'known_hosts').write_text(f'[127.0.0.1]:{port} ' + (p/'host.pub').read_text())
        (p/'sshd_config').write_text(f'''ListenAddress 127.0.0.1
Port {port}
HostKey {p/'host'}
PidFile {p/'pid'}
AuthorizedKeysFile {p/'authorized_keys'}
StrictModes yes
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
AllowUsers {user}
AllowAgentForwarding no
AllowTcpForwarding no
X11Forwarding no
PermitTunnel no
LogLevel ERROR
''')
        ssh = ['ssh', '-F', '/dev/null', '-T', '-p', str(port), '-i', str(p/'client'),
               '-o', 'IdentityAgent=none', '-o', 'IdentitiesOnly=yes', '-o', 'BatchMode=yes',
               '-o', 'StrictHostKeyChecking=yes', '-o', f'UserKnownHostsFile={p/"known_hosts"}',
               '-o', 'GlobalKnownHostsFile=/dev/null', f'{user}@127.0.0.1']
        with open(p/'server.log', 'w') as log, owned_process(
            ['/usr/sbin/sshd', '-D', '-e', '-f', str(p/'sshd_config')],
            stdout=subprocess.DEVNULL, stderr=log,
        ) as server:
            def listening():
                assert server.poll() is None, 'sshd exited'
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=.2):
                        return True
                except OSError:
                    return False
            wait_for(listening)
            # Negative control: SSH loss alone must not explain scope cleanup.
            control_file = p/'control.pids'
            raw = 'sleep 60 & child=$!; printf "%s %s" "$$" "$child" > ' + shlex.quote(str(control_file)) + '; wait "$child"'
            with recorded_processes(control_file), owned_process(ssh + ['exec bash --noprofile --norc -c ' + shlex.quote(raw)],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as client:
                wait_for(lambda: control_file.exists() and control_file.stat().st_size > 0)
                control_pids = [int(pid) for pid in control_file.read_text().split()]
                assert all(live(pid) for pid in control_pids)
                client.kill()
                client.wait(timeout=5)
                time.sleep(1)
                assert live(control_pids[1]), 'SSH itself cleaned the raw descendant; disconnect evidence is inconclusive'
            print('ok negative control: raw SSH descendant survives connection loss; explicitly cleaned', flush=True)
            for kind in ['exit', 'timeout', 'stop', 'eof', 'disconnect']:
                cwd = p/kind
                cwd.mkdir()
                pid_file = cwd/'child.pid'
                command_file = cwd/'command.pid'
                grandchild_file = cwd/'grandchild.pid'
                helper_file = cwd/'helper.pid'
                command = 'printf result; printf err >&2; exit 7' if kind == 'exit' else 'sleep 60 & child=$!; printf %s "$child" > child.pid; wait "$child"'
                if kind in ['eof', 'disconnect']:
                    branch = 'sleep 60 & leaf=$!; printf %s "$leaf" > grandchild.pid; wait "$leaf"'
                    command = 'printf %s "$$" > command.pid; bash -c ' + shlex.quote(branch) + ' & child=$!; printf %s "$child" > child.pid; wait "$child"'
                argv = [str(BINARY), 'background-run', '--cwd', str(cwd), '--timeout',
                        '1' if kind == 'timeout' else '30', '--', 'bash', '--noprofile', '--norc', '-c', command]
                remote = 'printf %s "$$" > ' + shlex.quote(str(helper_file)) + '; exec env NESSA_STAGE=ci NESSA_DATA_DIR=' + shlex.quote(str(cwd)) + ' ' + shlex.join(argv)
                with recorded_processes(grandchild_file, pid_file, command_file, helper_file), owned_process(ssh + [remote], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as client:
                    observed = []
                    if kind != 'exit':
                        files = [command_file, pid_file, grandchild_file] if kind in ['eof', 'disconnect'] else [pid_file]
                        for file in files:
                            wait_for(lambda: file.exists() and file.stat().st_size > 0)
                            observed.append(int(file.read_text()))
                        assert all(live(pid) for pid in observed), 'command never ran'
                    if kind == 'stop':
                        client.stdin.write(b'\1')
                        client.stdin.flush()
                    if kind == 'eof':
                        client.stdin.close()
                    if kind == 'disconnect':
                        client.kill()
                        client.wait(timeout=5)
                        wait_for(lambda: all(not live(pid) for pid in observed))
                        print('ok disconnect: SSH client killed; command, child and grandchild cleaned', flush=True)
                        continue
                    # Keep stdin open while waiting: completion must not depend on EOF.
                    wait_for(lambda: client.poll() is not None)
                    stdout = client.stdout.read(1_000_001)
                    stderr = client.stderr.read(1_000_001)
                    assert client.returncode == 0, stderr
                    assert len(stdout) <= 1_000_000
                    value = json.loads(stdout)
                    expected = {'exit': 'exited', 'timeout': 'timed_out', 'stop': 'cancelled', 'eof': 'cancelled'}[kind]
                    assert value['cause'] == expected, value
                    assert value['cleanup_verified'] is True, value
                    if kind == 'exit':
                        assert (value['exit_code'], value['stdout'], value['stderr']) == (7, 'result', 'err'), value
                    else:
                        wait_for(lambda: all(not live(pid) for pid in observed))
                    input_state = 'SSH stdin closed; command, child and grandchild cleaned' if kind == 'eof' else 'stdin held open'
                    print(f'ok {kind}: {expected}; cleanup_verified=true; {input_state}', flush=True)
finally:
    ROOT.rmdir()
