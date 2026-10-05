# 501 — Provider authentication recovery

Status: proposed; typed ACP recovery is implemented locally, Claude internal-error classification remains unavailable.

## Decision

ACP protocol 1 reserves `-32000` for Authentication required. The ACP worker
maps that response to `AgentError::AuthenticationRequired`; generic
`AgentError::Provider` codes from other adapters or saved history keep their
original meaning, including `-32000`. `AgentError::authentication_required`
reads only the explicit variant; the conversation projection publishes it from the
retained provider report, and both desktop surfaces consume the latest turn's published
fact. Internal errors (`-32603`) and diagnostic text never select the card.

The pure card lives in `src/provider-authentication/ui/`, with an injected login
action. Desktop workspace supplies its Redux controller; floating panel supplies
the host action through its App composition. The panel replaces the matching
authentication diagnostic `TranscriptDivider` and retains unrelated notices.
A pure authentication refusal publishes no diagnostic error; if independent
required work also failed, its error stays published and the panel keeps that
status beside the card.

The compact transcript card uses the requested heading **Your login expired**
and one **Sign in to Claude** or **Sign in to Codex** button. ACP's fact means
that authentication is required; it does not distinguish an expired login from
an absent login. That wording is the requested product copy, not a stronger
technical assertion by the adapter.

On macOS the injected `ProviderLogin` opens Terminal with the closed provider
command `claude auth login` or `codex login`. This starts the provider-owned login
flow using the person's default CLI configuration. Custom runtime environments,
provider executables installed only in Nessa's managed runtime, browser previews,
and other desktop operating systems are not promised this launch capability.
The native injected launcher publishes availability before a card enables its
action. Unsupported or unreachable hosts retain a disabled sign-in button and
do not attempt a launch. Browser verification injects the same capability port.
The native launch acknowledgement does not prove login completed. It does not
resend the failed message or remove the notice. Provider credentials remain
outside Nessa's launcher.

## States and order

| State | Event | Result | Evidence |
| --- | --- | --- | --- |
| Latest turn failed | Adapter-owned authentication-required report | Recovery card shown | SDK code test, projection restoration test, gateway mapping test |
| Latest turn failed | Generic provider code, including `-32000` or auth diagnostic text | No recovery card | SDK code test, projection test, gateway mapping test |
| Card unsupported | Capability absent, false, or unanswerable | Disabled action; no launch | Browser provider-sign-in script |
| ACP prompt refused authentication | Execution reply owns the refusal; finalized secondary failures absent and cleanup succeeded | Reply carries refusal; observation stream ends without duplicating it as a fault | Real ACP/Agent/storage roundtrip |
| ACP prompt refused authentication | Independent audit, delivery, or cleanup failure retained | Observation fault and required-work notice remain | ACP rejecting-audit and projection regressions |
| ACP startup or steering refused | No execution reply owns the refusal | Error remains on startup/steering path | ACP startup contract |
| Card available | Click / keyboard activate | Native login opens; button disabled while launch pending | Browser provider-sign-in script |
| Launch pending | Additional activation | No second launch | Disabled button and component in-flight guard |
| Launch pending | Launch acknowledged | Button available; card remains | Browser script |
| Launch pending | Launch refused / unsupported | Button available; small launch failure shown | Browser script |
| Card shown | Optimistic newer user input sending or failed before acceptance | Old refusal hidden while outbox owns the newer input | Public send and browser regressions |
| Optimistic retry | Gateway accepts its ID into pending; outbox retires | Latest mapped input owns recovery; old refusal stays hidden | Mapping, store reconciliation and browser regressions |
| Accepted retry | Pending becomes running user-only, then emits output | Old recovery stays retired throughout | Mapping and browser regressions |
| Failed turn | Duplicate pending entry names the same dispatched execution | No new input supersedes the refusal; card remains | Mapping regression |
| ACP delete refused authentication | Full listing omits the named session | Deletion settles NotListed | Public deletion contract |
| ACP delete refused authentication | Listing names session or fails | Original typed authentication refusal retained | Public deletion contract |
| Failed turn restored | Provider report retained | Same typed recovery fact | Projection restoration test |

## Remaining limitation

The diagnosed Claude OAuth refresh failure was delivered as generic ACP internal
error `-32603`. This implementation intentionally does not identify it as
expired authentication. A provider-published typed signal or dependable expiry
probe is needed to recover that particular failure automatically.
