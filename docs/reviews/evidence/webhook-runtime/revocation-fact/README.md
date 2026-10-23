# Held original revocation fact

Source checkpoint: d883b0ed6544c112f441ae1de73f443c7306280f. Base: published6a107073f9261dcfcaedb37b9c8c9fe773c1d470. Parent selected A: actual optional original revocation in the existing AccessSnapshot and held registry read. No API rename, new history, actor inference, Cedar change or production webhook composition.

The typed fact uses the domain transition validator and shared journal identity decoder. Current lifecycle and snapshot ordering are checked before a fact is consumed. Actual local reads fail closed when revoked metadata lacks valid retained evidence. Expiry alone has None. Explicit and both supersession forms preserve original actors, cause, request time, journal identity and nullable correlation. Ordinary Product secret/replay semantics are unchanged.

## Executed checks

Linux canonical Auth:214 unit+67 integration tests passed. All134 canonical server webhook tests passed. The final added cause-type assertion passed in the seven-test revocation integration target and private controls baseline/restoration. Seven-package locked all-target Clippy with-Dwarnings, formatting, architecture and diff checks passed. Clippy preserves the existing lease constant-evaluation warning. The first formatting/architecture invocation omitted RUSTUP_HOME and failed attempting a read-only default home; corrected documented environment passed without changing permissions.

Thirteen private one-rule controls all compiled and caused assertion failures, then byte-restored baseline passed: revoked cause; required fact; sequence/revision order; snapshot revision; credential identity; issuance/expiry/revocation times; actual registry reader; shared nonzero sequence/revision and correlation validation. Actual source copies compile against immutable third-party dependencies, without replacing canonical or reviewer artifacts. The original snapshot-revision mutation had an invalid Rust expression; its compile error is preserved and is not counted as a caught rule. Corrected controls use compiled mutations. Source hashes verify final copies match committed source.

Initial test compile errors and historical-assumption failures are preserved. Tests were corrected to the existing synchronous fixture pattern without enabling Tokio features. Actual provisioning uses its original principal; offline owner recovery uses LocalOperator. A later repeated revoke receipt cannot prove the original historical actor: valid accepted actors and absent historical correlation are preserved, while intrinsically invalid history is refused without byte replacement. No new historical authenticity claim was added.

## Documentation and limits

Nessa Docs validates135 pages with0 issues. All three credential diagrams rendered without page errors in system Chromium151; the lifecycle chart was visually inspected. This is documentation evidence, not canonical scripted Chromium/WebKit qualification. Platform CI, runtime coordinator, joint outbox expiry/revocation composition, real recipient capability/revoke E2E and stress/chaos remain unfinished. No live credential, registration, public listener or real recipient transmission occurred. No denied CI/report/issue action was retried. W24 remains separately unpublished with its recorded adjacent Major finding.

[Design and ordering table](../../../../design/webhook-delivery-implementation.md#parent-selected-held-revocation-fact) · [Source tests](../../../../../crates/nessa-auth/tests/revocation.rs) · [Credential map](../../../../state/services/auth/auth-credential.md)

Fresh independent review is pending for this candidate. No publication qualification is claimed before it completes.
