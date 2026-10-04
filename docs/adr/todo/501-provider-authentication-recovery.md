# 501 — Provider authentication recovery

Status: proposed; typed ACP recovery is implemented locally, Claude internal-error classification remains unavailable.

## Decision

The provider's numeric ACP authentication-required response (`-32000`) is the
only authentication fact used by this change. `AgentError::authentication_required`
owns the classification; the conversation projection publishes it from the
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
The native launch acknowledgement does not prove login completed. It does not
resend the failed message or remove the notice. Provider credentials remain
outside Nessa's launcher.

## States and order

| State | Event | Result | Evidence |
| --- | --- | --- | --- |
| Latest turn failed | ACP `-32000` in provider report | Recovery card shown | SDK code test, projection restoration test, gateway mapping test |
| Latest turn failed | ACP `-32603`, even auth diagnostic text | No recovery card | SDK code test, projection test, gateway mapping test |
| Card available | Click / keyboard activate | Native login opens; button disabled while launch pending | Browser provider-sign-in script |
| Launch pending | Additional activation | No second launch | Disabled button and component in-flight guard |
| Launch pending | Launch acknowledged | Button available; card remains | Browser script |
| Launch pending | Launch refused / unsupported | Button available; small launch failure shown | Browser script |
| Card shown | Later turn replaces latest auth refusal | Card removed | Gateway mapping test, browser script |
| Failed turn restored | Provider report retained | Same typed recovery fact | Projection restoration test |

## Remaining limitation

The diagnosed Claude OAuth refresh failure was delivered as generic ACP internal
error `-32603`. This implementation intentionally does not identify it as
expired authentication. A provider-published typed signal or dependable expiry
probe is needed to recover that particular failure automatically.
