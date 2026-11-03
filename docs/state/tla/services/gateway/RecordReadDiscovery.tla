---- MODULE RecordReadDiscovery ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, started, elapsed, steps, validated, targetOccupied, otherOccupied, held
vars == <<phase, started, elapsed, steps, validated, targetOccupied, otherOccupied, held>>
Init == /\ phase = "Queued" /\ started = FALSE /\ elapsed = FALSE /\ steps = 0 /\ validated = 0 /\ targetOccupied = TRUE /\ otherOccupied = TRUE /\ held = TRUE
WorkerStart == /\ phase = "Queued" /\ started' = TRUE /\ phase' = "Discovering" /\ UNCHANGED <<elapsed,steps,validated,targetOccupied,otherOccupied,held>>
Occupied == /\ phase = "Discovering" /\ targetOccupied /\ ~elapsed /\ steps < 2 /\ phase' = "Waiting" /\ UNCHANGED <<started,elapsed,steps,validated,targetOccupied,otherOccupied,held>>
ReleaseTarget == /\ targetOccupied /\ targetOccupied' = FALSE /\ UNCHANGED <<phase,started,elapsed,steps,validated,otherOccupied,held>>
Reoccupy == /\ phase = "Discovering" /\ ~targetOccupied /\ targetOccupied' = TRUE /\ UNCHANGED <<phase,started,elapsed,steps,validated,otherOccupied,held>>
Resume == /\ phase = "Waiting" /\ ~targetOccupied /\ ~elapsed /\ phase' = "Discovering" /\ UNCHANGED <<started,elapsed,steps,validated,targetOccupied,otherOccupied,held>>
ValidateStep == /\ phase = "Discovering" /\ ~targetOccupied /\ ~elapsed /\ steps < 2 /\ steps' = steps + 1 /\ validated' = validated + 1 /\ UNCHANGED <<phase,started,elapsed,targetOccupied,otherOccupied,held>>
Ready == /\ phase = "Discovering" /\ ~targetOccupied /\ steps < 2 /\ (~elapsed \/ steps = 0) /\ phase' = "Ready" /\ UNCHANGED <<started,elapsed,steps,validated,targetOccupied,otherOccupied,held>>
Elapse == /\ started /\ ~elapsed /\ phase \in {"Discovering","Waiting"} /\ elapsed' = TRUE /\ UNCHANGED <<phase,started,steps,validated,targetOccupied,otherOccupied,held>>
Bound == /\ phase \in {"Discovering","Waiting"} /\ (elapsed \/ steps = 2) /\ phase' = "Preparing" /\ UNCHANGED <<started,elapsed,steps,validated,targetOccupied,otherOccupied,held>>
Join == /\ phase \in {"Ready","Preparing"} /\ phase' = "Joined" /\ held' = FALSE /\ UNCHANGED <<started,elapsed,steps,validated,targetOccupied,otherOccupied>>
Next == WorkerStart \/ Occupied \/ ReleaseTarget \/ Reoccupy \/ Resume \/ ValidateStep \/ Ready \/ Elapse \/ Bound \/ Join
Spec == Init /\ [][Next]_vars
OccupiedDoesNotSpendSteps == steps = validated
QueueDoesNotSpendWorkBudget == ~started => ~elapsed
OwnStreamCanResume == phase = "Waiting" /\ ~targetOccupied /\ ~elapsed => ENABLED Resume
CapacityUntilSourceJoin == phase # "Joined" => held
====
