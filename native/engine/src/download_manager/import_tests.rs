use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    ActiveTaskEntry, BtConfig, Db, DownloadManager, DownloadManagerConfig, EngineEvent, EventSink,
    MAIN_QUEUE_ID, NewTaskSpec, ProxyConfig,
};

#[derive(Default)]
struct ImportSink(Mutex<Vec<EngineEvent>>);

impl EventSink for ImportSink {
    fn emit(&self, event: EngineEvent) {
        self.0.lock().expect("event sink lock").push(event);
    }
}

impl ImportSink {
    fn take(&self) -> Vec<EngineEvent> {
        std::mem::take(&mut *self.0.lock().expect("event sink lock"))
    }
}

async fn fixture() -> (Db, DownloadManager, Arc<ImportSink>) {
    let db = Db::connect("sqlite::memory:")
        .await
        .expect("memory database");
    db.seed_builtin_queues().await.expect("builtin queues");
    let sink = Arc::new(ImportSink::default());
    let manager = DownloadManager::new(
        db.clone(),
        DownloadManagerConfig {
            max_concurrent: 5,
            speed_limit_bps: 0,
            upload_limit_bps: 0,
            default_save_dir: std::env::temp_dir().to_string_lossy().into_owned(),
            app_data_dir: String::new(),
            data_dir: std::env::temp_dir(),
            bt_config: BtConfig::default(),
            proxy_config: ProxyConfig::default(),
            user_agent: String::new(),
        },
        sink.clone(),
        Arc::new(crate::NoopSelection),
    )
    .expect("download manager");
    (db, manager, sink)
}

fn occupy_five_slots(manager: &mut DownloadManager) {
    for slot in 0..5 {
        manager.active_tasks.insert(
            format!("occupied-{slot}"),
            ActiveTaskEntry {
                token: CancellationToken::new(),
                generation: 0,
                handle: None,
                is_bt: false,
                queue_id: MAIN_QUEUE_ID.into(),
            },
        );
    }
}

fn spec(index: usize) -> NewTaskSpec {
    NewTaskSpec {
        url: format!("http://127.0.0.1:1/import-{index}.bin"),
        save_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        file_name: format!("import-{index}.bin"),
        // The creation tests must neither spawn a metadata probe nor perform I/O.
        hint_file_size: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn paused_creation_fact_matches_every_persisted_field() {
    let (db, mut manager, sink) = fixture().await;
    let id = manager
        .create_task(NewTaskSpec {
            segments: 3,
            checksum: format!("sha256={}", "0".repeat(64)),
            ignore_tls_errors: true,
            proxy_url: "http://127.0.0.1:8123".into(),
            referrer: "https://example.test/source".into(),
            start_paused: true,
            ..spec(0)
        })
        .await
        .expect("create paused task");
    let persisted = db
        .load_task_by_id(&id)
        .await
        .expect("read task")
        .expect("task");
    let events = sink.take();
    let EngineEvent::TaskChanged(created) = events.first().expect("creation event") else {
        panic!("complete task fact must precede any progress or metadata");
    };
    assert_eq!(created.as_ref(), &persisted);
    assert_eq!(created.status, 2);
    assert_eq!(created.queue_order, 1);
    assert_eq!(created.queue_id, MAIN_QUEUE_ID);
    assert_eq!(created.segments, 3);
    assert!(created.ignore_tls_errors);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EngineEvent::TasksSnapshot(_)))
    );
    assert!(manager.active_tasks.is_empty());
    assert!(manager.pending_queue.is_empty());
}

#[tokio::test]
async fn rejected_resolver_creation_never_publishes_pending_fact() {
    let (_db, mut manager, sink) = fixture().await;
    let id = manager
        .create_task(NewTaskSpec {
            resolver_item: "unavailable-selection".into(),
            ..spec(0)
        })
        .await
        .expect("persist failed task");
    let events = sink.take();
    let EngineEvent::TaskChanged(created) = events.first().expect("creation event") else {
        panic!("missing complete creation fact");
    };
    assert_eq!(created.task_id, id);
    assert_eq!(created.status, 4);
    assert!(events.iter().all(|event| match event {
        EngineEvent::TaskChanged(task) => task.status == 4,
        EngineEvent::TaskProgress { status, .. } => *status == 4,
        EngineEvent::TasksSnapshot(_) => false,
        _ => true,
    }));
    assert!(manager.active_tasks.is_empty());
}

#[tokio::test]
async fn queued_import_coalesces_positions_and_pause_stops_every_queued_task() {
    let (db, mut manager, sink) = fixture().await;
    occupy_five_slots(&mut manager);
    let mut ids = Vec::new();
    for index in 0..128 {
        ids.push(
            manager
                .create_task(spec(index))
                .await
                .expect("create queued task"),
        );
    }
    let events = sink.take();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, EngineEvent::TaskChanged(_)))
            .count(),
        128
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        EngineEvent::TasksSnapshot(_) | EngineEvent::QueuePositionsChanged(_)
    )));
    assert_eq!(manager.active_tasks.len(), 5);
    assert_eq!(manager.max_concurrent, 5);
    assert_eq!(manager.pending_queue.len(), 128);
    let queue_storage = manager.pending_queue.as_slices().0.as_ptr();
    manager.drain_queue().await;
    assert_eq!(
        manager.pending_queue.as_slices().0.as_ptr(),
        queue_storage,
        "a full scheduler must not rebuild the waiting queue"
    );
    assert!(sink.take().is_empty());

    manager.flush_queue_positions();
    let positions = sink.take();
    let [EngineEvent::QueuePositionsChanged(positions)] = positions.as_slice() else {
        panic!("one coalesced complete position snapshot expected");
    };
    assert_eq!(positions.len(), ids.len());
    for (index, position) in positions.iter().enumerate() {
        assert_eq!(position.task_id, ids[index]);
        assert_eq!(position.position, (index + 1) as i32);
    }
    manager.flush_queue_positions();
    assert!(
        sink.take().is_empty(),
        "unchanged positions must not be rebroadcast"
    );

    manager.batch_pause_all(&ids).await;
    assert!(manager.pending_queue.is_empty());
    assert!(
        db.load_all_tasks()
            .await
            .expect("paused rows")
            .iter()
            .all(|task| task.status == 2)
    );
    assert!(
        sink.take().iter().any(|event| matches!(event,
            EngineEvent::QueuePositionsChanged(positions) if positions.is_empty()
        )),
        "pause must immediately clear stale positions"
    );
    manager.flush_queue_positions();
    assert!(sink.take().is_empty());
}

#[tokio::test]
async fn queue_drain_refills_all_five_configured_slots_not_one() {
    let (_db, mut manager, _sink) = fixture().await;
    let directory = std::env::temp_dir().join(format!("fluxdown-import-slots-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&directory).expect("isolated download directory");
    occupy_five_slots(&mut manager);
    let mut ids = Vec::new();
    for index in 0..8 {
        ids.push(
            manager
                .create_task(NewTaskSpec {
                    save_dir: directory.to_string_lossy().into_owned(),
                    ..spec(index)
                })
                .await
                .expect("create queued task"),
        );
    }
    manager.active_tasks.clear();
    manager.drain_queue().await;
    assert_eq!(manager.max_concurrent, 5);
    assert_eq!(manager.active_tasks.len(), 5);
    assert_eq!(manager.pending_queue.len(), 3);
    assert!(
        ids[..5]
            .iter()
            .all(|id| manager.active_tasks.contains_key(id))
    );
    assert_eq!(
        manager.pending_queue.front().expect("pending head").task_id,
        ids[5]
    );
    // The spawned workers are real downloader tasks; join cancellation before cleanup.
    manager.shutdown().await;
    std::fs::remove_dir_all(directory).expect("remove isolated download directory");
}
