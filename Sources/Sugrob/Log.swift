import Foundation

final class Log: @unchecked Sendable {
    static let shared = Log()

    private let queue = DispatchQueue(label: "sugrob.log")
    private let handle: FileHandle?
    private let formatter: DateFormatter

    private init() {
        let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/Sugrob")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent("spike.log")
        FileManager.default.createFile(atPath: url.path, contents: nil)
        handle = try? FileHandle(forWritingTo: url)
        formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss.SSS"
    }

    func write(_ message: String) {
        let now = Date()
        queue.async { [self] in
            let line = "\(formatter.string(from: now)) \(message)\n"
            handle?.write(Data(line.utf8))
        }
    }
}
