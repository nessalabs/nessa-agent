# 1168: Boat VM lifetime defaults and channel overrides

Issue: [#1168](https://github.com/nessalabs/nessa-agent/issues/1168)

Status: implemented, independently reviewed and verified against real Boat VMs. CI and broad-suite disposition remain pending.

## User behavior

Nessa settings owns the default VM auto-stop choice. A channel's settings offers
Use default, a finite duration, or No automatic stop. A new channel uses the
default. The channel's choice applies to both main-agent and worker Boat VMs;
their independent Local/Boat placement choices stay intact. Local compute does
not receive a cloud lifetime.

No automatic stop is the initial default. A finite duration includes active
work, and changing a setting applies on create/resume, not to a running VM's
current deadline. No automatic stop means users stop VMs in Boat and ongoing
compute can remain billable. Provider/account restrictions still apply.

## Ownership

The conversation domain owns the shared finite-or-unlimited lifetime value. A finite
lifetime contains positive seconds representable by Boat's u32 API. Inheritance
is absence of a channel override, distinct from an explicit unlimited value.

Use the existing gateway `config.json` for ordinary lifetime preferences. Do not
use the credential vault or introduce a preference database. The existing
`ConfigJson` mechanism supplies bounded file reads, the shared
`config.json.lock`, whole-file validation and atomic publication. MCP server
settings and native-linking edits already use this mechanism; runtime edits must
preserve their sections under the same lock.

The Boat configuration section holds the default nullable seconds and a map of
channel overrides. A missing channel key inherits; an explicit null disables
automatic stop; a positive value requests that duration. The initial default is
unlimited. Read the entire default/override pair from one document, then let the
domain resolve it. Reset-to-default removes only the channel key. Reuse the
existing whole-config byte bound; refuse an oversized edit without replacing
other settings. Token replacement/disconnection does not touch this section.
Preference editing does not require unlocking the secret vault.

Follow the existing configuration-change audit pattern: record the scoped
before/desired choice, authenticated initiator, cause and operation correlation
before publication, then record the observed publication outcome, including
non-durable replacement or uncertain failure. Do not report success after an
audit/storage failure. The accepted operation owns its publication/outcome work
if its socket caller disappears. The existing config backup capture already
reads `config.json` but currently exports only relay settings. Extend that path
to export the ordinary Boat runtime section, and include runtime audit records
alongside environment configuration audits. Do not add a new backup store kind.

The conversation service authorizes channel settings through an explicit channel-settings action in its existing Cedar
policy port using the stored channel's owner/organization facts. Product socket
admission still requires an appropriate authenticated management grant. A caller
cannot write an arbitrary channel key or learn another owner's
channel by setting its preferences.

The existing channel store owns conversation-to-channel membership. The
conversation workspace adapter receives that store from composition and maps
the stable channel ID into the workspace open request. The workspace application
asks the runtime-preference port for one snapshot, then the domain resolves
channel override, default, and initial unlimited at create/resume.
It never looks up a global service or puts channel authorization in the provider.

The Boat adapter serializes an explicit nullable ttlSeconds on create/resume:
finite seconds or null for no automatic stop. Omission must not restore Boat's
own default. VM receipt identity, no-replacement recovery and SSH ownership do
not change. A live attached VM is not power-cycled or extended by a preference
edit.

## Ordering and refusal table

| Event | Decision/effect | Regression evidence |
| --- | --- | --- |
| No saved default or override | Unlimited on next allocation/resume | Default resolution |
| Save finite default | Validate, persist, acknowledge | Boundary and config round-trip |
| Save explicit unlimited | Persist a distinct unlimited value | Nullable provider payload |
| Channel inherits; default changes | Next create/resume sees current default | Restart and channel isolation |
| Channel overrides | Override wins for coordinator and workers | Real adapter membership mapping |
| Reset channel | Delete only its override | Other channel/default preserved |
| Reset global default | Remove its stored preference, preserving explicit channel overrides | Config presence and audit before/after test |
| Snapshot identifies a different scope | Refuse before allocation or a success projection | Custom-adapter scope substitution test |
| Caller supplies non-enumerable or accessor channel scope | Capture the own scope once and project that same value into the dispatched request and response binding; inherited scope still refuses before dispatch | Client read/write non-enumerable and changing-accessor regressions |
| Invalid seconds or contradictory representation | Refuse before storage/provider effects | Protocol/domain boundary |
| Credential vault locked | Preference editing remains available; provisioning still requires its token | Independent settings/credential tests |
| Config/audit write fails | No success acknowledgement; retain intent and observed publication outcome | Config adapter fault tests |
| Different channel owner | Cedar refusal; no preference read/write | Actual policy tests |
| Simultaneous runtime/MCP/native edits | Shared config lock preserves unrelated values | Gated concurrent publication tests |
| Caller loses save response | Read back saved preference; no automatic replay | Public client contract |
| Settings change during a running VM | Existing deadline unchanged | UI sentence and allocation assertions |
| Uploaded input is sent to a finite Boat thread, then that VM resumes unlimited | Deliver the stored bytes into the selected workspace without replacing the VM or changing its active deadline; workspace copy survives resume | Combined real-provider uploaded-text, lifetime and restart receipts |
| Default-channel membership (None) | Use global preference; synthetic default channel has no override | Default membership test |
| Close saved-channel settings | Return keyboard focus to the channel's settings control; preserve label and click space beside row status | Both-engine/layout browser geometry and focus check |
| Row has settings actions and decorative status | Keep action hit testing separate: settings receives its click, status/count passes the pointer to the row. Presence of actions does not replace the decoration's identity. | Shared-controls real-browser row hit tests and settings matrix |
| Membership read fails or names missing channel | Refuse opening rather than use global policy | Adapter failure test |
| Channel lookup returns another valid identity | Refuse before using its owner facts or provisioning the requested target | Substituted-channel authority and membership tests |
| Several owners have over 1000 combined overrides | Parse the gateway-wide map under the existing configuration byte bound, without applying one owner's channel quota | Global-map parser regression |

## Verification and scope

This follows PR #1166, whose Linux dependency fix is verified independently.
The source review closed clean after three bounded rounds at
`471fa51e1f92fad5e17ddd20bc5dee407de1cffb`. Configuration, audit, allocation,
membership, authority, client and rendered tests cover the table. Source revert
probes fail before the restored tests pass. Browser settings verification passed
both engines/layouts, and raised conversation stress passed both engines. The
first real Boat workflow confirmed finite creation, unlimited resume, unchanged
active deadlines and default inheritance, with all four VMs confirmed absent.
The corrected-source repeat also passed with all four VMs confirmed absent;
the final CI package selection passed 3907 server tests with 15 existing ignores.
CI remains pending. Exact checkpoints,
receipts, screenshots, review dispositions and exclusions live in
[verification evidence](../../../verification/desktop/evidence/boat-runtime-settings/README.md).
The channel-workspaces state page names its exact reviewed source. Lifetime is
gateway-side policy and did not itself change the worker lease protocol.

The subsequent mainline integration inherits worker protocol
`066217f7701b98b9` and its matching prebuilt image. A new real provider workflow
passed at backend `f7a2a1f73`, with all four owned VMs independently confirmed
absent. Fresh assembled-source review closed clean in round two at
`e213c1ccb370ced51a9f4ea794cd58441a0717c1`, after correcting request scope capture
for JavaScript accessors and non-enumerable properties. The settings matrix
passes both engines/layouts and raised channel stress passes 9/9 per engine.
The CI pane-root scrolling repair is tracked in #1184; WebKit's separate GitHub
keyboard failure is tracked in #1186 without a baseline-cause claim. Broad
verification and final-head CI remain pending. Exact new attribution and review
exclusions are in the [mainline evidence](../../../verification/desktop/evidence/boat-runtime-settings/mainline/README.md).

Cloud wake, automatic idle detection, renewing active finite lifetimes and
altering already running VM deadlines are separate capabilities.


## Design review dispositions

The bounded design review identified four issues, resolved in implementation:
ordinary preferences must remain separate from credentials;
audit records need caller and before/after meaning; inheritance reads one
snapshot; and default membership is distinguished from read failure or a
dangling stored channel identity. The user then explicitly directed reuse of
existing config/settings files. That takes precedence over the reviewer
recommendation for separate SQLite storage: reuse ConfigJson and its publication
audit pattern instead. Channel settings gets an explicit Cedar action. Extend existing
config capture to include these preferences; #1161 remains the separate channel
catalogue backup gap. The verification record distinguishes completed checks from
the remaining final repeat, broad-suite baselines and CI.
