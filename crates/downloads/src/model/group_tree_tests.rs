use serde::Deserialize;
use serde_json::json;

use super::{
    GroupAggregate, TreeMember, TreeRow, compress_path, display_name, flatten, member_dir,
    sparkline_height, sparkline_sample,
};
use crate::model::{DownloadTaskView, TaskState};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    member_dir: Vec<MemberDirCase>,
    display_name: Vec<DisplayNameCase>,
    compress_path: Vec<CompressCase>,
    aggregate: Vec<AggregateCase>,
    flatten: Vec<FlattenCase>,
    sparkline_sample: Vec<SampleCase>,
    sparkline_height: Vec<HeightCase>,
}

#[derive(Deserialize)]
struct SampleCase {
    len: usize,
    max: usize,
    expect: Vec<usize>,
}

#[derive(Deserialize)]
struct HeightCase {
    progress: f32,
    status: i32,
    expect: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemberDirCase {
    group_dir: String,
    task_dir: String,
    expect: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisplayNameCase {
    name: String,
    save_dir: String,
    id: String,
    expect: String,
}

#[derive(Deserialize)]
struct CompressCase {
    path: String,
    expect: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Member {
    id: String,
    name: String,
    #[serde(default)]
    status: i32,
    total_bytes: i64,
    #[serde(default)]
    downloaded_bytes: i64,
    #[serde(default)]
    speed: i64,
    #[serde(default)]
    save_dir: String,
}

impl Member {
    fn view(&self) -> DownloadTaskView {
        let dto: fluxdown_protocol::TaskDto = serde_json::from_value(json!({
            "taskId": self.id, "url": "https://example.com/file", "fileName": self.name,
            "saveDir": self.save_dir, "status": self.status,
            "downloadedBytes": self.downloaded_bytes, "totalBytes": self.total_bytes,
            "errorMessage": "", "createdAt": "1", "proxyUrl": "", "queueId": "main", "checksum": ""
        }))
        .expect("task dto");
        DownloadTaskView::local(&dto, Some(self.speed), false)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AggregateCase {
    title: String,
    members: Vec<Member>,
    expect: AggregateExpect,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct AggregateExpect {
    state: String,
    total: usize,
    done: usize,
    downloading: usize,
    pending: usize,
    paused: usize,
    failed: usize,
    size_bytes: u64,
    downloaded_bytes: u64,
    speed: u64,
    progress: f32,
    eta_seconds: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlattenCase {
    title: String,
    group_dir: String,
    collapsed_dirs: Vec<String>,
    members: Vec<Member>,
    expect: Vec<serde_json::Value>,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("../../tests/fixtures/group_tree_cases.json"))
        .expect("group tree fixture")
}

fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Pending => "pending",
        TaskState::Downloading => "downloading",
        TaskState::Paused => "paused",
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
    }
}

#[test]
fn sparkline_samples_evenly_and_keeps_bars_visible() {
    let fixture = fixture();
    for case in fixture.sparkline_sample {
        assert_eq!(sparkline_sample(case.len, case.max), case.expect);
    }
    for case in fixture.sparkline_height {
        let height = sparkline_height(case.progress, TaskState::from_status(case.status));
        assert!((height - case.expect).abs() < 1e-3, "{}", case.progress);
    }
}

#[test]
fn member_dir_is_relative_to_group_root_or_root_when_outside() {
    for case in fixture().member_dir {
        assert_eq!(
            member_dir(&case.group_dir, &case.task_dir),
            case.expect,
            "{} / {}",
            case.group_dir,
            case.task_dir
        );
    }
}

#[test]
fn display_name_falls_back_to_dir_then_id() {
    for case in fixture().display_name {
        assert_eq!(
            display_name(&case.name, &case.save_dir, &case.id),
            case.expect
        );
    }
}

#[test]
fn compress_path_keeps_first_and_last_of_deep_paths() {
    for case in fixture().compress_path {
        assert_eq!(compress_path(&case.path), case.expect, "{}", case.path);
    }
}

#[test]
fn aggregate_counts_state_bytes_speed_and_eta() {
    for case in fixture().aggregate {
        let views: Vec<DownloadTaskView> = case.members.iter().map(Member::view).collect();
        let aggregate = GroupAggregate::of(&views);
        let actual = AggregateExpect {
            state: state_name(aggregate.state()).to_owned(),
            total: aggregate.counts.total,
            done: aggregate.counts.done,
            downloading: aggregate.counts.downloading,
            pending: aggregate.counts.pending,
            paused: aggregate.counts.paused,
            failed: aggregate.counts.failed,
            size_bytes: aggregate.size_bytes,
            downloaded_bytes: aggregate.downloaded_bytes,
            speed: aggregate.speed,
            progress: aggregate.progress,
            eta_seconds: aggregate.eta_seconds,
        };
        assert_eq!(actual, case.expect, "{}", case.title);
    }
}

#[test]
fn flatten_lists_root_then_each_directory_once() {
    for case in fixture().flatten {
        let members: Vec<TreeMember> = case
            .members
            .iter()
            .map(|member| TreeMember {
                dir: member_dir(&case.group_dir, &member.save_dir),
                name_fold: member.name.to_lowercase(),
                size_bytes: member.total_bytes.max(0) as u64,
            })
            .collect();
        let rows = flatten(&members, |dir| {
            case.collapsed_dirs.iter().any(|item| item == dir)
        });
        let actual: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| match row {
                TreeRow::Member { index, nested } => {
                    json!({ "member": case.members[*index].id, "nested": nested })
                }
                TreeRow::Dir {
                    path,
                    count,
                    size_bytes,
                    collapsed,
                } => json!({
                    "dir": path, "count": count, "sizeBytes": size_bytes, "collapsed": collapsed
                }),
            })
            .collect();
        assert_eq!(actual, case.expect, "{}", case.title);
    }
}
