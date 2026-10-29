# Local vault UI

`application/ports.ts` is the secret-free lifecycle seam; `controller.ts` owns one
coalesced dialog request and fences asynchronous reads against a later submit or
mount. The controller retains no passphrase. `adapters/native.ts` uses the native
app's retained pipe connection; `adapters/gateway.ts` uses the browser's existing
authenticated SDK connection. Only readonly native status reads coalesce.

`ui/provider.tsx` composes the dialog for onboarding and desktop protected-use
requests. `ui/dialog.tsx` owns transient password inputs, confirmation and optional
help. A protected-use hook retains its requester identity and the UI kit's nearest
portal container, so a Settings prompt renders above Settings. Leaving that
requesting surface cancels an unsubmitted dialog. Policy and cryptographic admission remain in the gateway. Canceling the UI
does not unlock data. Setup strength consumes the published vault contract.

Unit tests cover stale results, overlap and redacted refusals. The real gateway,
native pipe and browser flow is `verification/desktop/scripts/vault.mjs`; GitHub
consumer coverage is `github-live-events.mjs` and the conversation stress GitHub
step. See the [lifecycle](../../docs/state/services/gateway/local-vault.md).
