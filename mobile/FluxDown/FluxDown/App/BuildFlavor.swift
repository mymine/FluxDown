import FluxDomain
import Foundation

/// 构建 flavor（`mobile/FluxDown/scripts/build-core.sh --flavor`）。
///
/// - full（侧载 / 开发默认）：编译条件 `FULL_P2P` 开启，本机引擎含完整 P2P。
/// - store（App Store / TestFlight）：不定义 `FULL_P2P`（工程设置 `FLUX_FLAVOR_CONDITIONS` 置空），
///   本机引擎编译期剔除 P2P，界面上不出现 P2P 品牌化入口；远端主机上的任务与控制照常。
nonisolated enum BuildFlavor {
    /// 按 flavor 选文案键：store 用不含 P2P 字样的通用键。
    static func key(full: String, store: String) -> String {
        #if FULL_P2P
        return full
        #else
        return store
        #endif
    }
}

/// 需要本机 P2P 引擎才能处理的链接（magnet / ed2k / `torrent-file://` 哨兵）。
nonisolated enum P2PLink {
    static func isP2P(_ url: String) -> Bool {
        let proto = protocolOf(url)
        return proto == .bt || proto == .ed2k
    }
}
