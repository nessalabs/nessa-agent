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
            precondition(status.failure() == nil && status.purgeFailure() == nil)
            precondition(status.purgeAttempted())
            precondition(refresh.read() == nil && refresh.recheck() == nil)
            let purge = status.purge()!
            precondition(purge.receiver() == receiver && purge.records() > 0)
            precondition(purge.transcripts() > 0 && purge.catalogueEntries() >= 2)
            let recovered = try native.purgeReceipt(receiver: receiver)!
            precondition(recovered.observedAtMs() == purge.observedAtMs())
            precondition(recovered.records() == purge.records())
            do {
                _ = try native.transcript(conversation: conversation, revision: revision)
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
            precondition(status.receipt()?.organization() != nil && status.receipt()?.resource() != nil)
            precondition(refresh.selectionFailure() == nil && refresh.recheck() == nil)
            let read = refresh.read()!
            precondition(read.failure() == nil && read.complete() == true)
            precondition(read.conversations()! >= 2 && read.withdrawn().isEmpty)
            if mode == "resumed" { precondition(read.moved() == false) }
        }
        let page = try native.cataloguePage(after: nil, maximum: 16)!
        do {
            _ = try native.cataloguePage(after: nil, maximum: UInt32.max)
            fatalError("Core catalogue bound was bypassed")
        } catch NativeCallError.Session(let failure) {
            precondition(failure.kind() == .cache && failure.cache() == .cache)
        }
        precondition(page.scope().receiver() == receiver && page.scope().origin() == origin)
        precondition(page.scope().sameAs(other: page.scope()))
        precondition(page.generation() > 0 && page.activeBoundary() == nil)
        precondition(page.entries().contains { $0.key().id() == conversation && !$0.deleted() })
        let snapshot = try native.transcript(conversation: conversation, revision: revision)
        precondition(snapshot.state() == .retained)
        precondition(snapshot.downloaded()! > 0 && snapshot.downloaded() == snapshot.applied())
        precondition(snapshot.facts()! > 0 && snapshot.generation()! > 0)
        precondition(snapshot.scope()?.origin() == origin && snapshot.scope()?.receiver() == receiver)
        let original = try snapshot.viewJson()!
        let encoded = try JSONSerialization.jsonObject(with: Data(original.utf8)) as! [String: Any]
        precondition(encoded["conversationId"] as? String == conversation)
        try native.close()
        let afterClose = try snapshot.viewJson()
        precondition(afterClose == original)
        precondition(snapshot.downloaded()! > 0 && page.generation() > 0)
        do {
            _ = try native.cataloguePage(after: nil, maximum: 16)
            fatalError("Closed session admitted another read")
        } catch NativeCallError.Session(let failure) {
            precondition(failure.kind() == .closed)
            precondition(failure.cache() == nil && failure.enrollment() == nil)
        }
        let reopened = try NativeDeviceSession.open(root: root, directory: directory, address: address)
        let next = try reopened.transcript(conversation: conversation, revision: revision)
        precondition(next.downloaded() == snapshot.downloaded() && next.applied() == snapshot.applied())
        let reopenedView = try next.viewJson()
        precondition(reopenedView == original)
        try reopened.close()
        print("PASS: Swift \(mode) actual paired cache, scope, UInt64 progress, owned snapshots and closed-session error")
    }
}
