import FluxDomain
import Foundation

#if FULL_FLAVOR
// X1 的文件树模型（纯逻辑，同 Android `BtFileSelection.kt` 的 `buildBtTree` / `flatten` / `folderState`）。

nonisolated final class BtFolderNode {
    let name: String
    /// `a/b` 形式的完整路径（根为空串）；同时作为折叠状态的 key。
    let path: String
    let depth: Int
    var children: [BtNode] = []
    /// 子树内全部文件下标。
    var indices: [Int32] = []
    /// 子树内文件总大小。
    var size: Int64 = 0
    fileprivate var folders: [String: BtFolderNode] = [:]

    init(name: String, path: String, depth: Int) {
        self.name = name
        self.path = path
        self.depth = depth
    }
}

nonisolated enum BtNode {
    case folder(BtFolderNode)
    case file(name: String, depth: Int, file: BtFile)

    var id: String {
        switch self {
        case let .folder(node): "d:" + node.path
        case let .file(_, _, file): "f:\(file.index)"
        }
    }

    var depth: Int {
        switch self {
        case let .folder(node): node.depth
        case let .file(_, depth, _): depth
        }
    }
}

/// 路径 `a/b/c.txt` 建树；目录在前（按名称，忽略大小写），文件在后（保持清单顺序）；每个目录累计子树文件下标与大小。
nonisolated func buildBtTree(_ files: [BtFile]) -> BtFolderNode {
    let root = BtFolderNode(name: "", path: "", depth: -1)
    for file in files {
        var parts = file.path.split(separator: "/").map(String.init)
        if parts.isEmpty { parts = [file.path] }
        var current = root
        current.indices.append(file.index)
        current.size += file.size
        for segment in parts.dropLast() {
            let child: BtFolderNode
            if let existing = current.folders[segment] {
                child = existing
            } else {
                child = BtFolderNode(
                    name: segment,
                    path: current.path.isEmpty ? segment : current.path + "/" + segment,
                    depth: current.depth + 1
                )
                current.folders[segment] = child
                current.children.append(.folder(child))
            }
            child.indices.append(file.index)
            child.size += file.size
            current = child
        }
        current.children.append(.file(name: parts[parts.count - 1], depth: current.depth + 1, file: file))
    }
    sortChildren(root)
    return root
}

private nonisolated func sortChildren(_ folder: BtFolderNode) {
    var dirs: [BtFolderNode] = []
    var files: [BtNode] = []
    for child in folder.children {
        switch child {
        case let .folder(node): dirs.append(node)
        case .file: files.append(child)
        }
    }
    dirs.sort { $0.name.lowercased() < $1.name.lowercased() }
    folder.children = dirs.map { BtNode.folder($0) } + files
    dirs.forEach(sortChildren)
}

/// 按折叠集合展开成可见行（先序）。
nonisolated func flattenBtTree(_ root: BtFolderNode, collapsed: Set<String>) -> [BtNode] {
    var out: [BtNode] = []
    func walk(_ folder: BtFolderNode) {
        for child in folder.children {
            out.append(child)
            if case let .folder(node) = child, !collapsed.contains(node.path) { walk(node) }
        }
    }
    walk(root)
    return out
}

/// 清单超过 60 项：默认只展开第 1 层（深度 ≥ 1 的目录折叠）。
nonisolated func defaultCollapsedFolders(_ root: BtFolderNode, fileCount: Int) -> Set<String> {
    guard fileCount > 60 else { return [] }
    var set = Set<String>()
    func walk(_ folder: BtFolderNode) {
        for child in folder.children {
            guard case let .folder(node) = child else { continue }
            if node.depth >= 1 { set.insert(node.path) }
            walk(node)
        }
    }
    walk(root)
    return set
}

nonisolated enum BtCheckState: Sendable, Hashable { case off, on, mixed }

nonisolated func btFolderState(_ node: BtFolderNode, selected: Set<Int32>) -> BtCheckState {
    var n = 0
    for i in node.indices where selected.contains(i) { n += 1 }
    if n == 0 { return .off }
    return n == node.indices.count ? .on : .mixed
}

/// 初始选择：默认选择里仍存在的下标；为空则全选。
nonisolated func initialBtSelection(files: [BtFile], defaultIndices: [Int32]) -> Set<Int32> {
    let known = Set(files.map(\.index))
    let preset = Set(defaultIndices).intersection(known)
    return preset.isEmpty ? known : preset
}
#endif
