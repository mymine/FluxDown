import FluxDomain
import FluxUI
import Foundation
import Observation

/// 写回失败 → 反馈文案（`web/src/pages/webhooks/write.ts::writeErrorText` ↔ GPUI `SettingsErrorKind::i18n_key`；
/// 校验失败附带具体原因）。
enum WebhookErrorText {
    static func write(_ error: HostError) -> String {
        switch error.code {
        case .unavailable, .timeout:
            return L("localServiceDisconnected")
        case .conflict:
            return L("localServiceConflict")
        case .invalidArgument:
            return error.message.isEmpty ? L("localServiceInvalidArgument") : "\(L("localServiceInvalidArgument")): \(error.message)"
        default:
            return L("localServiceActionFailed")
        }
    }

    /// 测试 / 模拟失败：主机给出的细节优先，其次按码文案。
    static func detail(_ error: HostError) -> String {
        error.message.isEmpty ? ErrorText.describe(error) : error.message
    }
}

/// 「发送测试」的展示结果；行内测试归属发起的端点（`endpointId`）。
nonisolated struct WebhookTestReport: Equatable {
    var endpointId: String
    var success: Bool
    var text: String

    /// 测试回执 → 文案（GPUI `test_result_text`；行内与编辑器共用）。
    @MainActor
    static func make(endpointId: String, response: WebhookTestResponse) -> WebhookTestReport {
        if response.success {
            let status = response.statusCode == 0 ? L("mobileWebhookStatusOk") : String(response.statusCode)
            return WebhookTestReport(
                endpointId: endpointId,
                success: true,
                text: L("webhookTestOk", ["status": status, "ms": response.latencyMs])
            )
        }
        let error = response.error.isEmpty ? ExtensionsFormat.httpStatus(response.statusCode) : response.error
        return WebhookTestReport(endpointId: endpointId, success: false, text: L("webhookTestFail", ["error": error]))
    }

    @MainActor
    static func make(endpointId: String, error: HostError) -> WebhookTestReport {
        WebhookTestReport(endpointId: endpointId, success: false, text: L("webhookTestFail", ["error": WebhookErrorText.detail(error)]))
    }
}

/// S12 页面状态：端点写入（读-改-写 + 冲突重放）、测试、模拟、清空投递日志。
@MainActor
@Observable
final class WebhookModel {
    private(set) var testingId: String?
    private(set) var testReport: WebhookTestReport?
    private(set) var simulating = false
    private(set) var simulateText: String?
    private(set) var clearing = false

    @ObservationIgnored private let container: AppContainer
    @ObservationIgnored private let writer: WebhookEndpointWriter

    init(container: AppContainer) {
        self.container = container
        writer = WebhookEndpointWriter(port: ContainerDaemonConfigPort(container))
    }

    private var session: any HostSession { container.session }

    private func toast(_ text: String, _ tone: ToastTone) {
        container.toasts.show(text: text, tone: tone)
    }

    // MARK: 端点写入

    /// 保存（新增或按 id 覆盖）；失败 toast 并返回 false。
    func save(_ draft: WebhookEndpoint) async -> Bool {
        guard await write({ (writer) throws(HostError) in try await writer.upsert(draft) }) != nil else { return false }
        dropTestReport(draft.id)
        return true
    }

    func setEnabled(_ endpoint: WebhookEndpoint, _ enabled: Bool) async {
        _ = await write { (writer) throws(HostError) in try await writer.setEnabled(id: endpoint.id, enabled: enabled) }
    }

    func remove(_ endpoint: WebhookEndpoint) async {
        dropTestReport(endpoint.id)
        _ = await write { (writer) throws(HostError) in try await writer.remove(id: endpoint.id) }
    }

    /// 执行一次写入；成功返回 `()`，失败 toast 后返回 nil。
    private func write(_ operation: (WebhookEndpointWriter) async throws(HostError) -> Void) async -> Void? {
        do throws(HostError) {
            try await operation(writer)
            return ()
        } catch {
            toast(WebhookErrorText.write(error), .error)
            return nil
        }
    }

    // MARK: 测试

    /// 把端点（或编辑器草稿）直接交给 `daemon.webhook.test`，无需先保存。
    func test(_ endpoint: WebhookEndpoint) async {
        guard testingId == nil else { return }
        testingId = endpoint.id
        testReport = nil
        do throws(HostError) {
            let response: WebhookTestResponse = try await session.call(HostMethod.daemonWebhookTest, params: endpoint)
            testReport = .make(endpointId: endpoint.id, response: response)
        } catch {
            testReport = .make(endpointId: endpoint.id, error: error)
        }
        testingId = nil
    }

    func dropTestReport(_ endpointId: String) {
        if testReport?.endpointId == endpointId { testReport = nil }
    }

    // MARK: 投递日志

    func simulate() async {
        guard !simulating else { return }
        simulating = true
        simulateText = L("webhookLogPending")
        do throws(HostError) {
            let response: WebhookSimulateResponse = try await session.call(HostMethod.daemonWebhookSimulate)
            simulateText = response.dispatched == 0
                ? L("webhookSimulateNoTarget")
                : L("webhookSimulateDispatched", ["n": response.dispatched])
        } catch {
            simulateText = L("webhookTestFail", ["error": WebhookErrorText.detail(error)])
        }
        simulating = false
    }

    func clearDeliveries() async {
        guard !clearing else { return }
        clearing = true
        do throws(HostError) {
            try await session.callVoid(HostMethod.daemonWebhookClearDeliveries)
        } catch {
            toast(WebhookErrorText.write(error), .error)
        }
        clearing = false
    }

    /// 预设目录 + 模板变量清单（`daemon.webhook.get`；前端不复制模板内容）。
    func loadCatalog() async -> WebhookDeliveriesResponse? {
        do throws(HostError) {
            return try await session.call(HostMethod.daemonWebhookGet)
        } catch {
            toast(L("mobileWebhookCatalogFailed", ["message": WebhookErrorText.detail(error)]), .warning)
            return nil
        }
    }
}
