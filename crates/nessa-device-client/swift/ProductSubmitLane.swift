import Dispatch
import Foundation

/// One product submission hand-off at a time. Later callers wait in arrival
/// order. The state table is ADR 1138.
///
/// The body is the native hand-off only. The worker waits for the reply after
/// `handoff` returns, on a different queue.
final class ProductSubmitLane: @unchecked Sendable {
    private let lock = NSLock()
    private let submissions: DispatchQueue
    private var queue: [any SubmitAttempt] = []
    private var handingOff = false
    private var closed = false

    init(submissions: DispatchQueue = DispatchQueue(label: "nessa.product.submit")) {
        self.submissions = submissions
    }

    /// How many callers are waiting behind the current hand-off.
    /// Verification reads this. Admission does not branch on it.
    var waitingCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return queue.count
    }

    /// Fail every queued caller with `NativeWorkerError.closed`. A hand-off
    /// whose body has started keeps running. A later `handoff` fails the same way.
    func close() {
        let abandoned: [any SubmitAttempt]
        lock.lock()
        closed = true
        abandoned = queue
        queue.removeAll()
        lock.unlock()
        for attempt in abandoned {
            attempt.settleIfWaiting(NativeWorkerError.closed)
        }
    }

    func handoff<T: Sendable>(_ body: @escaping @Sendable () throws -> T) async throws -> T {
        let attempt = HandoffAttempt<T>(body)
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            return try await withCheckedThrowingContinuation { continuation in
                attempt.arm(continuation)
                admit(attempt)
            }
        } onCancel: {
            cancel(attempt)
        }
    }

    private func admit(_ attempt: any SubmitAttempt) {
        lock.lock()
        if attempt.isSettled {
            lock.unlock()
            return
        }
        if closed {
            lock.unlock()
            attempt.settleIfWaiting(NativeWorkerError.closed)
            return
        }
        if handingOff {
            queue.append(attempt)
            lock.unlock()
            return
        }
        handingOff = true
        lock.unlock()
        begin(attempt)
    }

    private func begin(_ attempt: any SubmitAttempt) {
        submissions.async { [self] in
            attempt.runIfCurrent()
            self.promote()
        }
    }

    private func promote() {
        lock.lock()
        if closed {
            handingOff = false
            let abandoned = queue
            queue.removeAll()
            lock.unlock()
            for attempt in abandoned {
                attempt.settleIfWaiting(NativeWorkerError.closed)
            }
            return
        }
        while !queue.isEmpty {
            let next = queue.removeFirst()
            if next.isSettled { continue }
            lock.unlock()
            begin(next)
            return
        }
        handingOff = false
        lock.unlock()
    }

    private func cancel(_ attempt: any SubmitAttempt) {
        lock.lock()
        let laneClosed = closed
        if let index = queue.firstIndex(where: { ObjectIdentifier($0) == ObjectIdentifier(attempt) }) {
            queue.remove(at: index)
        }
        lock.unlock()
        // Queued attempts have not started. A started body ignores this and
        // keeps the result it is already producing, including after shutdown.
        attempt.settleIfWaiting(laneClosed ? NativeWorkerError.closed : CancellationError())
    }
}

private protocol SubmitAttempt: AnyObject, Sendable {
    func runIfCurrent()
    /// Settle only when the body has not started. The body owns the result after that.
    func settleIfWaiting(_ error: any Error)
    var isSettled: Bool { get }
}

private final class HandoffAttempt<T: Sendable>: SubmitAttempt, @unchecked Sendable {
    private let lock = NSLock()
    private let body: @Sendable () throws -> T
    private var continuation: CheckedContinuation<T, Error>?
    private var outcome: Result<T, any Error>?
    private var cancelRequested = false
    private var startedBody = false

    init(_ body: @escaping @Sendable () throws -> T) {
        self.body = body
    }

    func arm(_ continuation: CheckedContinuation<T, Error>) {
        lock.lock()
        if let outcome {
            lock.unlock()
            continuation.resume(with: outcome)
            return
        }
        self.continuation = continuation
        lock.unlock()
    }

    var isSettled: Bool {
        lock.lock()
        defer { lock.unlock() }
        return outcome != nil
    }

    func settleIfWaiting(_ error: any Error) {
        lock.lock()
        cancelRequested = true
        let suppress = startedBody || outcome != nil
        lock.unlock()
        if !suppress {
            finish(.failure(error))
        }
    }

    func runIfCurrent() {
        lock.lock()
        if outcome != nil || cancelRequested {
            lock.unlock()
            return
        }
        startedBody = true
        lock.unlock()
        let result: Result<T, any Error>
        do { result = .success(try body()) }
        catch { result = .failure(error) }
        finish(result)
    }

    private func finish(_ result: Result<T, any Error>) {
        lock.lock()
        if outcome != nil {
            lock.unlock()
            return
        }
        outcome = result
        let continuation = self.continuation
        lock.unlock()
        continuation?.resume(with: result)
    }
}
