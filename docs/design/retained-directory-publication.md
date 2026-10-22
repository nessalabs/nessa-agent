# Retained-directory publication (#324)

Issue: [324](https://github.com/nessalabs/nessa-agent/issues/324).
Status: accepted design; implementation and platform evidence in progress.

## Owners

`nessa-local-storage::PrivateDirectoryTempFile` owns reservation identity,
pre-rename checks, native publication, immediate cleanup disarm and typed
acknowledgement. `PublishedPrivateFile` retains the exact destination, native
identity and open payload handle after rename. Windows retains its private
non-delete-sharing directory chain and renames through the original DELETE
reservation handle using one `FILE_RENAME_INFO` encoder. `publish_new` selects
exclusive publication; `replace` selects `FileRenameInfoEx` with POSIX semantics
through the same encoder as path-based replacement (#874). Existing delete-sharing
readers keep the old file while new opens observe the replacement. Mutation-capable
handles that omit delete sharing still pin their names; no ignore-read-only flag
or path fallback is used. Tests are in the storage owner and retained-directory
integration suite, indexed by the crate README.

The reservation carries two handles: original rename/cleanup authority and writable
payload. Public mutable file access can replace the payload slot. The shared
publication owner validates their native identities before flushing that payload
or renaming, using the existing platform identity owner. Original name validation
precedes this comparison; the captured original identity is reused afterward.
A same-object writable clone is valid. No content-immutability or application CAS
guarantee is implied by matching file identity.

The consumer owns application authorization and expected-old CAS under its
stable lock. Storage replacement can also publish to an absent name; it does not
establish that an application expected an existing record. Pairing consumption,
writable exact retry/reconciliation and its original pending operation remain
owned by #264, outside this shared-storage slice.

## Ordering table (written before source changes)

| Row | Event/order | Result and owner | Evidence |
| --- | --- | --- | --- |
| W01 | Current private reservation and closed existing destination | Replace with reservation identity; acknowledge flush and bindings; remove reservation name | `retained_replacement_publishes_the_reserved_identity_atomically` |
| W02 | Destination absent | Replacement publishes name; exclusive publication still refuses occupied name | `retained_replacement_accepts_an_absent_destination`; existing exclusive-publication test |
| W03 | Invalid destination, changed origin or reservation before rename | Existing typed checkpoint, no published fact; clean only matching origin reservation, report independent cleanup error | `replacement_invalid_name_keeps_original_destination`; existing origin/reservation tests; `replacement_changed_reservation_retains_foreign_name` |
| W04 | Application destination changed before expected-old read/CAS | Consumer refuses before reserve/rename; storage supplies no second CAS | #264 pending-store tests; excluded from this storage slice |
| W05 | Windows destination has mutation-capable open handle, or target is directory | Native Rename refusal, no published fact; origin reservation cleaned; valid replacement after obstruction released | `windows_replacement_refuses_a_pinned_destination_then_accepts_after_release`; `replacement_refuses_a_directory_destination` |
| W06 | Rename succeeds, caller loses answer or acknowledgement fails | Disarm reservation cleanup before any later fallible call; keep exact published file; Drop cannot remove destination | `native_replacement_failed_acknowledgement_retains_published_identity`; `native_publication_panic_after_rename_keeps_destination` |
| W07 | Each post-rename acknowledgement checkpoint fails | Preserve stage, source, published identity/name/handle; no cleanup error | `every_post_rename_failure_retains_name_identity_and_open_handle_without_cleanup`; actual native replacement test |
| W08 | Live consumer reconciliation | Match original published identity and exact bytes under same lock before acknowledgement | #264 consumption; excluded from this slice |
| W09 | Caller lost during asynchronous save | Consumer physical closure keeps storage/key/permit until drain; storage itself is synchronous | #264 owned worker tests; excluded from this slice |
| W10 | Reopen after physical process drain | Consumer reacquires private authority and reconciles exact durable operation; no invented old native identity | #264 restart tests; storage `retained_replacement_publishes_the_reserved_identity_atomically` reopens and reads |
| W11 | Unpublished temporary is dropped or explicit failure occurs | Origin-only best effort Drop; explicit failure reports cleanup failure independently; foreign same-name file preserved | existing reservation cleanup tests; `replacement_changed_reservation_retains_foreign_name` |
| W12 | Same-user leaf mutation inside final check/effect interval | Cooperating callers excluded by consumer lock; no native compare-and-swap claim | documented exclusion; Windows directory/lock lifetime tests |
| W14 | Valid Unicode extended destination path beyond MAX_PATH | Same validated encoder accepts exclusive publication followed by replacement; original private checks apply | `windows_publication_accepts_a_unicode_destination_beyond_max_path` |
| W13 | Process crash or power loss | Reopen may observe old/new/missing/corrupt; no automatic repair or fabricated result | normal reopen tests; power-loss experiment excluded |
| W15 | publish_new receives a valid private foreign payload through public as_file_mut replacement | ValidateReservation before payload flush/rename; published absent. Destination stays absent, foreign file stays intact; clean only original reservation | `publish_new_refuses_foreign_writable_payload_before_effect` |
| W16 | replace receives that foreign payload while original destination exists | Same pre-effect refusal; original destination identity/bytes and foreign file remain unchanged; original reservation cleanup only | `replacement_refuses_foreign_writable_payload_before_effect` |
| W17 | Ordinary writes or public slot replacement with try_clone of SAME original payload | Both operations remain accepted; payload/native authority/published destination all identify the same object | `same_original_writable_clone_remains_accepted_for_both_operations`; ordinary replacement/publication tests |
| W18 | Original reservation name changed before publication, with foreign payload also substituted | Original name check keeps precedence: ValidateReservation; independently retained cleanup failure; foreign same-name witness/destination remain untouched | `changed_origin_keeps_precedence_with_substituted_payload` (Unix namespace mutation); Windows retains original name-pinning tests |
| W19 | Destination has an ordinary delete-sharing reader during replacement (#874) | Publish reserved identity while that reader retains old bytes; new opens see complete replacement | `retained_replacement_keeps_a_reader_on_the_previous_record`; path-based `replacement_keeps_a_delete_sharing_reader_on_the_previous_record`; endpoint `publication_replaces_an_endpoint_while_a_discovery_reader_retains_the_old_record` |


Windows directory and mutation-capable handles pin names while held. A locked
replacement target is refused; releasing that obstruction admits ordinary
replacement. Existing verified read-only probes share deletion. Creation ACLs
remain protected current-user plus LocalSystem before bytes, with owner, regular
single-link and no-reparse checks on the same handles used for I/O.

## Windows protected restore directory candidate (#270)

The Windows-only native directory module in `nessa-local-storage` owns the
candidate mechanics; backup owns whether they are qualified for activation.
The existing backup fail-closed path stays in place until native tests prove
this candidate. Native CI [39701307415](https://github.com/nessalabs/nessa-agent/actions/runs/39701307415)
at `5f8d` disproved the original N03 assumption: the initial handle could not
create a child under a read/search-only directory DACL (access denied, OS 5).
That failed receipt remains failed. The revised candidate permits additions,
but never DELETE_CHILD, during copying. It seals all directories before final
whole-tree content and identity validation. The regression requires the original
N03 operation to remain denied after sealing; mutation through a pre-acquired
ADD handle must also fail. A granted handle alone is not proof of mutation rights.

Native `c76d` [39703824781](https://github.com/nessalabs/nessa-agent/actions/runs/39703824781)
also failed qualification: actual leaf deletion and post-seal subdirectory
creation succeeded, and the Win32 relative rename returned OS 87. These failed
receipts remain failed. The relative rename candidate now uses
`NtSetInformationFile(FileRenameInformation)` with a retained target handle and
a buffer at least the native structure size. Bounded read-only ACL/token
diagnostics accompany the unchanged mutation assertions; no SYSTEM grant or
test permission is changed on a guess. Backup activation stays unavailable.

| Order | Operation | Required result / failure |
| --- | --- | --- |
| N01 | Create one absent child with `NtCreateFile(FILE_CREATE)` and protected read/search plus ADD_FILE/ADD_SUBDIRECTORY user DACL, without DELETE_CHILD | Return initial ADD/DELETE/WRITE_DAC authority; collision preserves the existing object |
| N02 | Another process attempts actual root rename/removal, nested-directory removal or leaf mutation | Root shares READ and WRITE but not DELETE, permitting the native rename destination-parent internal write open; nested directories share DELETE but neither their own ACL nor their parent grants DELETE. Leaves have read-only user ACLs from creation; no DELETE_CHILD exists in the copied tree |
| N03 | Create children relative to retained append-only directory; seal every directory read/search-only before final enumeration and content/identity verification | Child creation works before seal; the original N03 create is denied after seal, including through pre-acquired ADD_FILE/ADD_SUBDIRECTORY handles. Pre-acquired ADD authority cannot rename an external source into the sealed tree. Any unexpected entry fails final validation |
| N04 | Rename retained child handles before sealing, without replacement; after seal and final validation, close descendant handles and rename root through its initial DELETE handle | Preserve occupied destinations. When native rename returns a sharing error, classify AlreadyExists only if a no-follow retained-relative probe confirms an occupied target; absent or unreadable targets preserve the sharing error. Never reopen source for DELETE. An attempted directory rename with open descendants must return AccessDenied, per FILE_RENAME_INFORMATION. Post-close mutation refusals must hold before publication; no root ACL DELETE grant is added speculatively |
| N05 | Delete through the exact retained object handle | Remove that object only; a replacement at its old name is not cleanup authority |
| N06 | Successful root promotion, then reopen descendants read/control-only relative to the retained root and release temporary restrictions; retain root WRITE_DAC authority | Restore ordinary private owner ACLs only after live rename; failure remains an explicit error |
| N07 | Any create/rename/delete fails | Return the OS error; no path fallback, automatic ACL relaxation, or success claim |
| N08 | Fetch acquires an existing private sessions directory before creating any file | Retain a no-DELETE-sharing handle and check current build rights before invoking the source. Perform no parent ACL mutation, snapshot restoration or sealing; foreign entries retain their own permissions |
| N09 | Existing directory has a pre-acquired DELETE_CHILD handle; fetch creates fresh source/output/sidecar leaves | Each owned leaf has protected FR ACL and original RW/DELETE/WRITE_DAC authority with READ-only sharing from birth. Actual deletion, relocation and writes through pre-acquired parent authority must fail while pins live |
| N10 | Fetch validates output/sidecar identities and content, releases owned ACLs, then completes availability | Release only owned output/sidecar ACLs while original write/delete sharing pins remain alive through final available audit. Existing or newly created sessions parent ACL never changes. Failure retires exact held leaves; ordinary consumer access follows attempt Drop |

| T01 | Capture the current thread token, or its absence, before impersonation | Preserve the exact previous TokenId and restore it; an absent token is restored to absence |
| T02 | Derive DISABLE_MAX_PRIVILEGE token, without disabling SIDs or adding restricted SIDs, and install on this thread only | Same user and group SIDs/attributes; effective backup/restore disabled; no process token mutation |
| T03 | Full-ACL ordinary writer attempts create/write/rename/delete before protected assertions | All actual operations succeed; protected negative assertions run unchanged |
| T04 | Spawn a competing writer | Install its own scoped ordinary token; impersonation is not inherited |
| T05 | Test exits or unwinds | RAII restores exact prior thread token; restoration errors fail visibly |
| P01 | Repeat post-seal subdirectory creation under original effective token, then ordinary token | Enabled backup/restore profile reproduces the privileged bypass; otherwise original refusal is asserted. Restricted refusal is asserted under both profiles; no skipped branch |

Native ce5d CI 39706336820 confirmed current user is not SYSTEM, the SYSTEM ACE
is nonmatching, and backup/restore privileges are enabled on the elevated runner.
Its correct DACLs therefore do not establish the ordinary-writer boundary.
The scoped tests preserve this privileged counterexample explicitly. They do
not claim protection from an administrator using backup/restore privileges.
Directory rename with open descendants is independently unsupported by the
[kernel rename contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information).
[CreateRestrictedToken](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken)
provides the privilege-only test boundary; source user and groups remain checked.

| C01 | Capture opaque NativeObjectId through each initially owned descriptor | Common lifecycle records relative component chains and IDs; names do not confer cleanup authority |
| C02 | Reopen sealed directories and leaves retained-relative, requesting read and WRITE_DAC | Verify private no-follow object type and saved ID before returning ACL authority; mismatches leave unrelated bytes and ACL unchanged |
| C03 | Stage source handle was closed, or cleanup retains only a READ marker whose name changed | ReOpenFile upgrades metadata on the SAME object, checks saved ID before any ACL change; no old-path fallback |
| C04 | Prepare restrictive FR and temporary FRFW descriptors before any permission change; verified leaf then grants user FRFW and same-object ReOpenFile requests RW + WRITE_DAC with READ/DELETE sharing and no WRITE sharing | Existing ordinary writable descriptor forces failure. Reapply the already-owned FR descriptor on success and failure before propagating the result; no later token lookup/allocation may bypass resealing. Never grant DELETE in this window |
| C04b | Ordinary writer writes and closes entirely inside the temporary FRFW window, before trusted exclusive reacquisition | Exclusive reopen may succeed with the same inode ID and changed bytes. Exclusivity is not content evidence: common lifecycle rechecks exact staged tree contents after reacquisition and before ACL release |
| C05 | Exclusive reacquisition and FR restoration succeeded | Recheck same ID, return retained writable authority; exact truncate/sync targets recorded inode while future ordinary writes remain refused |
| C06 | Live restricted tree reopens saved components and restores permissions | Verified leaf ACLs, then child-directory ACLs, then root ACL; failure preserves live effect and restriction evidence |

The native closed-file tests require actual ReOpenFile interoperability with
NtCreateFile-created objects, retained READ metadata upgrades, rejection of a
writer occupying the temporary permission window, and unchanged unrelated
replacement bytes/ACLs. The [ReOpenFile contract](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-reopenfile)
requires compatible sharing; compilation alone does not qualify those mechanics.

| A01 | Fetch with genuine inherited foreign files/directories in an existing or newly created sessions directory | Retain parent identity without DELETE sharing and pin owned leaves; never mutate the sessions parent ACL. Compare exact parent and foreign DACL/control immediately after begin, note, release, setup failure, retirement and Drop; other-chat writes remain usable |
| A02 | Root destination-parent rename requires internal FILE_WRITE_DATA/SYNCHRONIZE open | Root shares READ/WRITE, never DELETE. Actual pre-acquired root ADD creates/rename succeed before seal and fail after seal; fresh root DELETE and path moves stay refused. Existing exclusive occupied-target mapping remains conditional on observed occupancy |
| A03 | An archive directory needs independent enumeration cursors after its ancestor spelling changes | Open an empty counted native name beneath the retained handle (the related directory itself), require synchronous directory-query rights, and compare object identity. Interleaving two cursors must preserve each cursor's continuation; no pathname or shared-cursor fallback |
| A04 | Unrelated foreign session entries appear during an admitted fetch after root verification | The final-audit regression starts with an admitted empty private sessions directory, then an injected source creates ordinary unrelated file/directory/grandchild entries only after SessionAttempt begins. Capture exact descriptors at that point and require them unchanged through publication, availability callback, audit failure retirement and return. Keep the normal early refusal for unauthenticated extras present before root verification; the fixture must reach the actual final callback and retain all writer/delete/rename assertions |

| M06 | Genuine unprotected inherited foreign children are present under a newly created stage/copy root before seal | Create protected read/search-only FRFX with the original trusted BUILD/DELETE/WRITE_DAC handle, retaining the existing sharing policy. While actual additions remain denied even through preacquired ADD handles, apply the SAME readonly ACL through SetSecurityInfo on this empty owned directory, then require private P/AI readback. Only afterward grant append through SetSecurityInfo on this already-modern original MAX handle; use that same original-constructor path for seal, with mandatory P/AI readback. Release is the separate terminal raw transition in M10, including verified reopens. The documented MAX-handle exception is the candidate parent-only mechanism; the exact production masks require native genuine-inherited-child proof. Do not substitute an AR request or permit model loss. On any initialization/readback/grant failure return Retained without readiness, a repair traversal or returning authority. Preserve that model through append and seal; terminal release has its own exact postcondition. Use the admitted production root; disposable child setup adds explicit FA while preserving and verifying real inherited append ACEs, unprotected control and actual full writes. Legacy mutation controls must use genuinely legacy children; modern-child raw assignment has a separate unchanged-ACL positive control. Production seal and terminal release must preserve exact bytes/control/writes |

Native `74650438` [39728283448](https://github.com/nessalabs/nessa-agent/actions/runs/39728283448) disproved the birth-descriptor assumption: protected-directory integration failed 1 passed / 10 failed because NtCreateFile discarded the requested AI bit. Those failures occurred before the pinning and inherited-child unit controls ran. The readonly initialization part of revised M06 must use the same private production phase helpers in its regression: actual file creation, directory creation and rename-into fail both before and after readonly model initialization through the original/preacquired ADD handle and ordinary opens; the same operations succeed after append and fail after seal. An external movable-file positive control proves rename inputs are valid. Original root no-DELETE-sharing and no DELETE_CHILD grants remain unchanged. This sequence is a candidate requiring native proof, not an inferred kernel guarantee.

Native `a582fd6e` [39731130988](https://github.com/nessalabs/nessa-agent/actions/runs/39731130988) advanced the receipt: readonly initialization reached 0x9404, but the following raw NtSetSecurityObject append changed input/current 0x9404 to 0x9004. Protected-directory integration again failed 1/10 before later unit controls. Supplying the AI result bit is not a parent-only inheritance-selection mechanism. The bounded M06 transition candidate instead uses the [documented MAX-opened handle exception in SetSecurityInfo](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-setsecurityinfo) only after the owned directory has established modern inheritance. Earlier legacy-to-modern conversion on an occupied existing parent remains disallowed. This is not proof for the mixed access masks: exact constructor and verified-reopen masks, current P/AI, genuine inherited child bytes/control and actual full writes must all pass natively. Raw native model-clearing and legacy occupied-parent mutation remain causal controls.

| M07 | Closed-tree release closes copied descendants, then reopens saved child directories | The existing verified reopen requests MAXIMUM_ALLOWED/SYNCHRONIZE, validates required metadata rights and exact saved native identity before returning. Exercise this actual reopen before terminal raw release beneath a newly owned root with genuine inherited/unprotected foreign children; a verified reopen is not authority for provider transitions. Preserve exact child ACL/control/bytes and writes across seal and reopened release; wrong saved identity refuses before any ACL effect. No arbitrary sessions-parent restoration API is reintroduced |

| M08 | An ordinary legacy-model directory or wrong saved identity reaches a later directory transition | Refuse before ACL effects; only exclusive empty readonly initialization may convert model zero. Keep exact original ACL/control on refusal. A readonly existing sessions-parent fixture uses an admitted owned constructor followed by seal solely to set up that readonly precondition; session acquisition itself still has no ACL effects |

| M09 | A modern sealed owned directory is reopened through the actual verified MAX/SYNCHRONIZE path, then the former provider full-FA release is forced in a test | Require actual inherited-child ACL mutation through that exact reopened authority, reproducing 0125. Contrast with terminal raw full-FA release through the same production reopen in M10 and preserve exact original-constructor provider seal controls. A MAX request alone is not a blanket propagation guarantee. Keep raw native AI-clearing as a separate counterexample |

Native `0125274a` [39734683915](https://github.com/nessalabs/nessa-agent/actions/runs/39734683915) passed protected-directory integration 11/0 and storage units 53/3. Actual readonly phase, original constructor MAX transitions and leaf-pinning controls passed. Verified MAX/SYNCHRONIZE reopen followed by provider release widened inherited child ACE masks while child control remained 0x8404. The contrasting-handle test stopped at that release before its final negative control. Raw native assignment changed parent AI1 to AI0 but left genuinely modern child ACLs unchanged; its prior assumed mutation assertion was false for that fixture. The following terminal-release split is a candidate, not permission to remove exact child comparisons.

| M10 | Owned directory is released after stage finish/live rename, or during failure cleanup | Append and seal retain the qualified original-MAX provider path and strict P/AI contract. Release is a separate terminal raw native full-FA assignment: require original private P/AI before effects, then require exact full-FA DACL and protected AI0 result. Never invoke a later provider/model conversion or retry release on that terminal directory. Original and verified-reopen release must preserve exact foreign ACL/control/bytes and actual writes. Model-zero/repeated-transition refusal preserves exact state. Failed verification remains Retained/restricted, never readiness |
| M11 | Ordinary peer inserts a foreign legacy object while additions are available | Create an external full-FA legacy parent and ordinary genuinely inherited AI0/unprotected writable file. Verify native identity, exact ACL/control and bytes across ordinary hard-link injection into the modern owned tree, qualified provider seal, and terminal raw release, including actual ProtectedDirectories guard Drop after content validation rejects an extra/multi-link file. The external alias must remain unchanged. Also hard-link then ordinarily remove the external name before sealing: require the same legacy inode and exact ACL with one remaining link, so multi-link rejection cannot stand in for safety; preserve held-inode security, data and target-name writes across seal and original/reopened terminal release. Separately move an ordinary legacy empty directory and measure its actual post-move model/protection before the same transitions. Do not assume all foreign children are AI1 or infer safety from a later multi-link rejection. Any feasible posture that changes foreign ACL remains a blocker; do not weaken comparisons |
| M12 | e98 diagnostic query-order control before any sealing/release | e98 passed the original held handle's exact legacy receipt after target open/identity read, then failed the first target GetSecurityInfo equality; the single-link case also queried verify_file before its first paired check. Preserve both failing assertions and all production code. Record bounded native NtQuerySecurityObject DACL/control and Win32 GetSecurityInfo receipts on both handles before/after distinct link, target open, first target provider query, repeated held query, external unlink and verify_file. Use fresh reversed query order to distinguish kernel state changes, per-file-object views and provider normalization; do not reset baselines or print SID bytes. This is diagnostic evidence, not a claim that a getter mutates security |

## Windows session publication lifetime pins

| Phase | Parent authority | Owned leaf authority | Failure outcome |
|---|---|---|---|
| Begin | Retain exact sessions directory without DELETE sharing; no ACL effects. Absent parent requires atomically final ordinary private ACL, also never mutated. | Reserve external source through fresh pinned constructor: protected FR from birth; original RW+DELETE+WRITE_DAC File shares READ only. | No parent effects to restore. Exact source zero+sync; empty external scratch may remain. |
| Source/inspect | Parent identity pinned. Foreign entries/ACLs untouched. | Source gets only duplicate of exact reserved File. Inspect held File VFS. | Exact source bytes retired; primary source error preserved if retired. |
| Publish | No parent sealing. Existing output/sidecar names rejected. | Fresh distinct canonical output, same pinned construction. Validate held digest and exact name binding. Source old writer cannot address output. | Zero exact output/source Files; named leftovers Retained. |
| Note | Verify root spelling, sessions binding, output and sidecar names. | Fresh sidecar protected/pinned from birth. Store+sync; validate exact output held digest and both inode bindings while pins exclude unlink/rename/write. | Retire all held owned Files; never fresh pathname cleanup. |
| Release | No parent ACL effects. | Release ONLY owned output/sidecar ACLs while initial shareREAD-only Files still prohibit WRITE/DELETE access. | Typed Retained if exact owned release fails. |
| Available audit | Parent and owned leaf pins still alive. | No new writer/deleter can act despite released ACL until owner Drop. | Audit failure attempts exact owned retirement before closing pins. No fresh available receipt. |
| Successful Drop | Close exact handles; no namespace cleanup or ACL effects. | Ordinary consumer opens become possible. | No claim of permanent isolation after this boundary. |

The stage/copy root-rename lifecycle is separate: its descendants use protected shareREAD|DELETE constructors, then close-after-seal/verify before root rename. Do not globally change that sharing policy.

Native proof required: same initial DELETE-handle leaf rename succeeds despite no DELETE sharing; preacquired DELETE_CHILD handles cannot delete/rename pinned leaves; write denial survives owned ACL release until original File closes; ordinary read works via shareALL opens and regular SQLite consumer read works after Drop.

Regression matrix: genuine inherited foreign file/directory/grandchild ACL bytes, control and flags unchanged at every phase and setup/fault/Drop; attack after release inside audit callback; old-source writer vs canonical output; exact retirement under failure; ordinary consumer read/write after Drop; cached reads never mutate parent permissions.

Scope: fresh publication has a lifetime-pinned linearization boundary. Cached reads validate exact opened inode/digest and do not retain publication isolation after return. Parent namespace is never sealed, and unrelated entries may change; only expected owned names are bound and pinned.

Native `f4ce1f6e` [39724277229](https://github.com/nessalabs/nessa-agent/actions/runs/39724277229) qualified the empty-name cursor and exact ACL restoration, but storage still failed 49/2. With an existing parent control of 0x9004 unchanged, inherited foreign ACEs lost INHERITED_ACE and became protected (0x8004 to 0x9004). Merely preserving the legacy model cannot provide foreign-child isolation. Its owned-tree control also had explicit children, so no mutation was reproduced. The ordering above removes all sessions-parent ACL effects, and requires readonly-phase modern initialization plus genuinely inherited children for the separate stage/copy lifecycle. These revised mechanisms still require native qualification.

Earlier native receipts disproved selector-only, synchronization-only and MAX-origin isolation hypotheses. [303 CI](https://github.com/nessalabs/nessa-agent/actions/runs/39721179127) also rejected the literal-dot native directory reopen with InvalidFilename 123. [f4ce CI](https://github.com/nessalabs/nessa-agent/actions/runs/39724277229) then passed the empty-counted-name cursor, including retained identity, interleaved and concurrent independent cursors, wrong-path identity and actual shared-duplicate exhaustion controls. Its exact descriptor-restoration controls passed too; production sessions no longer use snapshot/restoration effects.

The remaining inherited-child mutation in f4ce is the reason for the ownership changes in the tables above. The [SetSecurityInfo documentation](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-setsecurityinfo) did not establish isolation for those actual original MAX handles. Fresh session publication now uses lifetime sharing pins and no parent ACL effects. The separate owned stage/copy lifecycle requires verified protected modern inheritance before granting additions and genuinely inherited-child controls. Those new native mechanics remain unqualified until their positive and causal controls pass. No test precondition or observed inheritance flag may be inferred from a full-access mask alone.

The empty-name cursor follows the related-file case in the [Microsoft filesystem sample](https://github.com/microsoft/Windows-driver-samples/blob/main/filesys/fastfat/create.c) and the [NtOpenFile contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntopenfile). Its actual native receipt is distinct from the pending pin/readonly-initialization qualification.

The threat boundary excludes an administrator, process-handle duplication and
an owner deliberately changing the DACL, as Unix directory mode protection also
excludes an owner deliberately changing its mode. Native Windows tests must
exercise the candidate on Windows; Linux compilation does not qualify it.
The existing platform CI owns those tests. No power-loss durability is claimed.

## Durability and evidence limits

Retained publication flushes the writable payload before and after handle rename.
Windows `PrivateDirectory::sync` revalidates identity; it has no directory-fsync
guarantee. No retained write-through move or arbitrary power-loss survival is
claimed. Path-based publication does not acquire this owner's retained directory
authority; the shared rename encoder preserves each entry point's own checks.

Local macOS runtime tests and Windows target compilation are distinct evidence.
Actual Windows runtime tests use the existing local-auth platform CI selection
(storage/auth/server/SDK tests and all-targets Clippy); no new job is required.
Until that runs, Windows runtime behavior remains unverified. Hosted elevated
runner evidence does not establish ordinary-user permissions independently.

Primary references: [FILE_RENAME_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info),
[SetFileInformationByHandle](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle),
[CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
[FlushFileBuffers](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers).
