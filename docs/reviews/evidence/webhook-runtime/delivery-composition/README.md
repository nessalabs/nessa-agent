# Supervised canonical outbound delivery

Initial mounted source `fc713591785970b8eca5ec20ad819024bf131cb0` preserves main
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


## Corrected dispatch and credential lifecycle

Current source is `365f769078a3d2298340952ba519edc515468332`, preserving actual
main `5bb482a6b7c11dea2ddb64a2898edb6c623b0a19`. The source correction is
`17eed9f1a18e861e13a0f1f989fdac6fec5924e0`; merge `39ae5d96` adds only
upstream UI/stress changes; the final source commit imports types at module
boundaries without changing the decisions. [Whole-feature round one](review-1.md)
reported three P1 and two P2 findings and remains changes-required at its
original snapshot. It does not approve this correction.

Dispatch admission begins inside the existing 15-second deadline. Partial Auth
and grant fences have an independent owner so deadline expiry can release them
while the same pinned future joins actual metadata work. Gate-only acquisition
supplies no authority; current grant, epoch and Cedar validation stay in one
application owner. Canonical capture retains its separate SDK/cursor ownership.
Sending SQL is likewise retained/joined after deadline; an in-progress fsync can
still produce a late conditional commit under W-audit-race, never permission to
POST. Actual responses are captured before callbacks and settled after authority
release. Live recovery distinguishes guarded original Queued/Retry from actual
Sending, preserves known outcomes, and never repeats an uncertain Sending id.

Installed stopped history no longer blocks a new registration after canonical
stop acknowledgement. Idle reads retry stop without a grant or delivery and keep
the original initiator. Seed removal cannot route retained webhook history into
ordinary issuance: matching replay uses its original configuration/purpose;
genuine unregistered principals keep native ordinary issuance.

Final exact-source checks: **265 full webhook tests**, including all three
original retention boundaries, late response/full-boundary and actual prehistory
equivalence; **38 host tests** and the native principal-grant/epoch regression;
required **seven-package all-target Clippy -D warnings**, formatting and
**108 architecture tests/checker** passed. The delivery module has eighteen cases:
sixteen mount the retained worker, one is scheduling-query coverage and one
exercises the real dispatch-admission method with blocked native metadata. The
preparing-source case injects SourcePreparing; it is not a physical SDK read.
Other mounted fixture boundaries remain as described above.

[Six refreshed fault controls](review-1-controls.jsonl) and
[two additional controls](review-1-additional-controls.jsonl) each produced a
named runtime failure, not a compiler-only result. [Selected actual result lines](review-1-results.txt)
include the failed PUBLIC revoke-before-metadata-release assertion at10.04s.
The source was restored byte-identically. The six controls are pinned to39ae5d96;
the two additional application/command source files are unchanged at365f7690.
The reproducible scripts reject a mismatching source before mutating it.
Earlier local controls and failed attempts remain historical, not extra passes.

The first partial-admission fixture driver deadlocked by stopping polls of the
future that generated its awaited notice. That suite completed31of32 cases and
was interrupted with scoped SIGINT: it is **not a full pass**. The corrected
fixture drives that SAME future concurrently with its notice, resumes real time
before OS progress, releases/joins the real blocker on failure, and catches a
retained-fence defect as an assertion failure without hanging. The new cold
runtime test proves retained-purpose replay, active fresh-request refusal and
normal ordinary issuance after seeds are removed. No production assertion,
retry/retention bound, TLS policy or deadline budget was relaxed.

The [three page source checkpoint](source-pages.json) covers the current owners
and their linked changed inputs. Inner diagrams remain byte-identical; source and
link checks do not claim a new Nessa Docs renderer/browser run. Review round two,
exact-head CI/platform, canonical Chromium/WebKit and default/raised feature
stress remain pending. Native outbox reset tests do not establish a new public
owner-reset surface; this review must assess required reachability rather than
infer completion. No issue-close or merge-ready claim is made.
