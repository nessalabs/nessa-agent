---------------- MODULE PeerDiscovery ----------------
EXTENDS Naturals
VARIABLES field, phase, remoteCredential, credential, present, running,
          announced, publishStep
vars == <<field, phase, remoteCredential, credential, present, running,
          announced, publishStep>>

Init == /\ field = "none" /\ phase = "Active"
        /\ remoteCredential = "current" /\ credential = "current"
        /\ present = TRUE /\ running = TRUE
        /\ announced = FALSE /\ publishStep = FALSE

Key == /\ field = "none" /\ present /\ running
       /\ field' = "keyed" /\ publishStep' = FALSE
       /\ UNCHANGED <<phase, remoteCredential, credential, present, running, announced>>
Offer == /\ field = "keyed" /\ present /\ running
         /\ field' = "offered" /\ publishStep' = FALSE
         /\ UNCHANGED <<phase, remoteCredential, credential, present, running, announced>>
Install == /\ field = "offered" /\ present /\ running
           /\ field' = "installed" /\ publishStep' = FALSE
           /\ UNCHANGED <<phase, remoteCredential, credential, present, running, announced>>

\* Remote expiry is not evidence already available to this gateway.
Expire == /\ remoteCredential' = "expired" /\ publishStep' = FALSE
          /\ UNCHANGED <<field, phase, credential, present, running, announced>>
ObserveNotCurrent == /\ present /\ running /\ remoteCredential = "expired"
                     /\ credential' = "noncurrent" /\ publishStep' = FALSE
                     /\ UNCHANGED <<field, phase, remoteCredential, present, running, announced>>
Forget == /\ present /\ present' = FALSE /\ publishStep' = FALSE
          /\ UNCHANGED <<field, phase, remoteCredential, credential, running, announced>>
\* Stop covers owner shutdown and admitted authority whose evidence cannot persist.
Stop == /\ running /\ running' = FALSE /\ publishStep' = FALSE
        /\ UNCHANGED <<field, phase, remoteCredential, credential, present, announced>>

Refresh == /\ running
           /\ announced' = (present /\ field = "installed" /\ phase = "Active" /\ credential = "current")
           /\ publishStep' = announced'
           /\ UNCHANGED <<field, phase, remoteCredential, credential, present, running>>
Withdraw == /\ ~running /\ announced' = FALSE /\ publishStep' = FALSE
            /\ UNCHANGED <<field, phase, remoteCredential, credential, present, running>>
Next == Key \/ Offer \/ Install \/ Expire \/ ObserveNotCurrent \/ Forget \/ Stop \/ Refresh \/ Withdraw
\* Only scheduler progress is assumed. A remote answer is never assumed to arrive.
Spec == Init /\ [][Next]_vars /\ WF_vars(Refresh) /\ WF_vars(Withdraw)

TypeOK == /\ field \in {"none", "keyed", "offered", "installed"}
          /\ phase = "Active"
          /\ remoteCredential \in {"current", "expired"}
          /\ credential \in {"current", "noncurrent"}
          /\ present \in BOOLEAN /\ running \in BOOLEAN
          /\ announced \in BOOLEAN /\ publishStep \in BOOLEAN
AnnounceOnlyInstalled == announced => (field = "installed" /\ phase = "Active")
PublishOnlyObservedCurrent == publishStep => (credential = "current" /\ present /\ running)
ExpiryKeepsPhase == phase = "Active"
ObservedNoncurrentEventuallyQuiet == [](credential = "noncurrent" \/ ~present \/ ~running => <>~announced)
=============================================================================
