#!/usr/bin/env python3
"""Opt-in checks against an already installed pinned Opencode binary.

The harness creates only synthetic credentials and hostile marker programs. It
never reads, copies, or prints a person's Opencode credentials. Its control run
proves each marker is reachable before the isolated launch is allowed to prove
that the same source is unreachable.
"""

from __future__ import annotations

import json
import os
import pathlib
import signal
import subprocess
import sys
import tempfile


POLICY = {"*": "deny", "read": "allow", "grep": "allow", "glob": "allow", "list": "allow"}


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


def rpc(process: subprocess.Popen, identifier: int, method: str, params: dict) -> dict:
    process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": identifier, "method": method, "params": params}) + "\n")
    process.stdin.flush()
    for line in process.stdout:
        message = json.loads(line)
        if message.get("id") == identifier:
            if "error" in message:
                raise AssertionError(f"{method} failed: {message['error']}")
            return message["result"]
    raise AssertionError(f"Opencode exited before answering {method}")


def acp_session(binary: pathlib.Path, workspace: pathlib.Path, environment: dict[str, str], restore: str | None = None) -> str:
    process = subprocess.Popen(
        [binary, "acp"], cwd=workspace, env=environment,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, start_new_session=True,
    )
    try:
        initialized = rpc(process, 1, "initialize", {
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
        result = rpc(process, 2, method, params)
        return result.get("sessionId", restore)
    finally:
        if process.stdin:
            process.stdin.close()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)


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


def boundary(binary: pathlib.Path) -> None:
    with tempfile.TemporaryDirectory(prefix="nessa-opencode-live-") as temporary:
        root = pathlib.Path(temporary)
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
            "import pathlib, time\n"
            f"pathlib.Path({str(markers['mcp'])!r}).write_text('loaded')\n"
            "time.sleep(30)\n"
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
            "NO_COLOR": "1",
        }
        control = subprocess.run(
            [binary, "debug", "agent", "plan"], cwd=workspace, env=caller_environment,
            capture_output=True, text=True, timeout=30,
        )
        assert control.returncode == 0, control.stderr
        assert markers["tool"].exists(), "control did not load the hostile custom tool"
        assert markers["plugin"].exists(), "control did not load the hostile native plugin"
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


def catalogue(binary: pathlib.Path, catalogue_path: pathlib.Path, model_id: str, auth_tier: str) -> None:
    with tempfile.TemporaryDirectory(prefix="nessa-opencode-catalogue-") as temporary:
        root = pathlib.Path(temporary)
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
            text=True, start_new_session=True,
        )
        try:
            rpc(process, 1, "initialize", {
                "protocolVersion": 1,
                "clientInfo": {"name": "nessa-catalogue-harness", "version": "0"},
                "clientCapabilities": {"fs": {"readTextFile": False, "writeTextFile": False}, "terminal": False},
            })
            result = rpc(process, 2, "session/new", {"cwd": str(workspace), "mcpServers": []})
        finally:
            process.stdin.close()
            process.wait(timeout=10)
        model_option = next(option for option in result["configOptions"] if option["id"] == "model")
        offered = {option["value"] for option in model_option["options"]}
        catalogue_data = json.loads(catalogue_path.read_text())
        shipped = {model["modelId"] for model in catalogue_data["models"] if model["provider"] == "opencode"}
        assert model_id in shipped, f"selected model is not in Nessa's shipped catalogue: {model_id}"
        assert model_id in offered, (
            f"selected Opencode model absent from pinned ACP catalogue for {auth_tier}: "
            f"{model_id}; offered={sorted(offered)}"
        )


if __name__ == "__main__":
    command, executable, *rest = sys.argv[1:]
    binary = pathlib.Path(executable).resolve()
    version = subprocess.run([binary, "--version"], capture_output=True, text=True, timeout=10)
    assert version.returncode == 0 and version.stdout.strip() == "1.18.31"
    if command == "boundary":
        boundary(binary)
    elif command == "catalogue":
        catalogue(binary, pathlib.Path(rest[0]), rest[1], rest[2])
    else:
        raise AssertionError(f"unknown command: {command}")
