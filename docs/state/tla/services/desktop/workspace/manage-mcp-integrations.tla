---- MODULE McpSettingsRequest ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES seq, pending, held, completed, published
vars == <<seq, pending, held, completed, published>>
Init == /\ seq = 0 /\ pending = 0 /\ held = "none" /\ completed = {} /\ published = 0
Request == \E kind \in {"list","save","remove","authorize"}: /\ pending = 0 /\ seq < 2 /\ seq' = seq + 1 /\ pending' = seq + 1 /\ held' = kind /\ UNCHANGED <<completed,published>>
Complete == \E reply \in 1..2: /\ reply = pending /\ completed' = completed \cup {reply} /\ published' = reply /\ pending' = 0 /\ held' = "none" /\ UNCHANGED seq
Stale == \E reply \in 1..2: /\ reply # pending /\ UNCHANGED vars
Next == Request \/ Complete \/ Stale
PendingIdentity == pending = 0 \/ pending = seq
CompletionIdentity == published = 0 \/ published \in completed
SlotMeaning == (pending = 0) = (held = "none")
NoFuturePublication == published <= seq
Spec == Init /\ [][Next]_vars
====
