---------------- MODULE BackupRestore ----------------
EXTENDS Naturals, FiniteSets
\* Two stages/digests; retained readiness receipts can arrive out of order.
VARIABLES phase, generation, actualDigest, sealedDigest, marker, receipts,
          candidate, spent, acceptedGeneration, acceptedDigest, spentBeforeAccept, live, durable
vars == <<phase, generation, actualDigest, sealedDigest, marker, receipts,
          candidate, spent, acceptedGeneration, acceptedDigest, spentBeforeAccept, live, durable>>
Init == /\ phase = "Pending" /\ generation = 1 /\ actualDigest = 1
        /\ sealedDigest = 0 /\ marker = FALSE /\ receipts = {}
        /\ candidate = <<0, 0>> /\ spent = {} /\ acceptedGeneration = 0
        /\ acceptedDigest = 0 /\ spentBeforeAccept = FALSE /\ live = FALSE /\ durable = FALSE
Seal == /\ phase = "Pending" /\ phase' = "Sealed" /\ sealedDigest' = actualDigest
        /\ UNCHANGED <<generation, actualDigest, marker, receipts, candidate, spent,
                       acceptedGeneration, acceptedDigest, spentBeforeAccept, live, durable>>
Marker == /\ phase = "Sealed" /\ ~marker /\ marker' = TRUE
          /\ UNCHANGED <<phase, generation, actualDigest, sealedDigest, receipts,
                         candidate, spent, acceptedGeneration, acceptedDigest, spentBeforeAccept, live, durable>>
Ready == /\ phase = "Sealed" /\ marker /\ sealedDigest = actualDigest
         /\ generation \notin spent /\ phase' = "Ready"
         /\ receipts' = receipts \cup {<<generation, sealedDigest>>}
         /\ UNCHANGED <<generation, actualDigest, sealedDigest, marker, candidate,
                        spent, acceptedGeneration, acceptedDigest, spentBeforeAccept, live, durable>>
PresentReceipt == /\ ~live /\ candidate' \in receipts
                  /\ UNCHANGED <<phase, generation, actualDigest, sealedDigest, marker,
                                 receipts, spent, acceptedGeneration, acceptedDigest, spentBeforeAccept, live, durable>>
ReplaceStage == /\ generation = 1 /\ ~live /\ phase' = "Pending" /\ generation' = 2
                /\ sealedDigest' = 0 /\ marker' = FALSE
                /\ UNCHANGED <<actualDigest, receipts, candidate, spent, acceptedGeneration,
                               acceptedDigest, spentBeforeAccept, live, durable>>
ChangeJournal == /\ actualDigest = 1 /\ ~live /\ actualDigest' = 2
                 /\ UNCHANGED <<phase, generation, sealedDigest, marker, receipts,
                                candidate, spent, acceptedGeneration, acceptedDigest, spentBeforeAccept, live, durable>>
RetireStage == /\ ~live /\ generation \notin spent /\ spent' = spent \cup {generation}
               /\ UNCHANGED <<phase, generation, actualDigest, sealedDigest, marker,
                              receipts, candidate, acceptedGeneration, acceptedDigest, spentBeforeAccept, live, durable>>
CurrentReady == phase = "Ready" /\ candidate \in receipts
                /\ candidate[1] = generation /\ candidate[2] = actualDigest
                /\ sealedDigest = actualDigest /\ generation \notin spent /\ ~live
Accept == /\ CurrentReady /\ phase' = "Live" /\ acceptedGeneration' = candidate[1]
          /\ acceptedDigest' = candidate[2] /\ spent' = spent \cup {generation}
          /\ spentBeforeAccept' = (generation \in spent) /\ live' = TRUE /\ durable' = TRUE
          /\ UNCHANGED <<generation, actualDigest, sealedDigest, marker, receipts, candidate>>
AcceptUnsynced == /\ CurrentReady /\ phase' = "Unconfirmed" /\ acceptedGeneration' = candidate[1]
                  /\ acceptedDigest' = candidate[2] /\ spent' = spent \cup {generation}
                  /\ spentBeforeAccept' = (generation \in spent) /\ live' = TRUE
                  /\ UNCHANGED <<generation, actualDigest, sealedDigest, marker, receipts, candidate, durable>>
Next == Seal \/ Marker \/ Ready \/ PresentReceipt \/ ReplaceStage \/ ChangeJournal \/ RetireStage \/ Accept \/ AcceptUnsynced
Spec == Init /\ [][Next]_vars
TypeOK == /\ phase \in {"Pending", "Sealed", "Ready", "Live", "Unconfirmed"}
          /\ generation \in 1..2 /\ actualDigest \in 1..2 /\ sealedDigest \in 0..2
          /\ marker \in BOOLEAN /\ receipts \subseteq ((1..2) \X (1..2))
          /\ candidate \in ((0..2) \X (0..2)) /\ spent \subseteq 1..2
          /\ acceptedGeneration \in 0..2 /\ acceptedDigest \in 0..2
          /\ spentBeforeAccept \in BOOLEAN /\ live \in BOOLEAN /\ durable \in BOOLEAN
AcceptanceHasCurrentProof == live => (acceptedGeneration = generation /\ acceptedDigest = sealedDigest /\ acceptedDigest = actualDigest /\ ~spentBeforeAccept /\ <<acceptedGeneration, acceptedDigest>> \in receipts /\ acceptedGeneration \in spent)
DurabilityRequiresPublication == durable => live
============================================================================
