import Foundation
import SwiftUI

/// Disposable verification app. No gateway connection, enrollment or user data.
@main struct NativeSessionCheckApp: App {
    var body: some Scene { WindowGroup { NativeSessionCheckView() } }
}
private struct NativeSessionCheckView: View {
    @State private var result = "Checking native session…"
    var body: some View {
        Text(result).padding().task {
            do {
                let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
                try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false,
                                                       attributes: [.posixPermissions: 0o700])
                defer { try? FileManager.default.removeItem(at: root) }
                let directory = root.appendingPathComponent("private")
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
                                                       attributes: [.posixPermissions: 0o700])
                let path = root.resolvingSymlinksInPath().path
                let worker = try await NativeSessionWorker.open(root: path, directory: "private", address: "127.0.0.1:9")
                let offMain = try await worker.perform { _, _ in !Thread.isMainThread }
                precondition(offMain)
                do {
                    _ = try await worker.bindSavedReceiver()
                    result = "FAIL: unpaired cache was accepted"
                    try await worker.close()
                    return
                } catch NativeCallError.Session(let failure) {
                    precondition(failure.kind() == .noCredential)
                }
                try await worker.close()
                do {
                    _ = try await worker.status()
                    result = "FAIL: closed worker admitted status"
                    return
                } catch NativeWorkerError.closed {}
                let reopened = try await NativeSessionWorker.open(root: path, directory: "private", address: "127.0.0.1:9")
                try await reopened.close()
                result = "PASS: release native session\nOff-UI worker and typed refusal\nPrivate state close/reopen"
            } catch { result = "FAIL: native session \(type(of: error))" }
        }
    }
}
