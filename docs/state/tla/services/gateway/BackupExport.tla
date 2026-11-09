---------------- MODULE BackupExport ----------------
EXTENDS Naturals, FiniteSets

\* Bounded safety abstraction. See the owning page for source mapping and exclusions.
VARIABLES phase, held, pinned, completeOutboxes, readStarted, mac, ack
vars == <<phase, held, pinned, completeOutboxes, readStarted, mac, ack>>
Init == /\ phase = "Idle"
        /\ held = FALSE
        /\ pinned = FALSE
        /\ completeOutboxes = FALSE
        /\ readStarted = FALSE
        /\ mac = FALSE
        /\ ack = FALSE

Hold == /\ (phase = "Idle")
    /\ phase' = "Holding"
    /\ held' = TRUE
    /\ UNCHANGED <<pinned, completeOutboxes, readStarted, mac, ack>>

Pin == /\ (phase = "Holding" /\ held /\ ~pinned)
    /\ pinned' = TRUE
    /\ completeOutboxes' = TRUE
    /\ UNCHANGED <<phase, held, readStarted, mac, ack>>

RefusePin == /\ (phase = "Holding")
    /\ phase' = "Refused"
    /\ held' = FALSE
    /\ UNCHANGED <<pinned, completeOutboxes, readStarted, mac, ack>>

Read == /\ (phase = "Holding" /\ pinned /\ completeOutboxes)
    /\ phase' = "Reading"
    /\ held' = FALSE
    /\ readStarted' = TRUE
    /\ UNCHANGED <<pinned, completeOutboxes, mac, ack>>

Seal == /\ (phase = "Reading" /\ ~held)
    /\ phase' = "Authenticated"
    /\ mac' = TRUE
    /\ UNCHANGED <<held, pinned, completeOutboxes, readStarted, ack>>

RefuseMac == /\ (phase = "Reading")
    /\ phase' = "Refused"
    /\ UNCHANGED <<held, pinned, completeOutboxes, readStarted, mac, ack>>

Publish == /\ (phase = "Authenticated" /\ pinned /\ completeOutboxes /\ mac /\ ~held)
    /\ phase' = "Published"
    /\ ack' = TRUE
    /\ UNCHANGED <<held, pinned, completeOutboxes, readStarted, mac>>

PublishUnconfirmed == /\ (phase = "Authenticated")
    /\ phase' = "Unconfirmed"
    /\ UNCHANGED <<held, pinned, completeOutboxes, readStarted, mac, ack>>

Next == Hold \/ Pin \/ RefusePin \/ Read \/ Seal \/ RefuseMac \/ Publish \/ PublishUnconfirmed
Spec == Init /\ [][Next]_vars
TypeOK == /\ (phase \in {"Idle", "Holding", "Reading", "Authenticated", "Published", "Unconfirmed", "Refused"})
    /\ (held \in BOOLEAN)
    /\ (pinned \in BOOLEAN)
    /\ (completeOutboxes \in BOOLEAN)
    /\ (readStarted \in BOOLEAN)
    /\ (mac \in BOOLEAN)
    /\ (ack \in BOOLEAN)
AckRequiresCompleteCut == (ack => (pinned /\ completeOutboxes /\ mac /\ ~held))
ReadingReleasesAdmission == (readStarted => ~held)
============================================================================
