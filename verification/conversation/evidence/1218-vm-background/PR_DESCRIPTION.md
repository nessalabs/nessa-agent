# Run cloud background commands in their thread's VM

**Before:** `background.start` in a cloud thread is refused because it has no local workspace.

**After:** the command runs under Shepherd in the thread's existing VM, keeps the VM in use, and settles the original task with its output and cleanup evidence.

Closes #1218.

## Background

The gateway already owns background-task admission, persistence, continuation and idle parking. The workspace runtime owns the hold that prevents a VM from resting. This change joins those existing owners with an independent SSH command session.

## The problem

Cloud leases report a VM working directory, while background admission accepted only a local directory. No task could therefore keep a cloud thread awake through the existing background owner.

## The solution

The source commit `9f10c41087f6602abae76168efd312e8574a4d6c` adds a typed execution target from the admitted lease, requires the VM's reported cwd, and keeps unsupported placements refused. The VM runner opens the existing workspace without provisioning, uses the verified installed build's `background-run` helper, and retains `InUse` through the SSH stream. Agent and background connections share the existing channel/allocation checks.

The helper runs the same bounded Shepherd command runner used locally. A cancellation byte or closed SSH stdin requests cleanup. A valid terminal record settles the task; losing the connection without one settles interrupted with cleanup unconfirmed. Restart uses the existing recovery path and never replays the command. Location is derived from the task's conversation and immutable placement.

## How we checked it

The repository file `verification/conversation/evidence/1218-vm-background/README.md` records commands, counts and revert probes.

- 36 background, 85 workspace, 128 SSH and 31 CLI tests passed; the opt-in real-SSH test was ignored.
- Both real helper CLI tests passed, covering open stdin, cleared synthetic secrets, and a large record on shared socket stdio.
- The fake-VM stress test passed at its default 3 repetitions and at 10 repetitions.
- Six deliberate reversions made their regression tests fail. Both the blocking-stdin hang and truncated socket-output record were reproduced before their fixes.
- Server and CI-package clippy with all targets and `-D warnings`, formatting, architecture checks and stress-plan tests passed.

## Tradeoffs and follow-ups

Unconfirmed cleanup retains the existing failed owner and can keep the VM from idle rest; automatic reconciliation is unchanged. The final record carries the bounded output tail; there is no interim output publication. Shells a provider starts outside `background.start` remain outside this task owner. Real Boat/SSH and browser VM tests were not run; no paid resources or real credentials were created. The lease protocol, worker-image pin, saved-login behavior and authorization policies are unchanged.

## Rollout

The gateway installs its verified helper build into an existing VM before using it, including an image whose lease protocol already matches. This description is prepared locally. No branch has been pushed and no PR has been published; publication awaits the user's approval.
