# Settings action ownership (#1087)

A native-hook save and an Allow permission request each retain a component-owned
request identity until their accepted work settles. React's next disabled render
is feedback to the person; the synchronous owner is the admission guard.
Source replacement or view cleanup retires that identity. This is frontend
coordination, not a gateway authorization rule or cancellation of physical work.

The follow-up starts from main
`0b8019cf45f99b285f10c41199875d474492c6d1`, independently of source PR #1090
and documentation PR #1093. Those published branches are unchanged by this work.

## Preserved behavior

Native-hook saves still show the proposed choice, disable both harness switches,
use a fresh request ID and reread actual configuration after the write answers.
A lost/refused write with a successful read shows the stored value with failure
copy. When rereading fails, only a matching successful acknowledgement supplies
a choice. Otherwise the rows become unread. Stored settings affect the next
Claude/Codex process start; this UI does not restart a harness.

Allow still asks only from the person's gesture. A current allowed answer
replays the latest waiting-review snapshot. Denied access points to the bound
source's instructions; rejection or not-asked allows retry. Unsupported platforms
hide the rows. Notification permission is not approval of an agent's tool call.

## Ownership contracts

| Ordering | Owner and invariant | Executable evidence |
| --- | --- | --- |
| Two native callbacks before disabling commits | Save identity is installed before dispatch; another write cannot start while it owns the slot. | Same-task component regression; browser measures the production switch. |
| Focus/visibility during write or reread | Both defer to the save's mandatory reread; an earlier read cannot publish over that owner. | Component and browser checks. |
| Source A retires before its save answer | Cleanup releases A's slot; B reads independently. A cannot start another read. | Controlled provider replacement and unmount regressions. |
| Source A retires during its reread while B saves | A's late reread cannot publish its choice or unlock B's action. | Controlled component regression. |
| Repeated Allow while unresolved | One ask identity admits one permission API call; the button remains disabled until settlement. | Actual browser-adapter component regression and browser fixture. |
| Allow rejects or remains not asked | Current owner releases; another gesture may retry. Other outcomes do not replay reviews. | Component outcome and retry cases. |
| Retired Allow settles after replacement/unmount | Both current request identity and currently bound source must match before state or waiting-review replay changes. | Component and browser replacement checks; unmount/remount regression. |

## Reachability and execution limits

The native duplicate-dispatch counterexample requires same-task callbacks before
React commits disabling; separate-event clicks already dispatched once in the
audit. Provider replacement is deliberately driven through the component seam;
ordinary production replacement remains unproven. The Allow counterexample
counts two calls through the actual browser permission adapter. It does not
establish two visible OS prompts. Retired replay is a callback ownership issue;
no notification authorization bypass is established.

No port here cancels admitted native saves or permission requests. Retiring the
component prevents follow-up reads/publication/replay; it does not undo stored
configuration or withdraw OS permission. The existing gateway adapter converts
native read/write failures into typed outcomes; these fixes retain that contract.

The browser fixture drives production rows with controlled save/read ports and
a controlled Notification API. Chromium alone does not close the two-engine
browser gate. WebKit, canonical scripted acceptance, full desktop run-all and
native/real-prompt acceptance remain separate required evidence. Do not retry
the recorded denied browser download or manual workflow dispatch.

See the [statecharts](../state/services/desktop/workspace/settings-action-owners.md)
and [verification evidence](../../verification/desktop/evidence/settings-action-owners/README.md).
