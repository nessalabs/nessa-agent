import Foundation

@main struct VerifyPaired {
    static func main() async throws {
        let arguments = Array(CommandLine.arguments.dropFirst())
        precondition(arguments.count == 7)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            DispatchQueue(label: "nessa.native.paired.verification").async {
                do { try check(arguments); continuation.resume() }
                catch { continuation.resume(throwing: error) }
            }
        }
        let worker = try await NativeSessionWorker.open(
            root: arguments[1], directory: arguments[2], address: arguments[3])
        do {
            let localMatches = try await worker.localEnrollment(capture: { local in
                precondition(!Thread.isMainThread)
                if arguments[0] == "terminal" { return local == nil }
                guard let local = local, let issued = local.issued() else { return false }
                return local.deviceKey() == expectedDeviceKey()
                    && local.gatewayPin().count == 44 && issued.receiver() == arguments[5]
                    && issued.credential() == ProcessInfo.processInfo.environment["NESSA_NATIVE_EXPECTED_CREDENTIAL_ID"]!
                    && local.publicIntent().consentClass() == .deviceRead
                    && local.publicIntent().consentClassName() == expectedConsentClassName()
            })
            precondition(localMatches)
            if arguments[0] == "terminal" {
                let count = try await worker.purgeReceipt(receiver: arguments[5], capture: { receipt in
                    precondition(!Thread.isMainThread)
                    return receipt?.records() ?? 0
                })
                precondition(count > 0)
            } else {
                let stamp = try await worker.bindSavedReceiver()
                let progress = try await worker.publishedTranscript(
                    stamp: stamp,
                    conversation: arguments[4], revision: "00000000-0000-4000-8000-000000000001",
                    capture: { snapshot in
                        precondition(!Thread.isMainThread)
                        let json = try snapshot.viewJson()!
                        let view = try JSONSerialization.jsonObject(with: Data(json.utf8)) as! [String: Any]
                        precondition(view["conversationId"] as? String == arguments[4])
                        return snapshot.downloaded()!
                    })
                precondition(progress > 0)
            }
            try await worker.close()
        } catch {
            try? await worker.close()
            throw error
        }
        print("PASS: Swift \(arguments[0]) owned capture serialization/decoding or historical purge receipt")
    }
    static func check(_ arguments: [String]) throws {
        precondition(!Thread.isMainThread)
        let (mode, root, directory, address) = (arguments[0], arguments[1], arguments[2], arguments[3])
        let (conversation, receiver, origin) = (arguments[4], arguments[5], arguments[6])
        let native = try NativeDeviceSession.open(root: root, directory: directory, address: address)
        defer { try? native.close() }
        let revision = "00000000-0000-4000-8000-000000000001"
        let stopped = NativeReadStop()
        stopped.stop()
        do {
            _ = try native.refresh(stop: stopped)
            fatalError("Pre-dispatch stop was lost")
        } catch NativeCallError.Session(let failure) {
            precondition(failure.kind() == .stopped)
        }
        do {
            _ = try native.enroll(code: "invalid")
            fatalError("Malformed code was accepted")
        } catch NativeCallError.Session(let failure) {
            precondition(failure.kind() == .enrollment)
            precondition(failure.enrollment()?.kind() == .crypto)
            precondition(failure.enrollment()?.crypto() == .invalidCode)
        }
        if mode == "terminal" {
            let refresh = try native.refresh(stop: NativeReadStop())
            let status = refresh.initial()
            precondition(status.completionCaptured())
            precondition(status.receipt()?.phase() == .terminal)
            precondition(status.receipt()?.terminalCause() == .credentialRevoked)
            let intent = status.receipt()!.publicIntent()
            precondition(intent.invitation().count == 16 && intent.attempt().count == 16)
            precondition(intent.consent().count == 16 && intent.consentClass() == .deviceRead)
            precondition(intent.consentClassName() == expectedConsentClassName())
            precondition(status.failure() == nil && status.purgeFailure() == nil)
            precondition(status.purgeAttempted())
            let local = try native.localEnrollment()
            precondition(local == nil)
            precondition(refresh.read() == nil && refresh.recheck() == nil)
            let purge = status.purge()!
            precondition(purge.receiver() == receiver && purge.records() > 0)
            precondition(purge.transcripts() > 0 && purge.catalogueEntries() >= 2)
            let recovered = try native.purgeReceipt(receiver: receiver)!
            precondition(recovered.observedAtMs() == purge.observedAtMs())
            precondition(recovered.records() == purge.records())
            do {
                _ = try native.bindSavedReceiver()
                fatalError("Removed issued receiver was used as saved-read authority")
            } catch NativeCallError.Session(let failure) {
                precondition(failure.kind() == .noCredential)
            }
            try native.close()
            precondition(purge.records() == recovered.records())
            print("PASS: Swift terminal status, independent purge, receipt recovery and typed no-credential refusal")
            return
        }
        if mode != "offline" {
            let stop = NativeReadStop()
            let refresh = try native.refresh(stop: stop)
            let status = refresh.initial()
            precondition(status.completionCaptured() && !status.purgeAttempted())
            precondition(status.receipt()?.phase() == .active && status.failure() == nil)
            precondition(status.receipt()?.receiver() == receiver)
            let receipt = status.receipt()!
            let intent = receipt.publicIntent()
            precondition(intent.invitation().count == 16 && intent.attempt().count == 16)
            precondition(intent.consent().count == 16 && intent.consentClass() == .deviceRead)
            precondition(intent.consentClassName() == expectedConsentClassName())
            precondition(intent.generation() > 0 && intent.expiryMs() > 0)
            precondition(receipt.sameIntent(other: receipt))
            precondition(status.receipt()?.organization() != nil && status.receipt()?.resource() != nil)
            precondition(refresh.selectionFailure() == nil)
            let read = refresh.read()!
            if mode == "epoch" {
                precondition(read.failure() == .refused)
                let recheck = refresh.recheck()!
                precondition(recheck.completionCaptured() && recheck.failure()?.kind() == .refused)
                precondition(recheck.receipt() == nil && !recheck.purgeAttempted())
                precondition(recheck.purge() == nil && recheck.purgeFailure() == nil)
                let purge = try native.purgeReceipt(receiver: receiver)
                precondition(purge == nil)
            } else {
                precondition(refresh.recheck() == nil)
                precondition(read.failure() == nil && read.complete() == true)
                precondition(read.conversations()! >= 2 && read.withdrawn().isEmpty)
            }
            if mode == "resumed" { precondition(read.moved() == false) }
        }
        let stamp = try native.bindSavedReceiver()
        let page = try native.publishedCataloguePage(stamp: stamp, after: nil, maximum: 16)!
        let local = try native.localEnrollment()!
        precondition(local.deviceKey() == expectedDeviceKey())
        precondition(local.issued()?.receiver() == receiver)
        precondition(local.issued()?.credential() == ProcessInfo.processInfo.environment["NESSA_NATIVE_EXPECTED_CREDENTIAL_ID"]!)
        precondition(local.gatewayPin().count == 44)
        precondition(local.publicIntent().consentClassName() == expectedConsentClassName())
        do {
            _ = try native.publishedCataloguePage(stamp: stamp, after: nil, maximum: UInt32.max)
            fatalError("Core catalogue bound was bypassed")
        } catch NativeCallError.Session(let failure) {
            precondition(failure.kind() == .cache && failure.cache() == .cache)
        }
        precondition(page.scope().receiver() == receiver && page.scope().origin() == origin)
        precondition(page.scope().sameAs(other: page.scope()))
        precondition(page.generation() > 0 && page.activeBoundary() == nil)
        precondition(page.entries().contains { $0.key().id() == conversation && !$0.deleted() })
        let key = page.entries().first!.key()
        let restored = try NativeEntryKey(creation: key.creation(), id: key.id())
        precondition(restored.creation() == key.creation() && restored.id() == key.id())
        let following = try native.publishedCataloguePage(stamp: stamp, after: restored, maximum: 16)!
        precondition(!following.entries().contains { $0.key().id() == key.id() })
        let snapshot = try native.publishedTranscript(stamp: stamp, conversation: conversation, revision: revision)
        precondition(snapshot.state() == .retained)
        precondition(snapshot.downloaded()! > 0 && snapshot.generation()! > 0)
        if mode == "epoch" {
            // The first page ends inside a semantic unit. Its confirmed physical
            // prefix survives, without claiming that the incomplete unit applied.
            precondition(snapshot.downloaded()! > snapshot.applied()!)
            precondition(snapshot.facts()! <= snapshot.applied()!)
        } else {
            precondition(snapshot.downloaded() == snapshot.applied() && snapshot.facts()! > 0)
        }
        precondition(snapshot.scope()?.origin() == origin && snapshot.scope()?.receiver() == receiver)
        let original = try snapshot.viewJson()!
        let encoded = try JSONSerialization.jsonObject(with: Data(original.utf8)) as! [String: Any]
        precondition(encoded["conversationId"] as? String == conversation)
        try native.close()
        let afterClose = try snapshot.viewJson()
        precondition(afterClose == original)
        precondition(snapshot.downloaded()! > 0 && page.generation() > 0)
        do {
            _ = try native.publishedCataloguePage(stamp: stamp, after: nil, maximum: 16)
            fatalError("Closed session admitted another read")
        } catch NativeCallError.Session(let failure) {
            precondition(failure.kind() == .closed)
            precondition(failure.cache() == nil && failure.enrollment() == nil)
        }
        let reopened = try NativeDeviceSession.open(root: root, directory: directory, address: address)
        let rebound = try reopened.bindSavedReceiver()
        let next = try reopened.publishedTranscript(stamp: rebound, conversation: conversation, revision: revision)
        precondition(next.downloaded() == snapshot.downloaded() && next.applied() == snapshot.applied())
        let reopenedView = try next.viewJson()
        if mode == "epoch" {
            var expected = encoded
            precondition(expected["transcriptState"] as? String == "unknown")
            expected["transcriptState"] = "stale"
            let actual = try JSONSerialization.jsonObject(with: Data(reopenedView!.utf8)) as! [String: Any]
            precondition(NSDictionary(dictionary: actual) == NSDictionary(dictionary: expected))
        } else {
            precondition(reopenedView == original)
        }
        try reopened.close()
        print("PASS: Swift \(mode) actual paired cache, scope, UInt64 progress, owned snapshots and closed-session error")
    }
    static func expectedConsentClassName() -> String {
        // Supplied independently by the Rust paired owner through Auth's wire
        // publication, rather than deriving text from the generated Swift enum.
        return ProcessInfo.processInfo.environment["NESSA_NATIVE_EXPECTED_CONSENT_CLASS"]!
    }
    static func expectedDeviceKey() -> Data {
        let text = ProcessInfo.processInfo.environment["NESSA_NATIVE_EXPECTED_DEVICE_KEY"]!
        precondition(text.utf8.count == 64)
        var result = Data()
        var start = text.startIndex
        while start != text.endIndex {
            let end = text.index(start, offsetBy: 2)
            result.append(UInt8(text[start..<end], radix: 16)!)
            start = end
        }
        return result
    }
}
