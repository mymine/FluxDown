//! `fluxdown_share` 类型 → `fluxdown_protocol` 的 UI DTO（两边 serde 形态相同，逐字段显式转换）。

use fluxdown_protocol as proto;
use fluxdown_protocol::{
    ShareHomeRelayDto, ShareNatType, ShareNetReportDto, ShareNetSnapshotDto, SharePathEventDto,
    SharePathKind, SharePathSummaryDto, ShareSessionBytesDto, ShareSessionSampleDto,
    ShareSessionSummaryDto,
};
use fluxdown_share as core;

pub(super) fn path_kind(kind: core::PathKind) -> SharePathKind {
    match kind {
        core::PathKind::DirectLan => SharePathKind::DirectLan,
        core::PathKind::DirectWan => SharePathKind::DirectWan,
        core::PathKind::Relay => SharePathKind::Relay,
        core::PathKind::Unknown => SharePathKind::Unknown,
    }
}

pub(super) fn nat_type(nat: core::NatType) -> ShareNatType {
    match nat {
        core::NatType::Cone => ShareNatType::Cone,
        core::NatType::Symmetric => ShareNatType::Symmetric,
        core::NatType::Unknown => ShareNatType::Unknown,
    }
}

pub(super) fn connection_mode(mode: proto::ShareConnectionMode) -> core::ConnectionMode {
    match mode {
        proto::ShareConnectionMode::Auto => core::ConnectionMode::Auto,
        proto::ShareConnectionMode::RelayOnly => core::ConnectionMode::RelayOnly,
        proto::ShareConnectionMode::DirectOnly => core::ConnectionMode::DirectOnly,
    }
}

/// 本机设置 → 端点设置。
pub(super) fn local_endpoint_config(
    local: &proto::ShareLocalConfigDto,
) -> core::LocalEndpointConfig {
    core::LocalEndpointConfig {
        udp_port_v4: local.udp_port_v4,
        udp_port_v6: local.udp_port_v6,
        portmapper: local.portmapper,
        extra_addrs: local.extra_addrs.clone(),
        mode: connection_mode(local.mode),
        pinned_relay: local.pinned_relay.clone(),
        send_rate_limit_bytes_per_sec: local.send_rate_limit_bytes_per_sec,
    }
}

pub(super) fn net_report(report: &core::NetReportInfo) -> ShareNetReportDto {
    ShareNetReportDto {
        udp_v4: report.udp_v4,
        udp_v6: report.udp_v6,
        mapping_varies_by_dest: report.mapping_varies_by_dest,
        nat_type: nat_type(report.nat_type),
        preferred_relay: report.preferred_relay.clone(),
        relay_latency_ms: report.relay_latency_ms.clone(),
        global_v4: report.global_v4.clone(),
        global_v6: report.global_v6.clone(),
        captive_portal: report.captive_portal,
    }
}

pub(super) fn snapshot(snapshot: &core::NetSnapshot) -> ShareNetSnapshotDto {
    ShareNetSnapshotDto {
        captured_at: snapshot.captured_at.clone(),
        endpoint_id: snapshot.endpoint_id.clone(),
        client_version: snapshot.client_version.clone(),
        platform: snapshot.platform.clone(),
        net_report: snapshot.net_report.as_ref().map(net_report),
        home_relay: snapshot.home_relay.as_ref().map(|relay| ShareHomeRelayDto {
            url: relay.url.clone(),
            connected: relay.connected,
        }),
        addrs: snapshot.addrs.clone(),
        metrics: snapshot.metrics.clone(),
    }
}

pub(super) fn summary(summary: &core::SessionSummary) -> ShareSessionSummaryDto {
    ShareSessionSummaryDto {
        started_at: summary.started_at.clone(),
        connect_ms: summary.connect_ms,
        first_direct_ms: summary.first_direct_ms,
        path_kind: path_kind(summary.path_kind),
        bytes: ShareSessionBytesDto {
            direct: summary.bytes.direct,
            relay: summary.bytes.relay,
            cdn: summary.bytes.cdn,
            bridge: summary.bytes.bridge,
            peer: summary.bytes.peer,
        },
        paths: summary
            .paths
            .iter()
            .map(|path| SharePathSummaryDto {
                id: path.id.clone(),
                kind: path_kind(path.kind),
                remote: path.remote.clone(),
                rtt_ms: path.rtt_ms,
                cwnd: path.cwnd,
                mtu: path.mtu,
                sent: path.sent,
                recv: path.recv,
                lost: path.lost,
                congestion_events: path.congestion_events,
                bw_estimate: path.bw_estimate,
            })
            .collect(),
        events: summary
            .events
            .iter()
            .map(|event| SharePathEventDto {
                at: event.at.clone(),
                kind: event.kind.clone(),
                path: event.path.clone(),
            })
            .collect(),
        verify_failures: summary.verify_failures,
        error: summary.error.clone(),
    }
}

pub(super) fn sample(sample: &core::SessionSample) -> ShareSessionSampleDto {
    ShareSessionSampleDto {
        at: sample.at.clone(),
        bytes: sample.bytes,
        rate_bps: sample.rate_bps,
        rtt_ms: sample.rtt_ms,
        path_kind: path_kind(sample.path_kind),
    }
}

pub(super) fn swarm_file(file: &core::SwarmServedFile) -> proto::ShareSwarmServedFileDto {
    proto::ShareSwarmServedFileDto {
        root_hash: file.root_hash.clone(),
        file_name: None,
        bytes: file.bytes,
        chunks: file.chunks,
    }
}

pub(super) fn role(role: core::SessionRole) -> proto::ShareSessionRole {
    match role {
        core::SessionRole::Sender => proto::ShareSessionRole::Sender,
        core::SessionRole::Receiver => proto::ShareSessionRole::Receiver,
        core::SessionRole::Swarm => proto::ShareSessionRole::Swarm,
    }
}
