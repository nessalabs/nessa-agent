---- MODULE VoiceGeneration ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES generation, current, pending, connected, closed, published
vars == <<generation, current, pending, connected, closed, published>>
Init == /\ generation = 0 /\ current = 0 /\ pending = {} /\ connected = {} /\ closed = {} /\ published = 0
Start == /\ generation < 2 /\ generation' = generation + 1 /\ current' = generation + 1 /\ pending' = pending \cup {generation + 1} /\ closed' = closed \cup connected /\ connected' = {} /\ published' = 0
End == /\ current # 0 /\ current' = 0 /\ closed' = closed \cup connected /\ connected' = {} /\ published' = 0 /\ UNCHANGED <<generation,pending>>
Connect == \E call \in pending: /\ pending' = pending \ {call} /\ connected' = (IF call = current THEN {call} ELSE connected) /\ closed' = (IF call # current THEN closed \cup {call} ELSE closed) /\ published' = (IF call = current THEN call ELSE published) /\ UNCHANGED <<generation,current>>
Refuse == \E call \in pending: /\ pending' = pending \ {call} /\ current' = (IF call = current THEN 0 ELSE current) /\ UNCHANGED <<generation,connected,closed,published>>
Next == Start \/ End \/ Connect \/ Refuse
CurrentConnection == connected \subseteq {current} \ {0}
NoReleasedConnection == connected \cap closed = {}
PublicationIdentity == published = 0 \/ published = current
Spec == Init /\ [][Next]_vars
====
