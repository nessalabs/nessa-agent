import Foundation

private enum CheckFailure: Error { case deadline }
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var value = 0
    func increment() { lock.withLock { value += 1 } }
    func count() -> Int { lock.withLock { value } }
}

@main struct VerifyWorker {
    static let address = "127.0.0.1:9"

    static func main() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false,
                                               attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: root) }
        for directory in ["held", "pre-stop", "pre-close", "drop"] {
            try FileManager.default.createDirectory(at: root.appendingPathComponent(directory),
                withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        }
        let path = root.resolvingSymlinksInPath().path
        try await heldAdmission(path)
        try await stoppedBeforeDispatch(path)
        try await closeBeforeDispatch(path)
        try await abandonedWorker(path)
        print("PASS: Swift pre-queue Busy, cancellation, shared close, physical scope drain and abandoned-worker cleanup")
    }

    static func wait(_ signal: DispatchSemaphore) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            DispatchQueue.global().async {
                if signal.wait(timeout: .now() + 5) == .success { continuation.resume() }
                else { continuation.resume(throwing: CheckFailure.deadline) }
            }
        }
    }

    static func expectLocked(_ root: String, _ directory: String) async throws {
        do {
            let unexpected = try await NativeSessionWorker.open(root: root, directory: directory, address: address)
            try await unexpected.close()
            fatalError("Private scope released while physical work remained")
        } catch NativeCallError.Session(let failure) {
            precondition(failure.kind() == .privateState && failure.privateState()?.kind() == .locked)
        }
    }

    static func heldAdmission(_ root: String) async throws {
        let worker = try await NativeSessionWorker.open(root: root, directory: "held", address: address)
        let entered = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        let result = Task {
            try await worker.perform { session, stop -> Int in
                precondition(!Thread.isMainThread)
                entered.signal()
                guard release.wait(timeout: .now() + 5) == .success else { throw CheckFailure.deadline }
                precondition(stop.isStopped())
                do {
                    _ = try session.status()
                    fatalError("Swift close did not signal the native owner before physical drain")
                } catch NativeCallError.Session(let failure) {
                    precondition(failure.kind() == .closed)
                }
                return 17
            }
        }
        try await wait(entered)
        do {
            _ = try await worker.status()
            fatalError("Second caller was queued behind the held operation")
        } catch NativeWorkerError.busy {}
        result.cancel()
        do {
            _ = try await worker.status()
            fatalError("Canceled waiter released physical admission")
        } catch NativeWorkerError.busy {}
        let drain = worker.beginClose()
        let canceledWaiter = Task { try await drain.value }
        canceledWaiter.cancel()
        let joined = worker.beginClose()
        do {
            _ = try await worker.refresh()
            fatalError("Closing session admitted new work")
        } catch NativeWorkerError.closed {}
        try await expectLocked(root, "held")
        release.signal()
        let completed = try await result.value
        precondition(completed == 17, "Cancellation erased an already admitted physical result")
        try await canceledWaiter.value
        try await joined.value
        try await worker.close()
        let reopened = try await NativeSessionWorker.open(root: root, directory: "held", address: address)
        try await reopened.close()
    }

    static func stoppedBeforeDispatch(_ root: String) async throws {
        let queue = DispatchQueue(label: "nessa.check.pre-stop")
        let worker = try await NativeSessionWorker.open(root: root, directory: "pre-stop", address: address, queue: queue)
        let effects = Counter()
        queue.suspend()
        let result = Task {
            try await worker.perform { _, _ in effects.increment(); return 1 }
        }
        result.cancel()
        queue.resume()
        do {
            _ = try await result.value
            fatalError("Pre-dispatch cancellation was lost")
        } catch is CancellationError {}
        precondition(effects.count() == 0)
        try await worker.close()
    }

    static func closeBeforeDispatch(_ root: String) async throws {
        let queue = DispatchQueue(label: "nessa.check.pre-close")
        let worker = try await NativeSessionWorker.open(root: root, directory: "pre-close", address: address, queue: queue)
        queue.suspend()
        let drain = worker.beginClose()
        do {
            _ = try await worker.status()
            fatalError("Close-before-dispatch did not fence admission")
        } catch NativeWorkerError.closed {}
        try await expectLocked(root, "pre-close")
        queue.resume()
        try await drain.value
        let reopened = try await NativeSessionWorker.open(root: root, directory: "pre-close", address: address)
        try await reopened.close()
    }

    static func abandonedWorker(_ root: String) async throws {
        let queue = DispatchQueue(label: "nessa.check.drop")
        var worker: NativeSessionWorker? = try await NativeSessionWorker.open(
            root: root, directory: "drop", address: address, queue: queue)
        weak let abandoned = worker
        queue.suspend()
        worker = nil
        precondition(abandoned == nil, "Worker leaked its native scope")
        try await expectLocked(root, "drop")
        queue.resume()
        let drained = DispatchSemaphore(value: 0)
        queue.async { drained.signal() }
        try await wait(drained)
        let reopened = try await NativeSessionWorker.open(root: root, directory: "drop", address: address)
        try await reopened.close()
    }
}
