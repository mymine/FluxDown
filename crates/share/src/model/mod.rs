//! FluxMesh界面的纯模型：不依赖 GPUI，全部可单测。

pub mod cloud;
pub mod diagnostics;
pub mod exit;
pub mod format;
pub mod logs;
pub mod panel;
pub mod pickup;
pub mod receive;
pub mod refresh;
pub mod settings_form;
pub mod status;
pub mod target;
pub mod usage;

pub use format::{
    CLAIM_PRESETS, CountLabel, claimed_summary, claims_label, format_bytes, format_datetime,
    format_percent, format_rate, parse_unix_secs, tier_label,
};
pub use pickup::{
    DEFAULT_PICKUP_CODE_LEN, ReceiveInput, format_pickup_code, normalize_pickup_code,
    parse_receive_input,
};
pub use status::{
    AppealUi, NetHealth, NetSegment, ShareAction, appeal_state, batch_actions, home_relay_latency,
    is_editable, nat_key, net_health, net_segments, ordered_shares, path_kind_key,
    provide_state_key, share_headline, share_status_key, source_missing_badge,
};
pub use target::{PanelTarget, panel_key, tasks_key};
