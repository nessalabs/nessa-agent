---------------- MODULE Skills ----------------
EXTENDS Naturals
\* Finite safety abstraction of SEP-2640 (io.modelcontextprotocol/skills) client
\* discovery, manifest-bound verification, refresh, and optional directory read.
\* Stuttering and incomplete operations are allowed.
VARIABLES capSkills, capDirRead, serverRev, retainedRev, approvedRev, loadedRev, dirReadDone, deliveredUnverified
vars == <<capSkills, capDirRead, serverRev, retainedRev, approvedRev, loadedRev, dirReadDone, deliveredUnverified>>

Init ==
    /\ capSkills \in BOOLEAN
    /\ capDirRead \in BOOLEAN
    /\ (capDirRead => capSkills)
    /\ serverRev = 1
    /\ retainedRev = 0
    /\ approvedRev = 0
    /\ loadedRev = 0
    /\ dirReadDone = FALSE
    /\ deliveredUnverified = FALSE

\* Discover or refresh the skill entry via skills/list or skills/get.
\* If the manifest revision changed, any prior manifest-bound approval and
\* loaded skill state for the previous revision are invalidated.
RefreshEntry ==
    /\ capSkills
    /\ retainedRev' = serverRev
    /\ approvedRev' = IF approvedRev = serverRev THEN approvedRev ELSE 0
    /\ loadedRev' = IF loadedRev = serverRev THEN loadedRev ELSE 0
    /\ UNCHANGED <<capSkills, capDirRead, serverRev, dirReadDone, deliveredUnverified>>

\* Bind approval to the currently retained manifest revision.
ApproveRetained ==
    /\ capSkills
    /\ retainedRev > 0
    /\ approvedRev' = retainedRev
    /\ UNCHANGED <<capSkills, capDirRead, serverRev, retainedRev, loadedRev, dirReadDone, deliveredUnverified>>

\* Server publishes a new revision of the skill out-of-band.
ServerMutatesSkill ==
    /\ serverRev = 1
    /\ serverRev' = 2
    /\ UNCHANGED <<capSkills, capDirRead, retainedRev, approvedRev, loadedRev, dirReadDone, deliveredUnverified>>

\* Read SKILL.md or a listed supporting file whose size, digest, and frontmatter
\* match the retained and approved manifest.
ReadVerified ==
    /\ capSkills
    /\ retainedRev > 0
    /\ approvedRev = retainedRev
    /\ serverRev = retainedRev
    /\ loadedRev' = retainedRev
    /\ UNCHANGED <<capSkills, capDirRead, serverRev, retainedRev, approvedRev, dirReadDone, deliveredUnverified>>

\* A read that fails manifest containment, byte size, SHA-256 digest, or
\* frontmatter verification is rejected without delivering content.
ReadRejected ==
    /\ capSkills
    /\ retainedRev > 0
    /\ UNCHANGED vars

\* Optional resources/directory/read requires directoryRead: true and never
\* extends the retained manifest or grants approval.
ReadDirectory ==
    /\ capSkills
    /\ capDirRead
    /\ dirReadDone' = TRUE
    /\ UNCHANGED <<capSkills, capDirRead, serverRev, retainedRev, approvedRev, loadedRev, deliveredUnverified>>

Next ==
    \/ RefreshEntry
    \/ ApproveRetained
    \/ ServerMutatesSkill
    \/ ReadVerified
    \/ ReadRejected
    \/ ReadDirectory

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ capSkills \in BOOLEAN
    /\ capDirRead \in BOOLEAN
    /\ serverRev \in {1, 2}
    /\ retainedRev \in {0, 1, 2}
    /\ approvedRev \in {0, 1, 2}
    /\ loadedRev \in {0, 1, 2}
    /\ dirReadDone \in BOOLEAN
    /\ deliveredUnverified \in BOOLEAN

NoRetainedEntryWithoutCapability == retainedRev > 0 => capSkills
NoDirReadWithoutCapability == dirReadDone => (capSkills /\ capDirRead)
LoadedMatchesRetainedAndApproved == loadedRev > 0 => (capSkills /\ loadedRev = retainedRev /\ loadedRev = approvedRev)
NeverDeliveredUnverified == ~deliveredUnverified
=============================================================================
