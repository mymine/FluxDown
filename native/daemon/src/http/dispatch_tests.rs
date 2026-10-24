#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use fluxdown_protocol::{
    DaemonEvent, DaemonSnapshot, EventFrame, FrameVerdict, ServiceEvent, WsServerMsg, judge_frame,
    method,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use crate::actor::{ActorOperation, ActorResult, DaemonActorHandle, DaemonCommand};

async fn write_request(socket: &mut TcpStream, id: u64, method: &str, params: Value) {
    let payload = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": id, "method": method, "params": params,
    }))
    .expect("request JSON");
    assert!(payload.len() <= u16::MAX as usize);
    let mask = [0x46, 0x4c, 0x55, 0x58];
    let mut frame = Vec::with_capacity(payload.len() + 8);
    frame.push(0x81);
    if payload.len() < 126 {
        frame.push(0x80 | payload.len() as u8);
    } else {
        frame.extend_from_slice(&[0x80 | 126]);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(i, byte)| byte ^ mask[i % 4]),
    );
    socket
        .write_all(&frame)
        .await
        .expect("write WebSocket request");
}

async fn read_message(socket: &mut TcpStream) -> Value {
    let mut header = [0; 2];
    socket
        .read_exact(&mut header)
        .await
        .expect("read WebSocket header");
    assert_eq!(
        header[0], 0x81,
        "expected a text response, not a close/event-gap frame"
    );
    assert_eq!(header[1] & 0x80, 0, "server frame must not be masked");
    let length = match header[1] & 0x7f {
        126 => socket.read_u16().await.expect("16-bit payload length") as usize,
        127 => usize::try_from(socket.read_u64().await.expect("64-bit payload length"))
            .expect("payload length fits memory"),
        length => length as usize,
    };
    assert!(length < 1024 * 1024, "bounded test response");
    let mut payload = vec![0; length];
    socket
        .read_exact(&mut payload)
        .await
        .expect("read WebSocket payload");
    serde_json::from_slice(&payload).expect("response JSON")
}

/// Use the actual authenticated HTTP/WebSocket consumer, with a controlled actor acknowledgement.
/// A pending ordinary create must not starve events, and the next pause must not overtake it.
#[tokio::test]
async fn ordinary_actor_wait_drains_events_without_reordering_commands() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let directory = std::env::temp_dir().join(format!("fluxdown-ws-dispatch-{}", uuid::Uuid::new_v4()));
        let db = fluxdown_engine::db::Db::connect("sqlite::memory:")
            .await.expect("memory database");
        let events = crate::event_hub::DaemonEventHub::new(DaemonSnapshot::default(), 16);
        let selections = crate::selection::DaemonSelection::new(events.clone());
        let blobs = Arc::new(crate::blob_store::BlobStore::open(directory.join("blobs"))
            .await.expect("blob directory"));
        let (actor, mut commands) = DaemonActorHandle::test_channel();
        let service = Arc::new(crate::service::DaemonService::new(
            crate::service_hello("ordered-dispatch-test", Vec::new()),
            events.clone(), selections, blobs, actor, db, directory.clone(),
            #[cfg(feature = "plugins")]
            None,
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("test listener");
        let address = listener.local_addr().expect("test address");
        let cancel = CancellationToken::new();
        let server = tokio::spawn(super::serve(listener, service, "test-token".into(), cancel.clone()));
        let mut socket = TcpStream::connect(address).await.expect("connect WebSocket");
        socket.write_all(b"GET /rpc HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-token\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n")
            .await.expect("WebSocket upgrade");
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(socket.read_u8().await.expect("upgrade response byte"));
            assert!(headers.len() < 4096);
        }
        assert!(headers.starts_with(b"HTTP/1.1 101"));
        write_request(&mut socket, 1, method::SYSTEM_HELLO, json!({
            "clientName": "fluxdown-agent", "clientVersion": "test",
            "minProtocolVersion": fluxdown_protocol::MIN_PROTOCOL_VERSION,
            "maxProtocolVersion": fluxdown_protocol::PROTOCOL_VERSION,
            "requestedRole": "daemon", "capabilities": [],
        })).await;
        assert_eq!(read_message(&mut socket).await["id"], 1);
        write_request(&mut socket, 2, method::DAEMON_TASK_CREATE, json!({
            "request": {"url": "https://example.test/a.bin", "startPaused": true},
        })).await;
        let DaemonCommand::Execute { operation, ack: create_ack } = commands.recv().await.expect("create command") else {
            panic!("expected create operation");
        };
        assert!(matches!(operation, ActorOperation::CreateTask { .. }));
        write_request(&mut socket, 3, method::DAEMON_TASK_PAUSE_ALL, Value::Null).await;

        // Publish > capacity while create remains pending. Each frame must reach the consumer
        // before the producer continues; neither a larger buffer nor resync can satisfy this.
        for _ in 0..64 {
            let emitted = events.publish(DaemonEvent::RuntimeStatsChanged(Default::default()));
            let received = read_message(&mut socket).await;
            assert_eq!(received["method"], method::SERVICE_EVENT);
            assert_eq!(received["params"]["sequence"], emitted.sequence);
            assert!(matches!(commands.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)),
                "pause must not overtake the pending create");
        }
        assert!(create_ack.send(Ok(ActorResult::Created("created-task".into()))).is_ok());
        let created = read_message(&mut socket).await;
        assert_eq!(created["id"], 2);
        assert!(created.get("error").is_none(), "{created}");
        let DaemonCommand::Execute { operation, ack: pause_ack } = commands.recv().await.expect("pause command") else {
            panic!("expected pause operation");
        };
        assert!(matches!(operation, ActorOperation::PauseAll));
        assert!(pause_ack.send(Ok(ActorResult::Unit)).is_ok());
        let paused = read_message(&mut socket).await;
        assert_eq!(paused["id"], 3);
        assert!(paused.get("error").is_none(), "{paused}");

        drop(socket);
        cancel.cancel();
        server.await.expect("join HTTP server").expect("HTTP server shutdown");
        std::fs::remove_dir_all(directory).expect("remove isolated test directory");
    }).await.expect("ordinary call must keep draining events while the actor is pending");
}

fn progress_event(task: usize, downloaded: i64) -> DaemonEvent {
    DaemonEvent::Engine(WsServerMsg::TaskProgress {
        task_id: format!("task-{task}"),
        status: 1,
        downloaded_bytes: downloaded,
        total_bytes: 1 << 40,
        speed: 1,
        file_name: String::new(),
        save_dir: String::new(),
        upload_speed: 0,
        url: String::new(),
        error_message: String::new(),
        uploaded_bytes: 0,
        seeding_status: 0,
        seeding_message: String::new(),
        seeding_time_secs: 0,
    })
}

/// 慢消费者（握手后一直不读套接字）在进度风暴下不会被 4009 断开：转发器及时接收广播并合并，
/// 恢复读取后收到的帧链按 `judge_frame` 无缺口地追到最后一帧。
#[tokio::test]
async fn stalled_consumer_under_progress_storm_is_coalesced_not_disconnected() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let directory = std::env::temp_dir().join(format!("fluxdown-ws-storm-{}", uuid::Uuid::new_v4()));
        let db = fluxdown_engine::db::Db::connect("sqlite::memory:")
            .await.expect("memory database");
        let events = crate::event_hub::DaemonEventHub::new(DaemonSnapshot::default(), 1024);
        let selections = crate::selection::DaemonSelection::new(events.clone());
        let blobs = Arc::new(crate::blob_store::BlobStore::open(directory.join("blobs"))
            .await.expect("blob directory"));
        let (actor, _commands) = DaemonActorHandle::test_channel();
        let service = Arc::new(crate::service::DaemonService::new(
            crate::service_hello("storm-test", Vec::new()),
            events.clone(), selections, blobs, actor, db, directory.clone(),
            #[cfg(feature = "plugins")]
            None,
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("test listener");
        let address = listener.local_addr().expect("test address");
        let cancel = CancellationToken::new();
        let server = tokio::spawn(super::serve(listener, service, "test-token".into(), cancel.clone()));
        let mut socket = TcpStream::connect(address).await.expect("connect WebSocket");
        socket.write_all(b"GET /rpc HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-token\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n")
            .await.expect("WebSocket upgrade");
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(socket.read_u8().await.expect("upgrade response byte"));
            assert!(headers.len() < 4096);
        }
        write_request(&mut socket, 1, method::SYSTEM_HELLO, json!({
            "clientName": "fluxdown-agent", "clientVersion": "test",
            "minProtocolVersion": fluxdown_protocol::MIN_PROTOCOL_VERSION,
            "maxProtocolVersion": fluxdown_protocol::PROTOCOL_VERSION,
            "requestedRole": "daemon", "capabilities": [],
        })).await;
        assert_eq!(read_message(&mut socket).await["id"], 1);

        // 起点 = 订阅时的游标；之后一帧都不读。数据量远超套接字缓冲，写端必然被卡住。
        let snapshot = events.snapshot();
        let epoch = snapshot.epoch.clone();
        let mut cursor = snapshot.sequence;
        let tasks = 50_usize;
        let storm = 80_000_usize;
        let mut last = cursor;
        for step in 0..storm {
            last = events.publish(progress_event(step % tasks, i64::try_from(step).expect("step"))).sequence;
            // 广播容量只有 1024：让接收任务有机会跟上，等价于真实运行时多线程调度。
            if step % 64 == 0 {
                tokio::task::yield_now().await;
            }
        }

        let mut received = 0_usize;
        while cursor < last {
            let value = read_message(&mut socket).await;
            assert_eq!(value["method"], method::SERVICE_EVENT, "{value}");
            let frame: EventFrame = serde_json::from_value(value["params"].clone()).expect("event frame");
            assert!(matches!(frame.event, ServiceEvent::Daemon(_)));
            assert_eq!(
                judge_frame(&epoch, cursor, &frame),
                FrameVerdict::Accept,
                "gap after sequence {cursor}: {:?}", frame.previous
            );
            cursor = frame.sequence;
            received += 1;
        }
        assert!(received < storm, "{received} frames received for {storm} published");

        drop(socket);
        cancel.cancel();
        server.await.expect("join HTTP server").expect("HTTP server shutdown");
        std::fs::remove_dir_all(directory).expect("remove isolated test directory");
    }).await.expect("storm must drain");
}
