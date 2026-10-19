import Accessibility
import Charts
import SwiftUI

/// 速度面积图（§9.11）：Swift Charts `AreaMark`（强调色渐变 .42 → 0）+ `LineMark`（2pt，Catmull-Rom 平滑）。
///
/// - `down`：下载序列；`up`：可选上传序列（`fdStatusSeeding` 色 1.6pt 线）。均为 1Hz 采样，从旧到新，
///   X 轴语义为「N 秒前」，不画 X 轴。
/// - 纵轴 3 条虚线网格（0.5pt `separator`），右侧轴标 10pt `.tertiary`（点缀，不承载信息；数值由 `valueFormat` 格式化）。
/// - 无障碍：`AXChartDescriptor`（可用音频图读出两条序列）；视觉图表本身不重复朗读。
/// - 文案由调用方本地化；默认值为英文兜底。
public struct SpeedAreaChart: View {
    public let down: [Double]
    public let up: [Double]?
    public let valueFormat: (Double) -> String
    public let title: String
    public let downLabel: String
    public let upLabel: String
    public let timeLabel: String
    /// 无障碍 X 值朗读（入参为点序号）；nil = 默认「N 秒前」（实时 1Hz 序列）。
    public let xValueLabel: ((Double) -> String)?
    /// 触摸读数的时间文案（入参为点序号）；nil = 不启用触摸读数。
    public let readoutTime: ((Int) -> String)?

    public init(
        down: [Double],
        up: [Double]? = nil,
        valueFormat: @escaping (Double) -> String = { $0.formatted(.number.notation(.compactName)) },
        title: String = "Speed",
        downLabel: String = "Download",
        upLabel: String = "Upload",
        timeLabel: String = "Seconds ago",
        xValueLabel: ((Double) -> String)? = nil,
        readoutTime: ((Int) -> String)? = nil
    ) {
        self.down = down
        self.up = up
        self.valueFormat = valueFormat
        self.title = title
        self.downLabel = downLabel
        self.upLabel = upLabel
        self.timeLabel = timeLabel
        self.xValueLabel = xValueLabel
        self.readoutTime = readoutTime
    }

    private nonisolated struct Point: Identifiable {
        let id: Int
        let value: Double
    }

    private func points(_ values: [Double]) -> [Point] {
        values.enumerated().map { Point(id: $0.offset, value: $0.element) }
    }

    private var count: Int { max(down.count, up?.count ?? 0) }

    @State private var selectedX: Double?

    private var selectedIndex: Int? {
        guard readoutTime != nil, let x = selectedX, !down.isEmpty else { return nil }
        return min(max(Int(x.rounded()), 0), down.count - 1)
    }

    public var body: some View {
        let downPoints = points(down)
        let upPoints = up.map(points)
        Chart {
            ForEach(downPoints) { p in
                AreaMark(x: .value("t", p.id), y: .value(downLabel, p.value), series: .value("series", "down"))
                    .foregroundStyle(
                        LinearGradient(colors: [Color.accentColor.opacity(0.42), Color.accentColor.opacity(0)], startPoint: .top, endPoint: .bottom)
                    )
                    .interpolationMethod(.catmullRom)
                LineMark(x: .value("t", p.id), y: .value(downLabel, p.value), series: .value("series", "down"))
                    .foregroundStyle(Color.accentColor)
                    .lineStyle(StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                    .interpolationMethod(.catmullRom)
            }
            if let upPoints {
                ForEach(upPoints) { p in
                    LineMark(x: .value("t", p.id), y: .value(upLabel, p.value), series: .value("series", "up"))
                        .foregroundStyle(Color.fdStatusSeeding)
                        .lineStyle(StrokeStyle(lineWidth: 1.6, lineCap: .round, lineJoin: .round))
                        .interpolationMethod(.catmullRom)
                }
            }
            if let index = selectedIndex, let time = readoutTime?(index) {
                RuleMark(x: .value("t", index))
                    .foregroundStyle(Color(uiColor: .separator))
                    .lineStyle(StrokeStyle(lineWidth: 1))
                    .annotation(
                        position: .top, spacing: 2,
                        overflowResolution: .init(x: .fit(to: .chart), y: .disabled)
                    ) {
                        VStack(spacing: 1) {
                            Text(time).foregroundStyle(.secondary)
                            Text(valueFormat(down[index])).foregroundStyle(.primary)
                        }
                        .font(.caption2)
                        .monospacedDigit()
                        .padding(.horizontal, 6)
                        .padding(.vertical, 3)
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6))
                    }
                PointMark(x: .value("t", index), y: .value(downLabel, down[index]))
                    .foregroundStyle(Color.accentColor)
                    .symbolSize(40)
            }
        }
        .chartXSelection(value: $selectedX)
        .chartPlotStyle { plot in
            plot.padding(.top, readoutTime != nil ? 32 : 0)
        }
        .chartXScale(domain: 0...max(count - 1, 1))
        .chartXAxis(.hidden)
        .chartYScale(domain: .automatic(includesZero: true))
        .chartYAxis {
            AxisMarks(position: .trailing, values: .automatic(desiredCount: 3)) { value in
                AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5, dash: [3, 4]))
                    .foregroundStyle(Color(uiColor: .separator))
                AxisValueLabel {
                    if let v = value.as(Double.self) {
                        Text(valueFormat(v))
                            .font(.system(size: 10))
                            .monospacedDigit()
                            .foregroundStyle(.tertiary)
                    }
                }
            }
        }
        .chartLegend(.hidden)
        .accessibilityChartDescriptor(
            SpeedChartDescriptor(
                down: down, up: up, valueFormat: valueFormat,
                title: title, downLabel: downLabel, upLabel: upLabel, timeLabel: timeLabel, xValueLabel: xValueLabel
            )
        )
    }
}

private struct SpeedChartDescriptor: AXChartDescriptorRepresentable {
    let down: [Double]
    let up: [Double]?
    let valueFormat: (Double) -> String
    let title: String
    let downLabel: String
    let upLabel: String
    let timeLabel: String
    let xValueLabel: ((Double) -> String)?

    func makeChartDescriptor() -> AXChartDescriptor {
        let count = max(down.count, up?.count ?? 0)
        let peak = max(down.max() ?? 0, up?.max() ?? 0)
        let xAxis = AXNumericDataAxisDescriptor(
            title: timeLabel,
            range: 0...Double(max(count - 1, 1)),
            gridlinePositions: []
        ) { index in
            xValueLabel?(index) ?? "\(Int(Double(max(count - 1, 0)) - index))"
        }
        let format = valueFormat
        let yAxis = AXNumericDataAxisDescriptor(
            title: title,
            range: 0...max(peak, 1),
            gridlinePositions: []
        ) { value in format(value) }

        func series(_ name: String, _ values: [Double]) -> AXDataSeriesDescriptor {
            AXDataSeriesDescriptor(
                name: name,
                isContinuous: true,
                dataPoints: values.enumerated().map { AXDataPoint(x: Double($0.offset), y: $0.element) }
            )
        }
        var all = [series(downLabel, down)]
        if let up { all.append(series(upLabel, up)) }
        return AXChartDescriptor(title: title, summary: nil, xAxis: xAxis, yAxis: yAxis, additionalAxes: [], series: all)
    }
}

#Preview("SpeedAreaChart") {
    let down = (0..<60).map { i in 18 + 10 * sin(Double(i) / 5) + Double((i * 7) % 5) }
    let up = (0..<60).map { i in 3 + 2 * sin(Double(i) / 7) }
    VStack(alignment: .leading, spacing: 20) {
        SpeedAreaChart(down: down, up: up).frame(height: 160)
        SpeedAreaChart(down: down).frame(height: 100)
    }
    .padding()
}

#Preview("SpeedAreaChart · 深色") {
    SpeedAreaChart(down: [0, 4, 9, 7, 14, 12, 18, 16], up: [0, 1, 1, 2, 2, 3, 2, 3])
        .frame(height: 140)
        .padding()
        .preferredColorScheme(.dark)
}
