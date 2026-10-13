# Upload targets — issue 814

The global file-only lookup originated in `b39136ff65b5ea7a201413b53fb92da632db5296`
(PR 87). Issue 814 replaces that lookup with the enclosing local parent.

A file's upload belongs to the local conversation that admitted it. A valid file
identity may also appear in another draft; it is not authority to update that
other conversation. The conversation application owns upload transitions and
resolves the enclosing conversation before the file. `UploadChange` now requires the parent and its admitted placement;
[attachment lifetimes](attachment-lifetimes.md) separates that placement from the
shared resource identity. The panel carries the original conversation through hashing
and staging, and the store carries it through preparation and remote replies.

The existing upload states and predecessor checks remain the transition owner.
Remote image references describe storage in the captured server conversation;
they do not prove storage in another conversation. Explicit remove/re-add now mints a fresh placement in issue 816. Retries within
a retained placement continue using the existing predecessor transitions.

## Ordering and enforcement

| Row | Input or ordering | Required result | Enforcer |
| --- | --- | --- | --- |
| UP1 | Same retained resource admitted in two local conversations | Progress/result changes only the captured parent | Parent/placement `changeUpload`, application and store tests |
| UP2 | Hashing waits while active conversation changes | Progress/failure/stage retain the original parent | `uploadImage`, panel tests |
| UP3 | Preparation or stage waits; original parent is removed and another parent holds that resource | Success/failure leaves the other parent unchanged | `stageAttachment`, store tests |
| UP4 | Original file or parent removed; aborted staging rejects | No file is resurrected; another parent's file remains unchanged | Existing predecessor checks and parent lookup, store tests |
| UP5 | Correct target stages or fails, then retries | Existing success/failure/retry behavior retained | Application, panel and store tests |
| UP6 | Caller omits parent from progress | Type check rejects before runtime | Required `UploadChange` and `UploadPorts.change`, compile checks |

Resource/placement lifetime is owned by issue 816. Local tab restoration lifetime is owned by issue 812. Once restoration assigns
fresh local IDs, UP3 also protects a restored parent: upload completion retains
the retired parent's ID instead of searching every draft. No saved snapshot or
wire contract changes. The runtime predicates here do not authenticate raw IDs.
