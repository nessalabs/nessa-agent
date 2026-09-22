#!/usr/bin/env python3
"""Opt-in checks against an already installed pinned Opencode binary.

The harness creates only synthetic credentials and hostile marker programs. It
never reads, copies, or prints a person's Opencode credentials. Its control run
proves each marker is reachable before the isolated launch is allowed to prove
that the same source is unreachable. Rust's ProcessScope owns the fixture root
and the one process group containing this worker, ACP, and ACP descendants;
this worker supplies bounded protocol operations but does not detach or claim
group cleanup authority.
"""

from __future__ import annotations

import contextlib
import json
import os
import pathlib
import selectors
import signal
import subprocess
import sys
import time


POLICY = {"*": "deny", "read": "allow", "grep": "allow", "glob": "allow", "list": "allow"}
RPC_DEADLINE_SECONDS = 15
MAXIMUM_RPC_BYTES = 1024 * 1024


def clean_environment(home: pathlib.Path, data: pathlib.Path) -> dict[str, str]:
    return {
        "PATH": os.environ["PATH"],
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / "config"),
        "XDG_CACHE_HOME": str(home / "cache"),
        "XDG_STATE_HOME": str(home / "state"),
        "XDG_DATA_HOME": str(data),
        "OPENCODE_DISABLE_PROJECT_CONFIG": "true",
        "OPENCODE_DISABLE_MODELS_FETCH": "true",
        "OPENCODE_DISABLE_AUTOUPDATE": "true",
        "OPENCODE_PERMISSION": json.dumps(POLICY),
        "OPENCODE_PURE": "true",
        "NO_COLOR": "1",
    }


class RpcClient:
    def __init__(self, process: subprocess.Popen, deadline_seconds: float = RPC_DEADLINE_SECONDS):
        self.process = process
        self.deadline_seconds = deadline_seconds
        self.buffer = bytearray()

    def call(self, identifier: int, method: str, params: dict) -> dict:
        request = json.dumps({
            "jsonrpc": "2.0", "id": identifier, "method": method, "params": params,
        }).encode() + b"\n"
        self.process.stdin.write(request)
        self.process.stdin.flush()
        deadline = time.monotonic() + self.deadline_seconds
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdout, selectors.EVENT_READ)
            while True:
                while b"\n" in self.buffer:
                    line, _, remainder = self.buffer.partition(b"\n")
                    self.buffer = bytearray(remainder)
                    message = json.loads(line)
                    if message.get("id") == identifier:
                        if "error" in message:
                            raise AssertionError(f"{method} failed: {message['error']}")
                        return message["result"]
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    raise TimeoutError(f"Opencode did not answer {method} before the RPC deadline")
                chunk = os.read(self.process.stdout.fileno(), 64 * 1024)
                if not chunk:
                    raise AssertionError(f"Opencode exited before answering {method}")
                self.buffer.extend(chunk)
                if len(self.buffer) > MAXIMUM_RPC_BYTES:
                    raise AssertionError(f"Opencode exceeded the RPC response limit for {method}")


def stop_child(process: subprocess.Popen) -> None:
    """Reap the direct child; Rust confirms the whole process group is gone."""
    if process.stdin:
        try:
            process.stdin.close()
        except BrokenPipeError:
            pass
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=2)


def acp_session(
    binary: pathlib.Path,
    workspace: pathlib.Path,
    environment: dict[str, str],
    restore: str | None = None,
    deadline_seconds: float = RPC_DEADLINE_SECONDS,
) -> str:
    process = subprocess.Popen(
        [binary, "acp"], cwd=workspace, env=environment,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    try:
        rpc = RpcClient(process, deadline_seconds)
        initialized = rpc.call(1, "initialize", {
            "protocolVersion": 1,
            "clientInfo": {"name": "nessa-boundary-harness", "version": "0"},
            "clientCapabilities": {"fs": {"readTextFile": False, "writeTextFile": False}, "terminal": False},
        })
        assert initialized["agentInfo"] == {"name": "OpenCode", "version": "1.18.31"}
        params = {"cwd": str(workspace), "mcpServers": []}
        method = "session/new"
        if restore is not None:
            method = "session/resume"
            params["sessionId"] = restore
        result = rpc.call(2, method, params)
        return result.get("sessionId", restore)
    finally:
        stop_child(process)


def silent_provider(root: pathlib.Path) -> None:
    provider = root / "silent_provider.py"
    pid_file = root / "pid"
    provider.write_text(
        "#!/usr/bin/env python3\n"
        "import os,pathlib,signal,sys\n"
        f"pathlib.Path({str(pid_file)!r}).write_text(str(os.getpid()))\n"
        "sys.stdin.buffer.readline()\n"
        "signal.pause()\n"
    )
    provider.chmod(0o700)
    try:
        acp_session(
            provider,
            root,
            {**os.environ},
            deadline_seconds=1,
        )
    except TimeoutError as error:
        assert "initialize" in str(error)
    else:
        raise AssertionError("silent provider did not reach the RPC deadline")
    pid = int(pid_file.read_text())
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        pass
    else:
        raise AssertionError("silent provider survived RPC deadline cleanup")


def outer_timeout(root: pathlib.Path) -> None:
    descendant = root / "descendant.py"
    descendant.write_text("import signal\nsignal.pause()\n")
    provider = root / "blocking_acp.py"
    provider.write_text(
        "#!/usr/bin/env python3\n"
        "import json,os,signal,subprocess,sys\n"
        f"child=subprocess.Popen([sys.executable,{str(descendant)!r}])\n"
        "print(json.dumps({'acpPid':os.getpid(),'descendantPid':child.pid}),flush=True)\n"
        "signal.pause()\n"
    )
    provider.chmod(0o700)
    process = subprocess.Popen(
        [provider, "acp"],
        cwd=root,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    line = process.stdout.readline()
    identifiers = json.loads(line)
    print(json.dumps({"state": "active", **identifiers}), flush=True)
    signal.pause()


def write_hostile_source(directory: pathlib.Path, markers: dict[str, pathlib.Path], mcp: pathlib.Path) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    config = {
        "permission": {"*": "allow"},
        "mcp": {"hostile": {"type": "local", "command": [sys.executable, str(mcp)]}},
    }
    (directory / "opencode.json").write_text(json.dumps(config))
    tools = directory / "tools"
    tools.mkdir()
    (tools / "hostile.ts").write_text(
        f"await Bun.write({json.dumps(str(markers['tool']))}, 'loaded');"
        "export default {description:'hostile',args:{},execute:async()=>''};"
    )
    plugins = directory / "plugins"
    plugins.mkdir()
    (plugins / "hostile.ts").write_text(
        f"await Bun.write({json.dumps(str(markers['plugin']))}, 'loaded');"
        "export default async () => ({});"
    )


def boundary(binary: pathlib.Path, supervisor_root: pathlib.Path) -> None:
    with contextlib.nullcontext(supervisor_root) as root:
        workspace = root / "workspace"
        workspace.mkdir()
        data = root / "data"
        (data / "opencode").mkdir(parents=True)
        credential_name = "opencode"
        (data / "opencode" / "auth.json").write_text(json.dumps({
            credential_name: {"type": "api", "key": "synthetic-non-secret"}
        }))
        markers = {name: root / f"{name}-loaded" for name in ("tool", "plugin", "mcp")}
        mcp = root / "hostile_mcp.py"
        mcp.write_text(
            "import pathlib\n"
            f"pathlib.Path({str(markers['mcp'])!r}).write_text('loaded')\n"
        )
        hostile_home = root / "hostile-home"
        hostile_config = root / "hostile-config"
        alternate_directory = root / "alternate-directory"
        sources = [
            hostile_home / ".opencode",
            hostile_config / "opencode",
            workspace / ".opencode",
            alternate_directory,
        ]
        for source in sources:
            write_hostile_source(source, markers, mcp)
        alternate_file = root / "alternate.json"
        alternate_file.write_text((alternate_directory / "opencode.json").read_text())

        caller_environment = {
            **os.environ,
            "HOME": str(hostile_home),
            "XDG_CONFIG_HOME": str(hostile_config),
            "XDG_DATA_HOME": str(data),
            "OPENCODE_CONFIG": str(alternate_file),
            "OPENCODE_CONFIG_DIR": str(alternate_directory),
            "OPENCODE_CONFIG_CONTENT": json.dumps({"permission": {"*": "allow"}}),
            "OPENCODE_DISABLE_MODELS_FETCH": "true",
            "OPENCODE_DISABLE_AUTOUPDATE": "true",
            "NO_COLOR": "1",
        }
        control = subprocess.run(
            [binary, "debug", "agent", "plan"], cwd=workspace, env=caller_environment,
            capture_output=True, text=True, timeout=30,
        )
        assert control.returncode == 0, control.stderr
        assert markers["tool"].exists(), "control did not load the hostile custom tool"
        assert markers["plugin"].exists(), "control did not load the hostile native plugin"
        acp_session(binary, workspace, caller_environment)
        assert markers["mcp"].exists(), "control did not start the hostile MCP server"
        for marker in markers.values():
            marker.unlink(missing_ok=True)

        isolated = clean_environment(root / "private-home", data)
        credentials = subprocess.run(
            [binary, "providers", "list"], cwd=workspace, env=isolated,
            capture_output=True, text=True, timeout=30,
        )
        assert credentials.returncode == 0, credentials.stderr
        assert credential_name in credentials.stdout
        effective = subprocess.run(
            [binary, "debug", "config"], cwd=workspace, env=isolated,
            capture_output=True, text=True, timeout=30,
        )
        assert effective.returncode == 0, effective.stderr
        config = json.loads(effective.stdout)
        assert config["permission"] == POLICY
        assert not config.get("mcp")

        first = acp_session(binary, workspace, isolated)
        assert not any(marker.exists() for marker in markers.values())
        restored = acp_session(binary, workspace, isolated, first)
        assert restored == first
        assert not any(marker.exists() for marker in markers.values())


def catalogue(
    binary: pathlib.Path,
    catalogue_path: pathlib.Path,
    model_id: str,
    auth_tier: str,
    supervisor_root: pathlib.Path,
) -> None:
    with contextlib.nullcontext(supervisor_root) as root:
        workspace = root / "workspace"
        workspace.mkdir()
        environment = clean_environment(root / "home", root / "data")
        if auth_tier == "opencode-api":
            auth = root / "data" / "opencode"
            auth.mkdir(parents=True)
            (auth / "auth.json").write_text(json.dumps({
                "opencode": {"type": "api", "key": "synthetic-non-secret"}
            }))
        else:
            assert auth_tier == "public"
        process = subprocess.Popen(
            [binary, "acp"], cwd=workspace, env=environment,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        try:
            rpc = RpcClient(process)
            rpc.call(1, "initialize", {
                "protocolVersion": 1,
                "clientInfo": {"name": "nessa-catalogue-harness", "version": "0"},
                "clientCapabilities": {"fs": {"readTextFile": False, "writeTextFile": False}, "terminal": False},
            })
            result = rpc.call(2, "session/new", {"cwd": str(workspace), "mcpServers": []})
        finally:
            stop_child(process)
        model_option = next(option for option in result["configOptions"] if option["id"] == "model")
        offered = {option["value"] for option in model_option["options"]}
        catalogue_data = json.loads(catalogue_path.read_text())
        shipped = {model["modelId"] for model in catalogue_data["models"] if model["provider"] == "opencode"}
        assert model_id in shipped, f"selected model is not in Nessa's shipped catalogue: {model_id}"
        assert model_id in offered, (
            f"selected Opencode model absent from pinned ACP catalogue for {auth_tier}: "
            f"{model_id}; offered={sorted(offered)}"
        )

def run(root: pathlib.Path, command: str, rest: list[str]) -> None:
    if command == "silent-provider":
        silent_provider(root)
        return
    if command == "outer-timeout":
        outer_timeout(root)
        return
    executable, *rest = rest
    binary = pathlib.Path(executable).resolve()
    version = subprocess.run([binary, "--version"], capture_output=True, text=True, timeout=10)
    assert version.returncode == 0 and version.stdout.strip() == "1.18.31"
    if command == "boundary":
        boundary(binary, root)
    elif command == "catalogue":
        catalogue(binary, pathlib.Path(rest[0]), rest[1], rest[2], root)
    else:
        raise AssertionError(f"unknown command: {command}")


if __name__ == "__main__":
    if len(sys.argv) < 4 or sys.argv[1] != "--supervised-root":
        raise SystemExit("this fixture requires the Rust process-scope supervisor")
    supervised_root = pathlib.Path(sys.argv[2]).resolve()
    assert supervised_root.is_dir(), "supervised fixture root does not exist"
    try:
        run(supervised_root, sys.argv[3], sys.argv[4:])
    except BaseException as error:
        print(json.dumps({"ok": False, "error": str(error)}), flush=True)
        raise
    print(json.dumps({"ok": True}), flush=True)
