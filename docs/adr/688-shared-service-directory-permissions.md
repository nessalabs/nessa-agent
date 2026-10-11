# 688 — Shared service directories

LaunchAgents and systemd user/wants directories can contain other applications'
services. Retained file authority accepts an existing current-user-owned leaf
with read/search bits for others and no group/other write bits. It creates missing
directories privately and leaves existing modes unchanged. Private files, runtime
and gateway data directories retain their existing private-mode requirements.

| Input / ordering | Outcome | Regression |
| --- | --- | --- |
| Existing owned regular 0755 service directory | Retain descriptor and publish a 0600 Nessa definition; preserve mode | shared service publication and inactive systemd registration |
| Missing service directory | Create 0700 with existing transaction cleanup | existing directory-transaction cases |
| Group/other writable, foreign owner, symlink | Refuse before publication | shared writable and symlink refusal, existing owner/type checks |
| Directory moved or permission changed after retention | Refuse binding acknowledgement; preserve foreign replacement | retained shared binding regression |
| Existing public/writable Nessa file | Refuse private-file admission | shared service private-file regression |

The Unix storage adapter owns descriptor-bound acquisition and file publication.
The Linux staging adapter owns systemd definition/wants transactions, preserving
its existing journal acknowledgement and exact replacement checks. Runtime/data
acquisition remains private. No shared directory is chmodded by registration.
