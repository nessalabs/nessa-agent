# Plan: ship the desktop app on Linux and Windows

Status: TODO. macOS is done. Linux and Windows are not started.

This plan is written in plain language so anyone on the team can follow it.
Diagrams use Mermaid and render on GitHub.

## 1. Where we are today

The desktop app opens on all three systems. But only macOS can run the
**gateway**, the background service that agents talk to. On Linux and Windows
the app compiles, then says "Bundled gateway services currently require macOS"
and stops there.

```mermaid
flowchart LR
    App[Desktop app window]
    Host{Which OS?}
    Mac[macOS adapter<br/>launchd, ~1,900 lines<br/>DONE]
    Lin[Linux adapter<br/>NOT STARTED]
    Win[Windows adapter<br/>NOT STARTED]
    GW[(Gateway service)]

    App --> Host
    Host -->|macOS| Mac --> GW
    Host -->|Linux| Lin -.->|"error: needs macOS"| App
    Host -->|Windows| Win -.->|"error: needs macOS"| App
```

The four things that are macOS-only right now:

| Piece | What it does | Where |
| --- | --- | --- |
| Gateway host adapter | Installs, starts, checks, and retires the background service | `src-tauri/src/gateway/infrastructure/macos/` |
| Runtime staging | Downloads Node, builds the CLI tools, packs them into the app | `scripts/desktop/prepare-macos.mjs` |
| Release plumbing | Builds, signs, and publishes the app and the update feed | `scripts/desktop/release-assets.mjs`, `.github/workflows/release.yml` |
| Bundle check | Proves the built app is signed and safe to ship | `scripts/desktop/verify-bundle.mjs` |

## 2. What the gateway lifecycle looks like

This is the contract every platform has to keep. The app asks the host adapter
to **register** a runtime. The adapter hands it to the OS service manager, waits
until the service answers on localhost, and reports back. On quit the app asks
the adapter to **stop agents**, and the service keeps running on its own.

```mermaid
sequenceDiagram
    participant App as Desktop app
    participant Host as GatewayHost adapter
    participant SM as OS service manager
    participant GW as Gateway process

    App->>Host: register(runtime, stage)
    Host->>Host: stage runtime files, write service definition
    Host->>SM: install and start service
    SM->>GW: spawn
    loop until ready or timeout
        Host->>GW: health check on loopback port
        GW-->>Host: ready
    end
    Host-->>App: ReconciledGateway (port, incarnation)
    App->>GW: chat, agents, MCP

    Note over App,GW: user quits the app
    App->>Host: stop_agents(gateway)
    Host->>GW: ask agents to stop
    Host-->>App: acknowledged
    Note over SM,GW: service outlives the app and restarts if it crashes
```

Only the two boxes on the right change per platform. The two on the left stay
the same.

## 3. Shared work (needed by both Linux and Windows)

Do these once. Linux uses them first, Windows reuses them.

1. **Per-platform runtime staging.** Split `prepare-macos.mjs` into a shared
   core plus a small OS-specific part. The core downloads that OS's Node,
   builds `nessa` and `nessa-mcp`, installs the claude-acp harness, fingerprints
   everything, and writes the manifest. Only the signing step differs by OS.
2. **Updater manifest for more targets.** `RELEASE_TARGETS` lists two Darwin
   triples today. Add `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`,
   and make `releaseTarget()` map them to `linux-x86_64` and `windows-x86_64`.
3. **Release workflow matrix.** Add `ubuntu-latest` and `windows-latest` rows
   to the build matrix. The `--bundles app,dmg` flag becomes per-row.
4. **Bundle verification per OS.** `verify-bundle.mjs` only knows Apple tools.
   Give each OS its own check, or make the Apple check macOS-only.
5. **Unix-only code in the shared adapter.** Move `OpenOptionsExt` mode bits
   and `/bin/launchctl` calls behind OS gates so Windows compiles.

```mermaid
flowchart TB
    subgraph Shared["Shared work (once)"]
        S1[Split runtime staging into core + OS part]
        S2[Add Linux and Windows updater targets]
        S3[Add runners to release matrix]
        S4[Per-OS bundle verification]
        S5[Gate Unix-only code]
    end
    subgraph Linux
        L1[systemd user unit adapter]
        L2[.deb and AppImage packaging]
        L3[Install / start / quit / reopen test]
    end
    subgraph Windows
        W1[Decide service model]
        W2[Windows adapter]
        W3[Authenticode signing]
        W4[NSIS installer]
    end
    Shared --> Linux --> Windows
```

## 4. Linux

Linux is the easy one. **systemd user units** work almost exactly like launchd,
so the macOS adapter design carries over.

What to build:

- A `Systemd` adapter that writes a unit file under
  `~/.config/systemd/user/`, runs `systemctl --user daemon-reload`, then
  `enable --now`. Health checks reuse the existing loopback logic.
- Run `loginctl enable-linger` once so the service keeps running after the user
  logs out. This matches the "service outlives the app" rule.
- Use XDG paths (`$XDG_DATA_HOME`, `$XDG_RUNTIME_DIR`) instead of macOS
  `~/Library` paths.
- Package as `.deb` and AppImage. Declare WebKitGTK as a runtime dependency.
- No code signing is required on Linux.

```mermaid
sequenceDiagram
    participant App as Desktop app
    participant Sys as Systemd adapter
    participant SD as systemd --user
    participant GW as Gateway

    App->>Sys: register(runtime, stage)
    Sys->>Sys: copy runtime to $XDG_DATA_HOME/nessa
    Sys->>Sys: write nessa-gateway.service
    Sys->>SD: systemctl --user daemon-reload
    Sys->>SD: systemctl --user enable --now nessa-gateway
    Sys->>SD: loginctl enable-linger (first time only)
    SD->>GW: start
    Sys->>GW: health check until ready
    Sys-->>App: ReconciledGateway
```

Done when: fresh Ubuntu machine, install the `.deb`, open the app, chat works,
quit, reopen, chat still works, log out and back in, gateway still running.

## 5. Windows

Windows is harder because it has **no per-user service manager** like launchd or
systemd. We have to pick one of three models before writing code.

| Option | Survives logout | Auto-restart on crash | Needs admin | Fits current design |
| --- | --- | --- | --- | --- |
| A. Task Scheduler logon task | No | Yes, with retry settings | No | Good |
| B. Real Windows Service | Yes | Yes | Yes, at install | Best, but admin prompt |
| C. App supervises child process | No | Only while app is open | No | Weak |

**Recommendation: Option A** (Task Scheduler). It needs no admin, restarts the
gateway on crash, and outlives the app window. Losing the service at logout is
acceptable because the app is per-user anyway. Option B is a fallback if users
ask for always-on.

```mermaid
flowchart TD
    Q1{Must the gateway run<br/>when no user is logged in?}
    Q1 -->|No| Q2{Can we ask for admin<br/>during install?}
    Q1 -->|Yes| B[Option B: Windows Service]
    Q2 -->|No| A[Option A: Task Scheduler logon task]
    Q2 -->|Yes| B
    A --> Pick[Recommended]
```

What to build:

- Decide the model (above) and write it down in this doc.
- A `TaskScheduler` adapter that uses `schtasks` or the Task Scheduler COM API
  to register a logon task, start it, and health-check on loopback.
- Windows paths: `%LOCALAPPDATA%\Nessa` for runtime and evidence.
- Replace file-mode bits and `launchctl` calls with Windows equivalents.
- **Authenticode signing.** Buy or provision a code-signing certificate, run
  `signtool` with a timestamp server, and sign the nested runtime binaries
  (Node, `nessa.exe`, `nessa-mcp.exe`) as well as the installer. Same shape of
  work as the Apple notarization already done.
- Package with the NSIS installer Tauri already supports.

```mermaid
sequenceDiagram
    participant App as Desktop app
    participant TS as TaskScheduler adapter
    participant Sched as Windows Task Scheduler
    participant GW as Gateway

    App->>TS: register(runtime, stage)
    TS->>TS: copy runtime to %LOCALAPPDATA%\Nessa
    TS->>Sched: create logon task "NessaGateway" (restart on failure)
    TS->>Sched: run task now
    Sched->>GW: start
    TS->>GW: health check until ready
    TS-->>App: ReconciledGateway
```

Done when: fresh Windows 11 machine, run the signed installer without
SmartScreen warnings, open the app, chat works, quit, reopen, chat still works,
kill the gateway process, it comes back on its own.

## 6. Order of work

```mermaid
gantt
    title Rollout order
    dateFormat  X
    axisFormat %s
    section Shared
    Staging split, updater targets, matrix   :s1, 0, 3
    Per-OS verification, gate Unix code      :s2, after s1, 2
    section Linux
    systemd adapter                          :l1, after s2, 3
    Packaging and end-to-end test            :l2, after l1, 2
    section Windows
    Service model decision                   :w1, after s2, 1
    Adapter and signing                      :w2, after l2, 4
    Installer and end-to-end test            :w3, after w2, 2
```

Linux first. It proves the shared staging and release pieces with the least new
design. Windows second, because it needs a lifecycle decision and a signing
setup before any adapter code is useful.

## 7. Already cross-platform (no work needed)

`nessa-auth`, `nessa-server`, `nessa-sdk`, and `nessa-local-storage` already
build and test on Windows and Ubuntu in the `local-auth` CI job, including a
Windows private-files smoke test. The server and auth layers are fine. Only the
desktop host is missing.

## 8. Checklist

- [ ] Shared: split runtime staging into core + OS parts
- [ ] Shared: add Linux and Windows updater targets and manifest keys
- [ ] Shared: add `ubuntu-latest` and `windows-latest` to the release matrix
- [ ] Shared: per-OS bundle verification
- [ ] Shared: gate Unix-only code in the gateway adapter
- [ ] Linux: `Systemd` adapter with linger and XDG paths
- [ ] Linux: `.deb` and AppImage packaging with WebKitGTK deps
- [ ] Linux: end-to-end install / start / quit / reopen / logout test
- [ ] Windows: record the service model decision
- [ ] Windows: `TaskScheduler` adapter
- [ ] Windows: Authenticode certificate and `signtool` in CI
- [ ] Windows: NSIS installer and end-to-end test
- [ ] Update `docs/guides/gateway-chat.md` with the new platform sections

Related: [Linux desktop packaging](linux-desktop-packaging.md).
