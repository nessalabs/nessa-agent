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

Recovery eligibility is owned once by
`src/provider-authentication/model/recovery.ts`. Each mapping retains the raw
execution identity of its latest published input, including nonduplicate pending
inputs. A local submission captures that identity when it starts (null when none
has been observed), and an explicit resend refreshes it. That boundary survives
replacement views while local intent is retained. Matching boundaries mean the
local send began after observing the published input; they do not compare remote
and local clocks or claim global chronology. Local turns are not persisted in
saved tab references, so restoring tabs reads published evidence afresh.

The pure card lives in `src/provider-authentication/ui/`, with an injected login
action. Desktop workspace supplies its Redux controller; floating panel supplies
the host action through its App composition. The panel replaces the matching
authentication diagnostic `TranscriptDivider` and retains unrelated notices.
A pure authentication refusal publishes no diagnostic error. If independent
required work also failed, the projection publishes `requiredWorkFailed` on
that failed turn and keeps the person-facing diagnostic in `error`. Surfaces
branch on the flag. The panel shows that diagnostic beside the card only when
`requiredWorkFailed` is set, and shows no status beside the card for a pure
refusal or for an authentication diagnostic folded into the turn's status.
The desktop transcript keeps every authentication terminal. The pane omits a
pure refusal's terminal only while the sign-in card is showing, and shows it
again when a local retry hides that card. It keeps the diagnostic beside the
card when the flag is set.

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
| Latest turn failed | Adapter-owned authentication-required report for Claude or Codex | Recovery card shown; the pane omits a pure refusal's terminal while the card is showing. The transcript keeps the terminal | SDK code test, projection restoration test, desktop transcript regression |
| Latest gateway refusal | Local retry observed that input and is still sending or failed in the outbox | Card hidden; the retained auth terminal is shown | Desktop transcript regression |
| Card showing | Authentication diagnostic folded into status, `requiredWorkFailed` absent | Card only; that diagnostic is not repeated beside it | Panel transcript regression |
| Card showing | `requiredWorkFailed` set | Card and the required-work diagnostic | Panel transcript regression |
| Latest turn failed | Same report for OpenCode, an unknown agent, or no agent | No recovery card; the failure stays on the terminal | Desktop gateway mapping test |
| Latest turn failed | Generic provider code, including `-32000` or auth diagnostic text | No recovery card | SDK code test, projection test, gateway mapping test |
| Card unsupported | Capability absent, false, or unanswerable | Disabled action; no launch | Browser provider-sign-in script |
| ACP prompt refused authentication | Execution reply owns the refusal; finalized secondary failures absent and cleanup succeeded | Reply carries refusal; observation stream ends without duplicating it as a fault | Real ACP/Agent/storage roundtrip |
| ACP prompt refused authentication | Independent audit, delivery, or cleanup failure retained | Observation fault remains; `requiredWorkFailed` and the required-work diagnostic are both published | ACP rejecting-audit and projection regressions |
| ACP startup or steering refused | No execution reply owns the refusal | Error remains on startup/steering path | ACP startup contract |
| Card available | Click / keyboard activate | Native login opens; button disabled while launch pending | Browser provider-sign-in script |
| Launch pending | Additional activation | No second launch | Disabled button and component in-flight guard |
| Launch pending | Launch acknowledged | Button available; card remains | Browser script |
| Launch pending | Launch refused / unsupported | Button available; small launch failure shown | Browser script |
| Startup/new/resume refuses before dispatch | Queue settles with typed primary authentication error; no provider report | Card eligible; pure refusal has no duplicate required-work notice | Real automatic attachment/retention and projection tests |
| Startup authentication cause | Queue audit, storage or cleanup also fails | Card eligible; `requiredWorkFailed` and the diagnostic are both retained | Primary-cause wrapper regressions |
| Provider report present | Later local failure contains authentication | Report's provider outcome alone owns recovery | Contradictory report/result regressions |
| Generic startup cause | Subsequent independent authentication error | No recovery authority | Primary-versus-independent cause regression |
| Local A retained before observing wire B | B publishes an authentication refusal | B card shown; A remains visible | Real panel projection/render and desktop send/store regressions |
| Wire B observed | Local A begins, then sends or fails | B recovery hidden; historical failure remains | Shared boundary owner and both real render paths |
| Wire B observed with retained A | Same B read again under another revision | Captured boundary unchanged; recovery does not flip | Replacement-view regressions |
| Multiple local submissions retained | Newly observed refused wire D | Older boundaries do not hide D; a local begun after D does | Both surface regressions |
| Local A retained | Explicit resend after B observed | Boundary refreshed to B; B recovery hidden | Panel replay and desktop resend regressions |
| Local A queued with captured B | A acknowledged pending, then omitted from incomplete view | B boundary retained until authoritative retirement | Projection regression |
| Optimistic retry | Gateway accepts its ID into pending; outbox retires | Latest mapped input owns recovery; old refusal stays hidden | Mapping, store reconciliation and browser regressions |
| Accepted retry | Pending becomes running user-only, then emits output | Old recovery stays retired throughout | Mapping and browser regressions |
| Failed turn | Duplicate pending entry names the same dispatched execution | No new input supersedes the refusal; card remains | Mapping regression |
| ACP delete refused authentication | Full listing omits the named session | Deletion settles NotListed | Public deletion contract |
| ACP delete refused authentication | Listing names session or fails | Original typed authentication refusal retained | Public deletion contract |
| Failed turn restored | Provider report retained | Same typed recovery fact | Projection restoration test |

## Claude in a Boat workspace (#1246)

A Boat VM is made from Nessa's image share code, and Boat gives a VM made
from a share code or a saved snapshot none of the owner's own logins
(`holdsCreatorLogins` is false and no token is in its environment, unlike a
plain sandbox in the same environment). Running `claude auth login` on the
Mac signs in only the Mac, so it cannot fix a refusal from a Boat VM.

When the refused turn's lease ran in a Boat VM, the card's action is
**Authenticate**. It does not open Terminal. The desktop host runs
`claude setup-token` in the background under a pseudo-terminal. The CLI opens the
default browser at Claude's sign-in, waits for the browser's local callback, and
prints a long-lived inference token. The host reads that token from the
CLI's output and hands it to the window, which saves it with
`boat.saveClaudeSignIn`. The gateway keeps it in the vault entry
`boat.claude-sign-in` beside the Boat connection. It is never written to
browser storage, a file, or a log.

Every serve of a Boat VM reads the saved token and sends it to
`nessa env serve` as the first line of the serve's standard input. An empty
line means that no token is saved. The VM's shell reads that line before
`env serve` starts, and exports `CLAUDE_CODE_OAUTH_TOKEN` only when the line
is non-empty. The token is never in a command line or on the VM's disk.
`env serve` already gives that variable to every Claude launch
(`composition::agent::launch_environment`). The gateway's own local Claude credential is still never
sent to another machine. Only the token minted for Boat goes to Boat.

The action does not resend the failed message. Once the token is saved, the
card says so, and the person's next message is served with it. A serve that
is already running keeps the environment it started with.

| State | Event | Result | Evidence |
| --- | --- | --- | --- |
| Latest turn refused, lease in a Boat VM, Claude | Card rendered | **Authenticate**, not Terminal sign-in | Desktop transcript regression, stress step |
| Latest turn refused, lease local, SSH or peer | Card rendered | Existing **Sign in to Claude** launch | Existing transcript regressions |
| Authenticate available | Click | Host mints in the background; button disabled while it waits | Card test, browser script |
| Minting | Second click | No second mint | Card in-flight guard |
| Minting | CLI prints a token and exits 0 | Window saves it; card shows the signed-in note | Host parser test, card test, stress step |
| Minting | CLI exits without a token, is missing, or 10 minutes pass | Child killed; "Could not sign in. Try again."; nothing saved | Host minting tests |
| Token minted | Save refused (vault locked, invalid token) | "Could not save the sign-in." and the button is available again; nothing kept | Card test, product dispatch test |
| Save arguments | Token outside the token alphabet, wrong prefix or too long | `invalid_request`; nothing stored | `ClaudeSignIn` tests |
| Token saved | Next serve of any Boat VM | First stdin line is the token; `env serve` runs with the variable | SSH serve test |
| No token saved | Serve | Empty first line; no variable | SSH serve test |
| Vault locked or entry unreadable | Serve | Served with an empty line; warning logged; the agent's refusal shows the card again | Runtime test |
| Token later revoked or expired | Next refusal | Card again; Authenticate replaces the entry | Save-overwrites test |
| `boat.disconnect` | — | Boat connection and the Claude token both cleared | Product dispatch test |

## Remaining limitation

The diagnosed Claude OAuth refresh failure was delivered as generic ACP internal
error `-32603`. This implementation intentionally does not identify it as
expired authentication. A provider-published typed signal or dependable expiry
probe is needed to recover that particular failure automatically.
