---------------- MODULE CredentialReceipt ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES issued, revoked, receipt, secretReturns, firstRevocation, replays
vars == <<issued, revoked, receipt, secretReturns, firstRevocation, replays>>
Init == issued = FALSE /\ revoked = FALSE /\ receipt = FALSE /\ secretReturns = 0 /\ firstRevocation = 0 /\ replays = 0
Issue == ~issued /\ issued' = TRUE /\ receipt' = TRUE /\ secretReturns' = secretReturns + 1 /\ UNCHANGED <<revoked, firstRevocation, replays>>
Revoke == issued /\ ~revoked /\ revoked' = TRUE /\ firstRevocation' = 1 /\ UNCHANGED <<issued, receipt, secretReturns, replays>>
Replay == receipt /\ replays = 0 /\ replays' = 1 /\ UNCHANGED <<issued, revoked, receipt, secretReturns, firstRevocation>>
RepeatedRevoke == revoked /\ firstRevocation' = firstRevocation /\ UNCHANGED <<issued, revoked, receipt, secretReturns, replays>>
Next == Issue \/ Revoke \/ Replay \/ RepeatedRevoke
Spec == Init /\ [][Next]_vars
TypeOK == issued \in BOOLEAN /\ revoked \in BOOLEAN /\ receipt \in BOOLEAN /\ secretReturns \in 0..2 /\ firstRevocation \in 0..1 /\ replays \in 0..1
BearerReturnedOnce == secretReturns <= 1
ReceiptRequiresOriginal == receipt => issued
RevocationHasOriginalFact == revoked => firstRevocation = 1
=============================================================================
