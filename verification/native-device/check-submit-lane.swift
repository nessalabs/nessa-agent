import Dispatch
import Foundation

private enum Wait<T: Sendable>: Sendable {
    case value(T)
    case cancelled
    case closed
    case deadline
    case body
    case other(String)
}

extension Wait: Equatable where T: Equatable {}

private final class Log: @unchecked Sendable {
    private let lock = NSLock()
    private var depth = 0
    private var order: [Int] = []
    private var overlapped = false

    func enter(_ id: Int) {
        lock.lock()
        depth += 1
        if depth != 1 { overlapped = true }
        order.append(id)
        lock.unlock()
    }

    func leave() {
        lock.lock()
        depth -= 1
        lock.unlock()
    }

    func snapshot() -> (order: [Int], overlapped: Bool) {
        lock.lock()
        defer { lock.unlock() }
        return (order, overlapped)
    }
}

private func lane() -> ProductSubmitLane {
    ProductSubmitLane(submissions: DispatchQueue(
        label: "nessa.test.submit", attributes: .concurrent))
}

private func wait(_ signal: DispatchSemaphore) async -> Bool {
    await withCheckedContinuation { continuation in
        DispatchQueue.global().async {
            continuation.resume(returning: signal.wait(timeout: .now() + 5) == .success)
        }
    }
}

private func poll(_ ready: @escaping @Sendable () -> Bool) async -> Bool {
    let start = Date()
    while !ready() {
        if Date().timeIntervalSince(start) > 5 { return false }
        try? await Task.sleep(nanoseconds: 10_000_000)
    }
    return true
}

private final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if done { return false }
        done = true
        return true
    }
}

/// A hung submission must fail the run. Waiting on the task inside a group
/// would keep the group alive after the deadline, so the deadline resumes
/// on its own and `exit` ends anything still blocked.
private func waitFor<T: Sendable>(_ task: Task<T, Error>) async -> Wait<T> {
    await withCheckedContinuation { (continuation: CheckedContinuation<Wait<T>, Never>) in
        let once = Once()
        let sleeper = Task {
            try? await Task.sleep(nanoseconds: 5_000_000_000)
            if once.claim() { continuation.resume(returning: .deadline) }
        }
        Task {
            let outcome: Wait<T>
            do { outcome = .value(try await task.value) }
            catch is CancellationError { outcome = .cancelled }
            catch NativeWorkerError.closed { outcome = .closed }
            catch is BodyBroke { outcome = .body }
            catch { outcome = .other(String(describing: error)) }
            sleeper.cancel()
            if once.claim() { continuation.resume(returning: outcome) }
        }
    }
}

private func hold(_ lane: ProductSubmitLane, _ id: Int, _ log: Log) -> (Task<Int, Error>, DispatchSemaphore, DispatchSemaphore) {
    let entered = DispatchSemaphore(value: 0)
    let release = DispatchSemaphore(value: 0)
    let task = Task {
        try await lane.handoff { () -> Int in
            log.enter(id)
            entered.signal()
            guard release.wait(timeout: .now() + 5) == .success else { throw CheckFailure.deadline }
            log.leave()
            return id
        }
    }
    return (task, entered, release)
}

private enum CheckFailure: Error, Sendable { case deadline }
private struct BodyBroke: Error, Sendable {}

@main struct CheckSubmitLane {
    static func main() async {
        guard await ordered() else { exit(1) }
        guard await cancelledWhileQueued() else { exit(1) }
        guard await failedBodyReleasesTheNext() else { exit(1) }
        guard await shutdownWithWaiters() else { exit(1) }
        guard await closedWhileIdle() else { exit(1) }
        print("PASS: product submissions run one at a time, in order; a cancelled waiter leaves; shutdown fails the queue")
    }

    static func ordered() async -> Bool {
        let lane = lane()
        let log = Log()
        let (first, entered, release) = hold(lane, 0, log)
        guard await wait(entered) else { return fail("held hand-off did not start") }
        var tasks: [Task<Int, Error>] = []
        for id in 1...8 {
            tasks.append(Task {
                try await lane.handoff { () -> Int in
                    log.enter(id)
                    Thread.sleep(forTimeInterval: 0.01)
                    log.leave()
                    return id
                }
            })
            guard await poll({ lane.waitingCount >= id }) else {
                return fail("submission \(id) did not wait behind the current hand-off")
            }
        }
        release.signal()
        guard await waitFor(first) == .value(0) else { return fail("held hand-off did not return its own value") }
        var got: [Int] = []
        for task in tasks {
            guard case .value(let id) = await waitFor(task) else { return fail("a queued submission did not return") }
            got.append(id)
        }
        let shot = log.snapshot()
        guard !shot.overlapped else { return fail("two hand-offs ran at once: \(shot.order)") }
        guard got == Array(1...8), shot.order == Array(0...8) else {
            return fail("submissions finished out of order: \(shot.order)")
        }
        return true
    }

    static func cancelledWhileQueued() async -> Bool {
        let lane = lane()
        let log = Log()
        let (first, entered, release) = hold(lane, 0, log)
        guard await wait(entered) else { return fail("held hand-off did not start") }
        let early = Task { try await lane.handoff { () -> Int in log.enter(1); log.leave(); return 1 } }
        guard await poll({ lane.waitingCount >= 1 }) else { return fail("first waiter did not queue") }
        let dropped = Task { try await lane.handoff { () -> Int in log.enter(2); log.leave(); return 2 } }
        guard await poll({ lane.waitingCount >= 2 }) else { return fail("cancelled waiter did not queue") }
        let late = Task { try await lane.handoff { () -> Int in log.enter(3); log.leave(); return 3 } }
        guard await poll({ lane.waitingCount >= 3 }) else { return fail("waiter behind the cancelled one did not queue") }
        dropped.cancel()
        guard await waitFor(dropped) == .cancelled else {
            return fail("cancelled waiter did not leave while a hand-off was still held")
        }
        guard lane.waitingCount == 2 else { return fail("cancelled waiter stayed in the queue") }
        release.signal()
        guard await waitFor(first) == .value(0) else { return fail("held hand-off did not finish after a cancel") }
        guard await waitFor(early) == .value(1) else { return fail("waiter in front of the cancelled one did not run") }
        guard await waitFor(late) == .value(3) else { return fail("waiter behind the cancelled one did not run") }
        let shot = log.snapshot()
        guard shot.order == [0, 1, 3], !shot.overlapped else {
            return fail("cancel disturbed the other submissions: \(shot.order)")
        }
        return true
    }

    static func failedBodyReleasesTheNext() async -> Bool {
        let lane = lane()
        let log = Log()
        let (first, entered, release) = hold(lane, 0, log)
        guard await wait(entered) else { return fail("held hand-off did not start") }
        let failing = Task {
            try await lane.handoff { () -> Int in
                log.enter(1)
                log.leave()
                throw BodyBroke()
            }
        }
        guard await poll({ lane.waitingCount >= 1 }) else { return fail("failing submission did not queue") }
        let next = Task {
            try await lane.handoff { () -> Int in
                log.enter(2)
                log.leave()
                return 2
            }
        }
        guard await poll({ lane.waitingCount >= 2 }) else { return fail("submission behind a failure did not queue") }
        release.signal()
        guard await waitFor(first) == .value(0) else { return fail("held hand-off did not return") }
        guard await waitFor(failing) == .body else { return fail("a body error was replaced or lost") }
        guard await waitFor(next) == .value(2) else { return fail("the submission behind a failed hand-off did not run") }
        let shot = log.snapshot()
        guard shot.order == [0, 1, 2], !shot.overlapped else {
            return fail("a body error stalled or reordered the lane: \(shot.order)")
        }
        return true
    }

    static func shutdownWithWaiters() async -> Bool {
        let lane = lane()
        let log = Log()
        let (first, entered, release) = hold(lane, 0, log)
        guard await wait(entered) else { return fail("held hand-off did not start") }
        let queued = (1...2).map { id in
            Task { try await lane.handoff { () -> Int in log.enter(id); log.leave(); return id } }
        }
        guard await poll({ lane.waitingCount >= 2 }) else { return fail("waiters were not queued before shutdown") }
        lane.close()
        guard lane.waitingCount == 0 else { return fail("shutdown left waiters in the queue") }
        for task in queued {
            guard await waitFor(task) == .closed else {
                return fail("a queued waiter hung or did not fail closed during shutdown")
            }
        }
        guard log.snapshot().order == [0] else { return fail("shutdown ran a waiter that was still queued") }
        release.signal()
        guard await waitFor(first) == .value(0) else {
            return fail("the hand-off already running did not finish after shutdown")
        }
        let after = Task { try await lane.handoff { () -> Int in log.enter(9); log.leave(); return 9 } }
        guard await waitFor(after) == .closed else { return fail("a submission after shutdown did not fail closed") }
        guard log.snapshot().order == [0] else { return fail("a submission after shutdown ran its body") }
        return true
    }

    static func closedWhileIdle() async -> Bool {
        let lane = lane()
        lane.close()
        let task = Task { try await lane.handoff { () -> Int in 1 } }
        guard await waitFor(task) == .closed else { return fail("a closed idle lane accepted a submission") }
        return true
    }

    static func fail(_ message: String) -> Bool {
        print("FAIL: \(message)")
        return false
    }
}
