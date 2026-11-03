---------------- MODULE ChildPreparation ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, audit, durable, submitted, sealed
vars == <<phase, audit, durable, submitted, sealed>>
Init == phase = "Reserved" /\ audit = FALSE /\ durable = FALSE /\ submitted = FALSE /\ sealed = FALSE
Audit == phase = "Reserved" /\ audit' = TRUE /\ UNCHANGED <<phase, durable, submitted, sealed>>
Prepare == phase = "Reserved" /\ audit /\ ~sealed /\ phase' = "Prepared" /\ UNCHANGED <<audit, durable, submitted, sealed>>
Save == phase = "Prepared" /\ durable' = TRUE /\ UNCHANGED <<phase, audit, submitted, sealed>>
PublishReady == phase = "Prepared" /\ durable /\ ~sealed /\ phase' = "Ready" /\ UNCHANGED <<audit, durable, submitted, sealed>>
InitialSubmit == phase = "Ready" /\ ~sealed /\ phase' = "TaskAdmitted" /\ submitted' = TRUE /\ UNCHANGED <<audit, durable, sealed>>
LoseSubmit == phase = "TaskAdmitted" /\ phase' = "Uncertain" /\ UNCHANGED <<audit, durable, submitted, sealed>>
Seal == ~sealed /\ sealed' = TRUE /\ UNCHANGED <<phase, audit, durable, submitted>>
Drain == sealed /\ phase' = "Closing" /\ UNCHANGED <<audit, durable, submitted, sealed>>
Next == Audit \/ Prepare \/ Save \/ PublishReady \/ InitialSubmit \/ LoseSubmit \/ Seal \/ Drain
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Reserved", "Prepared", "Ready", "TaskAdmitted", "Uncertain", "Closing"} /\ audit \in BOOLEAN /\ durable \in BOOLEAN /\ submitted \in BOOLEAN /\ sealed \in BOOLEAN
FactoryAfterAudit == phase \in {"Prepared", "Ready", "TaskAdmitted", "Uncertain"} => audit
TaskNeedsReady == submitted => durable /\ audit
ReadyDurable == phase = "Ready" => durable
=============================================================================
