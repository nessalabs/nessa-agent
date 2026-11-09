---- MODULE ParentStop ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, captured, children, rootDigest, rootSaved, validated, attempted, confirmed, declined, effects, later, sealed
vars == <<phase, captured, children, rootDigest, rootSaved, validated, attempted, confirmed, declined, effects, later, sealed>>
Init == /\ phase = "Capturing" /\ captured = {1,2} /\ children = {} /\ rootDigest = {} /\ rootSaved = FALSE /\ validated = FALSE /\ attempted = {} /\ confirmed = {} /\ declined = {} /\ effects = {} /\ later = FALSE /\ sealed = FALSE
SaveChild == /\ phase = "Capturing" /\ children' = {2} /\ phase' = "ChildrenSaved" /\ UNCHANGED <<captured,rootDigest,rootSaved,validated,attempted,confirmed,declined,effects,later,sealed>>
SaveRoot == /\ phase = "ChildrenSaved" /\ rootDigest' = captured /\ rootSaved' = TRUE /\ validated' = TRUE /\ phase' = "Committed" /\ UNCHANGED <<captured,children,attempted,confirmed,declined,effects,later,sealed>>
LateTurn == /\ ~later /\ later' = TRUE /\ UNCHANGED <<phase,captured,children,rootDigest,rootSaved,validated,attempted,confirmed,declined,effects,sealed>>
Crash == /\ phase = "Committed" /\ phase' = "Recovering" /\ validated' = FALSE /\ UNCHANGED <<captured,children,rootDigest,rootSaved,attempted,confirmed,declined,effects,later,sealed>>
CorruptCandidate == /\ phase = "Recovering" /\ children' = children \cup {3} /\ UNCHANGED <<phase,captured,rootDigest,rootSaved,validated,attempted,confirmed,declined,effects,later,sealed>>
Restore == /\ phase = "Recovering" /\ rootSaved /\ rootDigest = ({1} \cup children) /\ validated' = TRUE /\ phase' = "Restored" /\ UNCHANGED <<captured,children,rootDigest,rootSaved,attempted,confirmed,declined,effects,later,sealed>>
Attempt == /\ phase \in {"Committed","Restored"} /\ rootSaved /\ validated /\ \E target \in ({1} \cup children) \ attempted: /\ attempted' = attempted \cup {target} /\ effects' = effects \cup {target} /\ UNCHANGED <<phase,captured,children,rootDigest,rootSaved,validated,confirmed,declined,later,sealed>>
Confirm == /\ \E target \in attempted \ confirmed: /\ confirmed' = confirmed \cup {target} /\ UNCHANGED <<phase,captured,children,rootDigest,rootSaved,validated,attempted,declined,effects,later,sealed>>
Decline == /\ \E target \in confirmed \ declined: /\ declined' = declined \cup {target} /\ UNCHANGED <<phase,captured,children,rootDigest,rootSaved,validated,attempted,confirmed,effects,later,sealed>>
Next == SaveChild \/ SaveRoot \/ LateTurn \/ Crash \/ CorruptCandidate \/ Restore \/ Attempt \/ Confirm \/ Decline
Spec == Init /\ [][Next]_vars
ExactCapturedEffects == effects \subseteq captured
RootBeforeEffect == effects # {} => rootSaved
ReceiptBeforeDecline == declined \subseteq confirmed /\ confirmed \subseteq attempted
NoLifetimeSeal == ~sealed
====
