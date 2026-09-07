# Typed dependency injection

Implemented foundation: explicit constructor/factory injection in both runtimes.
There is no DI framework, runtime token registry, or global service locator.
Composition constructs adapters once and consumers receive typed dependencies.

## TypeScript

`src/composition/dependencies.ts` creates one application's session handle,
connection function, and conversation effects. `src/main.tsx` constructs this
object and the store, then gives the same instance to the session lifecycle.
`makeStore` injects only conversation effects into Redux thunk extra arguments.
Reducers remain pure; they do not resolve services. Tests can create separate
stores with different adapters without changing global state.

The conversation application owns `ConversationEffects`, which returns product
DTOs, not a client SDK object. The session adapter owns the transport handle and
its teardown. Unmount closes its current client; late connection results are
closed by the lifecycle cancellation check. Dependencies must remain stable for
the lifetime of a mounted lifecycle. Creating a factory does not open a socket.

```ts
const dependencies = createDependencies({
  conversation: myConversationEffects,
})
const store = makeStore(dependencies)
```

The default connector uses the authenticated product session at `/session`.
Host composition injects the credential source; client metadata does not supply
authority. Alternate connectors implement the current typed ready/close contract,
and the SDK owns connection recovery for each client instance.

## Rust server

`app::ports::Clock` is an application-owned trait. `RuntimeDependencies` holds
an `clock: Arc<dyn Clock>` and supplies the default monotonic implementation.
`CompositionRoot` constructs the dependencies and passes them to
`AppState::with_dependencies`. State clones share the injected clock. Separate
states can have independent clocks, including deterministic test adapters.
`AppState::from_environment` remains a compatibility constructor for existing
callers. Auth, hello, and routing behavior are unchanged.

This server pattern complements Tauri's existing `platform::current()` host
injection; it does not add a second host abstraction. Dependencies are scoped to
one server/application, not a static process-wide registry. Rust ownership/Arc
controls adapter lifetime. Future background adapters must expose explicit
startup/shutdown owned by composition; dropping a pointer is not a substitute
for draining writes or stopping workers.

## Adding another backend

1. Define a narrow port in the consuming application's module, using Nessa-owned
   inputs/results and typed errors. Domain code does not import adapters.
2. Implement the port in that feature's adapter module. Translate third-party
   DTOs there; application use cases resolve identities and call domain behavior.
3. Add a typed field to the relevant dependency bundle and construct the adapter
   in composition. Inject only the fields the consumer uses, not the entire app.
4. When there are multiple real production choices, add an explicit config enum
   and an exhaustive factory selection. Unsupported/missing configuration fails
   startup; it does not silently select another backend.
5. Run shared behavior tests for each adapter, including failure and isolation.
   Adapter selection is a deployment/startup choice, never a request parameter.

For event streaming, define Nessa's required stream port when integrating the
external crate, then implement it with the real crate. Preserve durable cursor,
ordering, replay, deduplication, and shutdown semantics in contract tests. This
foundation does not implement an event store or claim all stream backends are
interchangeable. No speculative stream stub is registered today. Product authentication is now
composed separately through `ProductDependencies` and the local registry adapter.

For identity, local and future managed providers implement the appropriate
verification/administration ports. Product policy enforcement stays in Nessa.
See [identity and tenancy](auth/identity-tenancy-and-cloud.md) for migration boundaries.

## Checks for contributors and agents

- Concrete backend construction belongs at composition or in its adapter factory.
- Feature effects use application-owned ports, not global getters or string lookup.
- React, Redux, client SDKs, and infrastructure types stay out of domain rules.
- No `Any`/downcast container in Rust or untyped dependency bag in TypeScript.
- Test different injected implementations in isolated application instances.
- Add abstractions when there is an actual consumer; do not scaffold empty layers.

Validation includes a TypeScript test dispatching through two independent effect
adapters and a disconnected store, plus a Rust test proving shared clock state
within a cloned app and isolation between apps.

## Environment selection (implemented)

Rust extends the existing `Environment::load(EnvSource)` parser. TypeScript uses
`src/env/environment.ts` with a pure map-based source; `src/env/vite.ts` alone
reads frontend variables. Composition receives parsed configuration. Tests pass
maps/MockEnv or inject adapters directly, never mutate global environment.

Frontend UI scenarios (start Vite in development mode):

```sh
VITE_NESSA_STAGE=dev VITE_NESSA_CONVERSATION_BACKEND=scenario VITE_NESSA_CONVERSATION_SCENARIO=echo pnpm dev
VITE_NESSA_STAGE=ci VITE_NESSA_CONVERSATION_BACKEND=scenario VITE_NESSA_CONVERSATION_SCENARIO=offline pnpm dev
```

The default conversation backend is `local`, preserving the existing dev-session
connector. Scenario mode injects conversation effects and does not mount the
socket lifecycle. It is a UI test mode, not a simulated authenticated server:
no gateway-ready status or trusted identity is fabricated. Use real-server
integration tests for middleware, hello, and protocol behavior. `echo` returns
the submitted text; `offline` rejects. Alpha/prod stages and production frontend
builds reject scenario mode even if a dev stage is supplied. Vite values are public
build inputs, not server security controls or runtime deployment secrets.

Rust real-server fixture:

```sh
NESSA_STAGE=ci NESSA_UPTIME_BACKEND=fixed NESSA_UPTIME_FIXED_MS=123 cargo run -p nessa-server
```

The default is `NESSA_UPTIME_BACKEND=monotonic`. Fixed uptime is dev/CI-only,
requires an explicit unsigned millisecond value, and affects only reported
uptime—not authentication deadlines or expiry. Real middleware and handlers still
run. Unknown backends, missing scenario values, and stray values for an inactive
backend are rejected. The local registry must be initialized in every stage; a legacy token grants no gateway access.

Selectors are deliberately per dependency. Event-stream and identity selectors
will be introduced with their real ports/adapters; there is no fake-everything flag.

## Product authentication composition

The `/session` route receives `ProductDependencies`: credential verifier,
current access reader, absolute clock, embedded policy evaluator, and health clock.
A separate administration port handles lifecycle operations. The serving router
does not mount the legacy `AppState` shared-token handler. `composition/local_auth.rs` selects the real
local adapter and moves its durable writes off socket workers. Tests inject ports
without removing authentication or policy enforcement.

Each operation authorizes against an owned committed snapshot. Its successful
snapshot read orders admission relative to mutation publication; admitted work and
responses may finish after revocation. The store serializes mutations through
persistence/publication, while normal handlers and socket writes share no global
admission mutex. Remote adapters must preserve coherent current snapshot reads and
bounded provider work; a connection-wide cached permission is insufficient.
Idle invalidation currently polls at one second; the registry's notification port
is available for future integration. See [local setup](../adr/done/0010-local-authentication.md).
