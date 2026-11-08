import AppKit

/// Обновляется из GitHub Releases. Работает только у приложения в /Applications,
/// чтобы не заменять сборку разработчика.
@MainActor
final class Updater {
    private struct Release: Decodable {
        struct Asset: Decodable {
            let name: String
            let browser_download_url: URL
        }
        let tag_name: String
        let assets: [Asset]
    }

    static let currentVersion = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"

    private let repo = Bundle.main.object(forInfoDictionaryKey: "SugrobRepo") as? String
    private var timer: Timer?
    private var isUpdating = false

    func start() {
        guard Bundle.main.bundlePath.hasPrefix("/Applications/") else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 30) { [weak self] in self?.check() }
        timer = Timer.scheduledTimer(withTimeInterval: 6 * 3600, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.check() }
        }
    }

    func check() {
        guard !isUpdating, let repo, !repo.isEmpty,
              Bundle.main.bundlePath.hasPrefix("/Applications/"),
              let url = URL(string: "https://api.github.com/repos/\(repo)/releases/latest") else { return }
        isUpdating = true
        Task { @MainActor in
            defer { isUpdating = false }
            do {
                try await update(from: url)
            } catch {
                Log.shared.write("update: \(error.localizedDescription)")
            }
        }
    }

    private func update(from url: URL) async throws {
        var request = URLRequest(url: url)
        request.setValue("Sugrob-updater", forHTTPHeaderField: "User-Agent")
        let (data, _) = try await URLSession.shared.data(for: request)
        let release = try JSONDecoder().decode(Release.self, from: data)
        let version = release.tag_name.hasPrefix("v") ? String(release.tag_name.dropFirst()) : release.tag_name
        guard version.compare(Self.currentVersion, options: .numeric) == .orderedDescending,
              let asset = release.assets.first(where: { $0.name == "Sugrob.zip" }) else { return }

        Log.shared.write("update: downloading \(version)")
        let (archive, _) = try await URLSession.shared.download(from: asset.browser_download_url)
        let workspace = FileManager.default.temporaryDirectory.appendingPathComponent("sugrob-update-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: workspace, withIntermediateDirectories: true)
        _ = try await Self.run("/usr/bin/ditto", ["-x", "-k", archive.path, workspace.path])

        // Ставим только сборку с той же подписью, что и у текущего приложения.
        let newApp = workspace.appendingPathComponent("Sugrob.app").path
        let currentApp = Bundle.main.bundlePath
        _ = try await Self.run("/usr/bin/codesign", ["--verify", "--deep", newApp])
        let newRequirement = try await Self.run("/usr/bin/codesign", ["-d", "-r-", newApp])
        let currentRequirement = try await Self.run("/usr/bin/codesign", ["-d", "-r-", currentApp])
        guard Self.designated(newRequirement) == Self.designated(currentRequirement) else {
            Log.shared.write("update: signature mismatch, skipped")
            return
        }

        let script = workspace.appendingPathComponent("install.sh")
        try """
        #!/bin/bash
        while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
        rm -rf "$3.old"
        mv "$3" "$3.old" && mv "$2" "$3" && rm -rf "$3.old"
        open "$3"
        """.write(to: script, atomically: true, encoding: .utf8)

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/bash")
        process.arguments = [script.path, String(getpid()), newApp, currentApp]
        try process.run()
        Log.shared.write("update: installing \(version)")
        NSApp.terminate(nil)
    }

    private static func designated(_ output: String) -> String {
        output.split(separator: "\n").first { $0.hasPrefix("designated") }.map(String.init) ?? ""
    }

    private struct ToolError: LocalizedError {
        let errorDescription: String?
    }

    /// Запускает утилиту и возвращает её вывод (stdout и stderr).
    private nonisolated static func run(_ tool: String, _ arguments: [String]) async throws -> String {
        try await Task.detached {
            let process = Process()
            let pipe = Pipe()
            process.executableURL = URL(fileURLWithPath: tool)
            process.arguments = arguments
            process.standardOutput = pipe
            process.standardError = pipe
            try process.run()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            let output = String(decoding: data, as: UTF8.self)
            guard process.terminationStatus == 0 else {
                throw ToolError(errorDescription: "\(tool): \(output)")
            }
            return output
        }.value
    }
}
