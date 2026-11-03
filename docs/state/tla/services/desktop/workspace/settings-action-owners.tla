---- MODULE SettingsActionOwners ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES binding, native, notice, nativeOwner, noticeOwner, publishedNative, replayed
vars == <<binding, native, notice, nativeOwner, noticeOwner, publishedNative, replayed>>
Init == /\ binding = 1 /\ native = "Ready" /\ notice = "NotAsked" /\ nativeOwner = 0 /\ noticeOwner = 0 /\ publishedNative = 0 /\ replayed = 0
Save == /\ native = "Ready" /\ native' = "Writing" /\ nativeOwner' = binding /\ UNCHANGED <<binding,notice,noticeOwner,publishedNative,replayed>>
WriteAnswer == /\ native = "Writing" /\ nativeOwner = binding /\ native' = "Rereading" /\ UNCHANGED <<binding,notice,nativeOwner,noticeOwner,publishedNative,replayed>>
ReadAnswer == /\ native = "Rereading" /\ nativeOwner = binding /\ native' = "Ready" /\ publishedNative' = binding /\ nativeOwner' = 0 /\ UNCHANGED <<binding,notice,noticeOwner,replayed>>
Ask == /\ notice = "NotAsked" /\ notice' = "Asking" /\ noticeOwner' = binding /\ UNCHANGED <<binding,native,nativeOwner,publishedNative,replayed>>
Grant == /\ noticeOwner # 0 /\ noticeOwner = binding /\ notice' = "Granted" /\ noticeOwner' = 0 /\ replayed' = noticeOwner /\ UNCHANGED <<binding,native,nativeOwner,publishedNative>>
Reject == /\ noticeOwner # 0 /\ noticeOwner = binding /\ notice' = "NotAsked" /\ noticeOwner' = 0 /\ UNCHANGED <<binding,native,nativeOwner,publishedNative,replayed>>
Rebind == /\ binding = 1 /\ binding' = 2 /\ native' = "Ready" /\ notice' = "NotAsked" /\ publishedNative' = 0 /\ replayed' = 0 /\ UNCHANGED <<nativeOwner,noticeOwner>>
Next == Save \/ WriteAnswer \/ ReadAnswer \/ Ask \/ Grant \/ Reject \/ Rebind
NativePublication == publishedNative = 0 \/ publishedNative = binding
GrantReplay == replayed = 0 \/ (replayed = binding /\ notice = "Granted")
Spec == Init /\ [][Next]_vars
====
