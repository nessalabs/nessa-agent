# Supervised canonical outbound delivery

Source `fc713591785970b8eca5ec20ad819024bf131cb0` preserves main
`37c7772244dde5ab7d9ba382196b3f34fd0192fc` (Channels and GitHub App), the
published command/grant slice `3a347ad38`, and its earlier scoped reviews.
This checkpoint does not assert merge readiness or final platform/browser/stress
acceptance.

The public owner WebSocket flow issues/registers a configured principal and
shares the selected conversation. The same native Auth/Cedar and canonical grant
store feed the principal SDK reader, durable queue and existing signed HTTPS
adapter. There is no paired-receiver impersonation, new protocol/SDK/policy
surface or live provider dependency. Configuration builds passive dependencies;
the host starts the retained sender only after fallible startup and enters its
existing serve/cleanup owner without an intervening await.

The delivery-module tests have distinct scopes: twelve mount the worker; one
checks only the public scheduling-candidate query. The preparing-source control
injects the canonical source's bounded-progress refusal. Other mounted controls
use actual canonical SDK storage and native Auth/grants. Actual recipient traffic
uses loopback TCP/TLS with an in-memory CA: only the socket/CA preparation seam
changes, while the real bounded POST, signature, headers and response parser run.
The normal public-address/TLS/deadline/byte-policy tests remain in the suite.

Recipient/lifecycle evidence covers one-chat delivery, HMAC and stable retry
ID/timestamp/signature/body, capability refusal with no cursor/delivery/POST,
unshare/revoke/expiry/old-epoch no-send, public revoke winning during preparation,
absolute expiry during actual durable Sending acknowledgement, cold recovery of
an actually received but unacknowledged response, cold Sending-to-Unknown without
reposting it, actual POST/revocation/process-cleanup ownership, and preserving a
received response before a later Clock callback panics. That panic remains failed
cleanup even when the actual response is successfully settled afterward.

Original failures remain preserved: known response publication loss originally
left live cleanup unresolved; source preparation originally failed the shared
supervisor; late expiry exposed stale due-batch work; an old queued row completing
after 1,024 newer terminals could not settle at capacity. The corrections reuse
native history/publication, fresh bounded due selection and the existing limit.
The retention fallback deletes only published alternatives and retains owed
facts. A counter probe first measured 1,105/4,177/8,273 VM steps with published-only
history; the corrected affected-registration query measures 80/80/80 at
128/512/1,024 rows. No assertion, flush, limit or timeout was relaxed.

All three original full-boundary retention cases and the new late-response
boundary passed in the full local suite. They were explicitly excluded from
some earlier focused runs, not from this full qualification. Prior original
Windows failures and their source-specific repair receipts remain historical;
Linux results do not replace fresh Windows/macOS qualification.

[Nine revert probes](controls.jsonl) each failed in runtime tests, rather than
compilation, and every source byte was restored. They cover retained-context
validation, fresh due work, Sending acknowledgement, post-ack expiry, observation
capture before callbacks, physical-job joining, original response time, bounded
source preparation and stale-snapshot refusal. Two possible simplifications were
also falsified, so their guards remain. The [reproduction script](controls.py)
contains only isolated test mutations and restores source in `finally`.

The final restored run passed **258 full webhook tests** with no retention
exclusions, **38 host/lifecycle tests**, required seven-package all-target Clippy
`-D warnings`, formatting, **108 architecture tests** and architecture check.
All thirteen delivery-module cases are included. Source/test bytes match the
frozen source above; controls changed no surviving byte. Independent whole-feature
review and source-matched CI, platform, browser and default/raised stress acceptance
remain separate outstanding gates.

[State-source reconciliation](source-pages.json) records the three affected page
checkpoints and changed linked inputs. Their existing inner diagrams are
byte-identical; no new render or browser result is claimed.
