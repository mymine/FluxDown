import FluxDomain
import Foundation
import UniformTypeIdentifiers

// MARK: - 拖放（iPad / Stage Manager）

/// 拖入 N1 的文件：`NSItemProvider` 的文件表示只在回调内有效，先按扩展名归类，再把（受限大小的）内容暂存到临时目录，
/// 交给与文件选择器相同的导入路径，用完 `discard`。
nonisolated enum DroppedFiles {
    nonisolated enum Kind: Sendable, Hashable {
        #if FULL_FLAVOR
        case torrent
        #endif
        /// `.txt` / `.url` / `.list`：宽松解析链接。
        case text
    }

    nonisolated struct File: Sendable, Hashable {
        let url: URL
        let kind: Kind
    }

    /// 暂存内容上限：full 为种子上限（TXT 读取自身还有更小的上限）。
    #if FULL_FLAVOR
    private static let stageLimit = TorrentFile.maxBytes + 1
    #else
    private static let stageLimit = 2 * 1024 * 1024 + 1
    #endif

    static func kind(ofFileNamed name: String) -> Kind? {
        let ext = (name as NSString).pathExtension.lowercased()
        #if FULL_FLAVOR
        if ext == "torrent" { return .torrent }
        #endif
        return ["txt", "url", "list"].contains(ext) ? .text : nil
    }

    /// 加载全部提供者；不支持的类型计入 `rejected`，加载 / 暂存 I/O 失败计入 `failed`。
    @MainActor
    static func load(_ providers: [NSItemProvider]) async -> (files: [File], rejected: Int, failed: Int) {
        var files: [File] = []
        var rejected = 0
        var failed = 0
        for provider in providers {
            let staged: Staged = await withCheckedContinuation { continuation in
                _ = provider.loadFileRepresentation(forTypeIdentifier: UTType.data.identifier) { url, _ in
                    continuation.resume(returning: url.map(stage) ?? .failed)
                }
            }
            switch staged {
            case let .file(file): files.append(file)
            case .unsupported: rejected += 1
            case .failed: failed += 1
            }
        }
        return (files, rejected, failed)
    }

    /// 删除暂存的临时副本（每个文件独占一个目录）。
    static func discard(_ files: [File]) {
        for file in files {
            try? FileManager.default.removeItem(at: file.url.deletingLastPathComponent())
        }
    }

    private enum Staged {
        case file(File)
        case unsupported
        case failed
    }

    private static func stage(_ source: URL) -> Staged {
        guard let kind = kind(ofFileNamed: source.lastPathComponent) else { return .unsupported }
        let fm = FileManager.default
        let dir = fm.temporaryDirectory.appendingPathComponent("fluxdown-drop-" + UUID().uuidString, isDirectory: true)
        let destination = dir.appendingPathComponent(source.lastPathComponent)
        do {
            try fm.createDirectory(at: dir, withIntermediateDirectories: true)
            let handle = try FileHandle(forReadingFrom: source)
            defer { try? handle.close() }
            let data = try handle.read(upToCount: stageLimit) ?? Data()
            try data.write(to: destination)
            return .file(File(url: destination, kind: kind))
        } catch {
            try? fm.removeItem(at: dir)
            return .failed
        }
    }
}
