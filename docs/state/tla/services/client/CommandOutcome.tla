---------------- MODULE CommandOutcome ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, dispatched, wire, reply, settled
vars == <<phase, dispatched, wire, reply, settled>>
Init == phase = "Waiting" /\ dispatched = FALSE /\ wire = 0 /\ reply = 0 /\ settled = FALSE
Dispatch == phase = "Waiting" /\ dispatched' = TRUE /\ UNCHANGED <<phase, wire, reply, settled>>
Lost == phase = "Waiting" /\ phase' = "OutcomeUnknown" /\ UNCHANGED <<dispatched, wire, reply, settled>>
Retry == phase = "OutcomeUnknown" /\ wire = 0 /\ phase' = "Waiting" /\ wire' = 1 /\ UNCHANGED <<dispatched, reply, settled>>
Reply == phase = "Waiting" /\ reply = wire /\ phase' = "Acknowledged" /\ settled' = TRUE /\ UNCHANGED <<dispatched, wire, reply>>
LateReply == phase = "Waiting" /\ wire = 1 /\ reply = 0 /\ reply' = 1 /\ UNCHANGED <<phase, dispatched, wire, settled>>
Refuse == phase = "Waiting" /\ ~dispatched /\ phase' = "Refused" /\ UNCHANGED <<dispatched, wire, reply, settled>>
Lookup == phase = "OutcomeUnknown" /\ phase' = "ReceiptLookup" /\ UNCHANGED <<dispatched, wire, reply, settled>>
Recover == phase = "ReceiptLookup" /\ phase' = "Reconciled" /\ settled' = TRUE /\ UNCHANGED <<dispatched, wire, reply>>
Next == Dispatch \/ Lost \/ Retry \/ Reply \/ LateReply \/ Refuse \/ Lookup \/ Recover
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Waiting", "OutcomeUnknown", "ReceiptLookup", "Reconciled", "Acknowledged", "Refused"} /\ dispatched \in BOOLEAN /\ wire \in 0..1 /\ reply \in 0..1 /\ settled \in BOOLEAN
ReplyNeedsCurrentWire == phase = "Acknowledged" => reply = wire
RefusalBeforeEffect == phase = "Refused" => ~dispatched
UnknownNotSettled == phase = "OutcomeUnknown" => ~settled
=============================================================================
