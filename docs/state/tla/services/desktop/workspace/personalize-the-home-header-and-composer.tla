---- MODULE HeaderImageOwnership ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES serial, memory, lastChoice, touched, readPending, writes, durable, palette, decoded
vars == <<serial, memory, lastChoice, touched, readPending, writes, durable, palette, decoded>>
Init == /\ serial = 0 /\ memory = 0 /\ lastChoice = 0 /\ touched = FALSE /\ readPending = TRUE /\ writes = {} /\ durable = 0 /\ palette = 0 /\ decoded = {}
Choose == /\ serial < 2 /\ serial' = serial + 1 /\ memory' = serial + 1 /\ lastChoice' = serial + 1 /\ touched' = TRUE /\ writes' = writes \cup {serial + 1} /\ decoded' = decoded \cup {serial + 1} /\ palette' = 0 /\ UNCHANGED <<readPending,durable>>
InitialRead == /\ readPending /\ readPending' = FALSE /\ memory' = (IF touched THEN memory ELSE 0) /\ UNCHANGED <<serial,lastChoice,touched,writes,durable,palette,decoded>>
Persist == \E version \in writes: /\ durable' = version /\ writes' = writes \ {version} /\ UNCHANGED <<serial,memory,lastChoice,touched,readPending,palette,decoded>>
WriteRefused == \E version \in writes: /\ writes' = writes \ {version} /\ UNCHANGED <<serial,memory,lastChoice,touched,readPending,durable,palette,decoded>>
Decode == \E version \in decoded: /\ decoded' = decoded \ {version} /\ palette' = (IF version = memory THEN version ELSE palette) /\ UNCHANGED <<serial,memory,lastChoice,touched,readPending,writes,durable>>
Next == Choose \/ InitialRead \/ Persist \/ WriteRefused \/ Decode
ChoiceOutranksInitialRead == touched => memory = lastChoice
PaletteCurrentImage == palette = 0 \/ palette = memory
PersistedVersionExists == durable <= serial
Spec == Init /\ [][Next]_vars
====
