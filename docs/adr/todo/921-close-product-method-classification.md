# 921. Close product method classification at the gateway

## Purpose

Make a product-method extension incomplete until the gateway deliberately chooses
its handler, response lane, and paired-admission shape. This closes an extension
contract; it does not change any method's current behavior or give Rust a policy
decision that belongs to Cedar.

- **Date:** 2026-10-15
- **Status:** proposed

## Context

The manifest already owns product method names, parameter types, and the Cedar
grant asked before dispatch. Its generator publishes string constants,
`PRODUCT_READY_METHODS`, and `action_for_method`. The gateway then makes three
independent choices from the original string:

1. `product/socket.rs` routes exact names and name prefixes to handler modules;
2. `ResponseClass::for_method` selects Control, App, Record, or the Ordinary
   default before work takes a lane permit; and
3. `product/read_access.rs` selects Drive and Comment paired access from exact
   names, gives every other conversation write the Write default, and separately
   lists methods whose own read/list/follow admission is authoritative.

Handler modules make further exact-name choices. Watches and subscriptions have
connection-owned dispatch before the ordinary handler; the handshake and
`auth.session` have lifecycle-owned dispatch. Unknown strings are refused as
`unknown_method`, but they currently inherit the Ordinary lane on their way to
that refusal, subject to existing request validation, session checks and lane
capacity. An invalid or saturated request can therefore be refused earlier.

The test named `every_manifest_method_has_one_runtime_dispatch_path` proves only
that manifest names equal the generated advertised-name list and that most names
have a generated grant. A newly generated, granted name satisfies those checks
without selecting a handler, a non-default lane, or paired admission. No
advertised method is presently known to lack a handler, and this finding was
source inspection rather than a runtime reproduction.

The extension path therefore crosses these owners:

| Concern | Current owner | Extension step |
| --- | --- | --- |
| Wire name, params, grant | `protocol/product/manifest.json` and `v1.json` | Add the method/schema and regenerate Rust and TypeScript |
| Known-name identity | generated string constants/list | Generated indirectly from the manifest |
| Authentication lifecycle | product socket | Place handshake/session/watch methods in their lifecycle path |
| Handler route | product socket and handler module | Add a top-level route and the handler's exact method case |
| Response capacity | product socket | Add a lane case only when the Ordinary default is wrong |
| Paired admission shape | `product/read_access.rs` | Add to the own-check list or Drive/Comment match only when a default is wrong |
| Policy verdict | Cedar behind the access ports | Keep deciding whether the admitted principal may act |
| Client presentation | generated client plus the consuming feature | Add the typed call and its product behavior |

This is related to #797 only at a later boundary. A future private admitted
command may bind the parsed method, caller, and immutable initiation together.
That command must still check grant freshness at effect entry; issuance cannot
turn an authorization observation into a permanent capability. This decision
does not implement or enlarge that work.

Current source owners (reviewed at `9812c3c`):
[manifest](../../../protocol/product/manifest.json),
[generator](../../../scripts/generate-product-protocol.mjs),
[socket](../../../crates/nessa-server/src/product/socket.rs),
[paired access](../../../crates/nessa-server/src/product/read_access.rs),
[connection watches](../../../crates/nessa-server/src/product/change_watch/connection.rs),
[subscriptions](../../../crates/nessa-server/src/product/subscription/mod.rs), and
[writer tests](../../../crates/nessa-server/tests/product/socket/writer.rs).

## Decision

Generate a closed Rust `ProductMethod` enum from the existing manifest beside
the generated method constants. Its fallible parser accepts exactly the manifest
names; generation refuses two wire names that normalize to the same Rust variant.
Parse each request once at the socket boundary. Do not add routing, lane, or
paired access fields to the manifest: those are gateway behavior, not client
protocol.

Add one gateway-owned exhaustive classification from `ProductMethod` to a
`MethodPlan` containing three independent typed choices:

- `Handler`: the exact lifecycle or handler-module entry point;
- `ResponseLane`: Control, App, Record, or Ordinary; and
- `PairedAdmission`: manifest-grant admission, an existing method-owned
  read/list/follow check, Drive, Comment, catalogue Write, or lifecycle-owned
  admission for the handshake/session/watch paths.

The names may differ during implementation, but the three fields stay explicit
and have no `Default`. Each arm supplies the three fields directly: no
shared catch-all plan, wildcard, or prefix arm. `Handler` names an existing
lifecycle or handler-module entry point. It deliberately classifies ownership;
selecting a module alone does not prove the module implements that method.
Handler tests must exercise each classified method through its target and
distinguish implemented refusal (for example invalid params) from
`unknown_method`. Internal operation dispatch may retain exact matches, but it
consumes the parsed identity rather than parsing the wire string again.

An unknown string does not construct `ProductMethod` or enter the classifier.
Keep the current request/session checks and bounded Ordinary capacity path ahead
of its `unknown_method` response; parsing identity early must not reorder those
refusals. No extra queue or unbounded task is introduced. Parse the handshake as
well, while retaining its existing pre-authentication lifecycle and the
`already_authenticated` response when it appears on an authenticated connection.

The method plan describes which admission question to ask, not its answer.
Manifest grants remain generated from the manifest. Paired Drive, Comment, and
Write remain facts supplied to `PairedAccess`; Cedar remains the sole authority
that allows or denies them. The plan does not cache a verdict, role, grant, or
policy revision.

### Enforced omissions and proof

| Change or defect | Enforcement |
| --- | --- |
| Manifest method has no unique Rust identity | Generator collision/coverage test fails; generated-output drift check fails |
| Generated `ProductMethod` gains a variant with no method plan | Rust compilation fails on the non-exhaustive match |
| A method arm omits handler, lane, or paired shape | Required `MethodPlan` fields make that arm fail Rust compilation |
| A new method silently inherits a catch-all plan | The synthetic extension compile probe fails if the unedited classifier compiles |
| Existing method is assigned the wrong handler/lane/paired shape | Table-driven gateway tests compare the plan with reviewed expectations |
| A selected handler does not implement its assigned method | Per-method handler tests reject `unknown_method`; classification compilation alone cannot detect this |
| Generated names/actions disagree with the manifest | Generator tests and the renamed membership/action test fail |
| Unknown input becomes known or bypasses refusal | Socket test fails before handler effects and asserts `unknown_method` after successful existing admission prerequisites |
| Control work can be starved by ordinary saturation | Existing saturated-lane socket tests fail; the migration adds one typed-method counterpart |

The implementation proof adds a generator fixture with a synthetic granted
method, regenerates into a temporary tree, and compiles the real gateway
classifier module without editing it. The expected result is a non-exhaustive-match compiler
error. This proves omission detection for a new identity: it cannot inherit an
existing plan. Required struct fields separately check completeness of the
plan. It does not prove correct assignments or handler invocation. After a
deliberate plan is added, focused tests assert its exact handler, lane, and paired
shape. The
fixture must not modify checked-in generated files or the live manifest. Keep
the classifier dependency-light enough to compile that real module with
`rustc` in the temporary fixture; do not compile a retyped example classifier.
This probe belongs in the existing product-protocol check. No additional source
parser or AST-shape guard is needed to restate the compiler's check.

Rename `every_manifest_method_has_one_runtime_dispatch_path` to describe its
actual guarantee: manifest/generated method membership and generated action
membership agree. The exhaustive `ProductMethod -> MethodPlan` compilation
proof establishes classification completeness. Classification tests establish the selected plan;
per-method handler tests establish implementation, and socket tests establish
that admission and execution consume the selected plan.

### Bounded migration

| Slice | Change | Behavior proof |
| --- | --- | --- |
| Identity | Teach the generator to emit `ProductMethod`, `as_str`, and fallible parsing, preserving current wire strings. This inert publication replaces no running classifier | Generator round trips each manifest name and refuses unknown/case-changed names and variant collisions |
| Gateway cutover | Add the exhaustive `MethodPlan` and switch socket admission, queue classification, lifecycle routing and handler entry points to the parsed identity in the same PR. Delete `ResponseClass::for_method`, `paired_action`, the own-check string list and prefix routing as their consumers move; no merged intermediate state keeps two authorities | A reviewed per-method table checks handler/lane/paired shape; handler tests exercise each target; existing authorization, unknown-method, capacity and saturation suites pass |
| Extension proof (part of cutover) | Rename the overstated membership test and add the synthetic extension compile probe to the existing product-protocol check | A new manifest variant cannot compile against the unedited classifier; making a plan field optional/defaulted must be rejected in review and exercised by the classification tests |

The following ordering cases supply the cutover's socket regression matrix.
Each row needs a positive and refusal assertion where applicable; implementation
adds exact test names to the PR, rather than treating this design as executed proof.

| Input/order | Preserve at cutover | Evidence owner |
| --- | --- | --- |
| Authentication method before and after authentication | Existing challenge/authentication path; `already_authenticated` after authentication | Socket handshake tests |
| Valid known ordinary/app/record method | Its current handler, admission facts and lane; one parsed identity reaches execution | Per-method table, handler and socket tests |
| Unknown name, valid session and available Ordinary slot | `unknown_method`, no handler effects | Socket unknown-method test |
| Unknown name with invalid frame, expired session or saturated Ordinary lane | Existing validation/authentication/capacity refusal ordering; no early parser response that bypasses it | Socket validation, expiry and saturation tests |
| Ordinary or App work held while a control arrives | Control admission and bounded response delivery keep their independent capacity | Existing writer/control saturation tests plus typed-method counterpart |
| Paired Drive, Comment, catalogue Write and own-check method | Same target, initiator and Cedar question before effects; unchanged policy verdict authority | Paired-access and socket tests |
| Watch/subscription starts, refreshes and ends | Existing connection ownership, per-batch authority checks and teardown | Watch/subscription socket tests |

Each slice updates generated output and consumers together. There is one current
wire contract: no alias parser, legacy enum, dual dispatch, schema-version bump,
or compatibility path. The migration neither adds a command bus nor moves every
gateway concern into the protocol crate. The plan contains only identity-based
classification; current credential, grant and conversation facts remain with
their existing admission owners and are read at the same boundaries as today.

## Alternatives considered

**Put handler, lane, and paired access in the protocol manifest.** Rejected.
Clients need method identity, params, and the requested grant; gateway capacity
and routing are server implementation. Publishing them would invert ownership
and make the manifest a universal behavior registry.

**Keep strings and strengthen enumeration tests.** Rejected. Another allowlist
can drift and a fallback still makes omission look intentional. A generated
closed type plus an exhaustive construction lets the compiler identify the new
work at every gateway build.

**Generate the complete gateway plan.** Rejected. It would move server behavior
into the protocol generator and still require a second authority for actual
handler code. Only identity is generated; the gateway states its own choices.

**Introduce a universal command bus or bind authorization permanently into a
parsed command.** Rejected. This issue needs extension completeness, not another
dispatch architecture. Admission and freshness retain their existing owners.

## Consequences

Adding a method becomes intentionally noisier: regeneration creates a variant,
and the server does not compile until all three gateway choices are reviewed.
Changing a choice remains ordinary reviewed gateway code, with table tests for
semantic mistakes the type system cannot recognize.

The gateway gains a small catalogue type and carries a parsed enum with each
request. In exchange it deletes distributed string classification and misleading
defaults. Unknown-method refusal and the Ordinary capacity used to deliver it
remain unchanged; control, app, and record lane ownership and saturation behavior
remain in the gateway. Cedar keeps all allow/deny authority. Runtime behavior is
unchanged until a separately reviewed implementation issue performs the migration.
