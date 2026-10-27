# Standalone backup verification (#270, partial)

`nessa-backup-verify --bundle ABSOLUTE_PATH --mac-key-file ABSOLUTE_PATH`
checks an existing export cut without starting the gateway. From a checkout, run
`cargo run -p nessa-server --bin nessa-backup-verify -- --bundle ABSOLUTE_PATH --mac-key-file ABSOLUTE_PATH`.
The command is a separate build target; this milestone does not change release packaging. Both paths are
explicit. The key file contains the existing 32 raw HMAC key bytes; it is read,
not created, recovered or copied into the bundle. The verifier has no profile,
identity, admission, publisher, network or execution dependency. Its output is
JSON. Exit 0 means the supplied bytes passed verification, 1 means verification
failed, and 2 means command usage was refused.

Compatibility means the current `nessa-export-cut/1` grammar and writer
inventory. Store payloads remain opaque bytes except for the deletion inventory,
whose canonical format and watermark are checked. SQLite schemas, provider
compatibility, recoverability and authority acceptance are not established.
The exported deletion inventory has no separate MAC line: its body fingerprint
is authenticated by the outer manifest HMAC. No new encryption/key format is
introduced.

The caller supplies the trusted key. This does not answer how it survives source
loss, where the latest inventory is retained, or whether this cut is current.
The report names redacted, skipped and deferred entries and says that restore
readiness and inventory freshness were not assessed. It does not fetch deferred
history, import stores, replace keys or activate authority. Quarantine,
acceptance, and deferred fetch are
[quarantined restore](quarantined-restore.md), not this verifier.

## Order and refusal boundaries

| Row | Event / input | Result and evidence |
| --- | --- | --- |
| V1 | Manifest stream exceeds the byte budget | Stop after budget + 1 bytes; no record stream is opened. `manifest_reads_are_bounded` |
| V2 | Unsupported version, malformed/truncated text, duplicate public body identities, missing/out-of-order store or noncanonical fields | Typed refusal before payload access. Format decoder tests and `bad_manifests_do_not_open_payloads` |
| V3 | Named-entry budget is exceeded | Refuse before adding the next entry. `manifest_entry_limit_is_checked` |
| V4 | Manifest parses but HMAC is wrong or key differs | Authentication refusal before enumerating or opening record bodies. `authentication_precedes_payload_reads` |
| V5 | Authenticated layout has an extra, missing or repeated body | Refuse; only public records authorize body paths. `layout_matches_only_public_records` |
| V6 | Bundle root (including trailing separator/dot spellings), manifest, records directory, store directory or body is a symlink, special file, hard-linked body or unsafe private storage | Refuse using the private-storage reader; create/repair nothing. Directory-adapter and retained-root tests |
| V7 | Public body is truncated or corrupted | Stream SHA-256 disagrees with its authenticated fingerprint; no success report. `corrupted_and_truncated_bodies_are_refused` |
| V8 | Aggregate body-byte budget is exhausted, including a growing stream | Stop after remaining budget + 1 bytes with a resource-limit result. `body_reads_share_one_budget` |
| V9 | Deletion inventory has an unsupported version, malformed/noncanonical entries, duplicate fence keys or mismatched watermark | Typed refusal, even if the outer manifest was deliberately re-signed. Inventory-verification tests |
| V10 | Inventory was skipped or absent in an otherwise authenticated cut | Report its absence and the named omissions; do not infer current revocations. `an_authenticated_partial_cut_is_reported_honestly` |
| V11 | All named public bytes and inventory format match | Report counts and authenticated omissions; readiness/freshness remain not assessed. `a_published_cut_verifies_without_profile_effects` |
| V12 | Key file is missing, oversized, truncated or unsafe | Refuse without generating or repairing a key. Standalone command tests |
| V13 | A public body or its store directory is replaced after its reader opens, even with identical bytes | Refuse at EOF when the retained name/binding differs. `a_body_replacement_after_open_is_refused` and `a_store_replacement_after_open_is_refused` |
| V14 | Writer omission rows repeat an identity or overlap a public record: redaction plus secret skip, repeated unnameable/JSONL skips, or a public row plus another file's skipped line | Accept the authenticated metadata and report each omission. Only public body paths require uniqueness; every row still consumes the entry budget. `writer_omission_rows_can_repeat_and_overlap_public_names` and `current_writer_omissions_verify_without_publishing_secret_bytes` |
| V15 | Bundle root is replaced or becomes unsafe after manifest authentication, including an empty cut without `records/` | Source acquisition retains the private root once. Manifest reads, root enumeration and child directory acquisition use that retained authority, and enumeration validates its binding even on the empty path. Root-replacement regression tests |

The default work budget is 4 MiB of manifest text, 16,384 named entries,
4 MiB of inventory text and 1 GiB of public bodies in total. Reads use an 8 KiB
buffer. Resource refusals are distinct from malformed-format and authentication
failures. The library accepts explicit budgets; the command uses these defaults.
Directory enumeration stops on the first unknown body and does not recurse into
unrecognized directories. Metadata collections are bounded by the entry budget;
the inventory entry count is checked before decoding. Bodies are hashed without
opening a database or retaining their full contents, except for the separately
bounded inventory text.

Omission identities are the current writer's labels, not unique restore keys.
The writer can emit a redaction and secret skip together, repeat generic skip
labels, or reuse a JSONL line label across files. The verifier retains these
authenticated rows without inferring an origin or resolving their ambiguity.
An omission row does not authorize a body; another public row with the same
label may authorize that path. Recoverable identities and payload schemas
remain outside this format/integrity milestone.

The directory source owns a retained private root from acquisition until it is
dropped. The private-storage owner exposes read-only root and child-directory
acquisition using its existing identity, ownership and link checks. Enumeration
does not reopen the root by path, and opening a child carries the same retained
root chain forward. This closes the previous ownership gap between reading an
authenticated manifest and enumerating an empty cut; a later path lookup is not
allowed to substitute a new root for the acquired one.

Verification observes the bytes read; it does not reserve a bundle for a later
restore or establish off-device freshness. A later consumer must verify its own
inputs. Private-storage checks preserve existing ownership/link boundaries;
the verifier does not chmod, copy, create directories or write receipts.
