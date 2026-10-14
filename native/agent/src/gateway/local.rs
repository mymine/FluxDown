//! 进程内网关连接：嵌入式宿主（Android / iOS 应用）不经 WebSocket，直接取得与 UI 客户端
//! 完成 `system.hello` 之后等价的会话。握手校验、UI 计数、事件订阅与按通道并发的请求处理
//! 全部复用 [`run_socket`](super::run_socket) 的同一套实现。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use fluxdown_protocol::{
    ApplicationErrorCode, EventFrame, RequestId, RpcErrorData, RpcErrorObject, RpcRequest,
    RpcResponse, ServiceHello, ServiceRole, validate_first_request,
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{GatewayService, RequestLanes, declares_selection_ui};

type Waiters = Mutex<HashMap<RequestId, oneshot::Sender<RpcResponse>>>;

/// 进程内事件流的终止原因。
#[derive(Debug, thiserror::Error)]
pub enum LocalEventError {
    /// 消费速度落后于事件产生，已有 `skipped` 帧被丢弃：调用方必须用新的连接重新取快照。
    #[error("local event stream lagged behind by {skipped} frames")]
    Lagged { skipped: u64 },
    /// agent 已关闭，不会再有事件。
    #[error("local agent connection closed")]
    Closed,
}

/// 进程内 UI 连接。`call` 与 `next_event` 都只需 `&self`，可由 `Arc` 在多个任务间共享。
///
/// 丢弃连接等价于 WebSocket 断开：声明了 `client.selections` 的连接会在后台完成
/// `ui_disconnected` 记账并收尾请求通道。
pub struct LocalConnection {
    hello: ServiceHello,
    service: Arc<GatewayService>,
    lanes: Option<RequestLanes>,
    waiters: Arc<Waiters>,
    next_id: AtomicI64,
    events: tokio::sync::Mutex<broadcast::Receiver<EventFrame>>,
    cancel: CancellationToken,
    ui_client: bool,
    snapshot_ready: AtomicBool,
    /// `Drop` 可能发生在任意线程（如 FFI 对象被宿主语言的 GC 线程释放）：收尾任务投递到创建
    /// 连接的 runtime。
    runtime: tokio::runtime::Handle,
}

impl GatewayService {
    /// 校验 `system.hello` 请求（与 WebSocket 首帧完全相同的规则）并建立进程内连接。
    pub(crate) async fn connect_local(
        self: &Arc<Self>,
        hello: RpcRequest,
        cancel: CancellationToken,
    ) -> Result<LocalConnection, RpcErrorObject> {
        let client = validate_first_request(&hello, ServiceRole::Agent)
            .map_err(|data| RpcErrorObject::application("hello rejected", data))?;
        let ui_client = declares_selection_ui(&client);
        if ui_client {
            self.ui_connected().await;
        }
        let (events, lanes, responses) = self.open_session(true);
        let waiters = Arc::new(Waiters::default());
        tokio::spawn(route_responses(responses, Arc::clone(&waiters)));
        Ok(LocalConnection {
            hello: self.session_hello(true),
            service: Arc::clone(self),
            lanes: Some(lanes),
            waiters,
            next_id: AtomicI64::new(1),
            events: tokio::sync::Mutex::new(events),
            cancel,
            ui_client,
            snapshot_ready: AtomicBool::new(false),
            runtime: tokio::runtime::Handle::current(),
        })
    }
}

impl LocalConnection {
    /// agent 对 `system.hello` 的应答内容。
    #[must_use]
    pub fn hello(&self) -> &ServiceHello {
        &self.hello
    }

    /// 与 WebSocket 相同的通道语义：同一通道内按到达顺序处理，慢通道不阻塞其它通道。
    /// 响应的 `id` 恒为调用方给出的 `id`（同一连接上并发的重复 `id` 也不会串线）。
    pub async fn call(&self, mut request: RpcRequest) -> RpcResponse {
        let initial_snapshot =
            self.ui_client && request.method == fluxdown_protocol::method::SYSTEM_SNAPSHOT;
        let wire_id = RequestId::Integer(self.next_id.fetch_add(1, Ordering::Relaxed));
        let caller_id = std::mem::replace(&mut request.id, wire_id.clone());
        let (waiter, response) = oneshot::channel();
        lock(&self.waiters).insert(wire_id.clone(), waiter);
        let rejected = match &self.lanes {
            Some(lanes) => lanes.submit(request),
            None => Some(unavailable(wire_id.clone())),
        };
        if let Some(rejected) = rejected {
            lock(&self.waiters).remove(&wire_id);
            return with_id(rejected, caller_id);
        }
        match response.await {
            Ok(response) => {
                if initial_snapshot
                    && matches!(&response, RpcResponse::Success(_))
                    && !self.snapshot_ready.swap(true, Ordering::AcqRel)
                {
                    // 事件订阅早于快照请求，重放晚于快照生成；与 WebSocket 路径相同。
                    self.service.local.shell.ui_snapshot_ready();
                }
                with_id(response, caller_id)
            }
            Err(_) => with_id(unavailable(wire_id), caller_id),
        }
    }

    /// 下一帧事件（等价于 `service.event` 通知）。`Lagged` 后必须重新 `connect` 取快照。
    pub async fn next_event(&self) -> Result<EventFrame, LocalEventError> {
        let mut events = self.events.lock().await;
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(LocalEventError::Closed),
            frame = events.recv() => frame.map_err(|error| match error {
                broadcast::error::RecvError::Lagged(skipped) => LocalEventError::Lagged { skipped },
                broadcast::error::RecvError::Closed => LocalEventError::Closed,
            }),
        }
    }

    /// 显式关闭本机会话：等待请求通道结束并通知 UI 断开。
    pub async fn close(mut self) {
        let lanes = self.lanes.take();
        let ui_client = self.ui_client;
        self.ui_client = false;
        if ui_client {
            self.service
                .ui_disconnected(self.snapshot_ready.load(Ordering::Acquire))
                .await;
        }
        if let Some(lanes) = lanes {
            lanes.shutdown().await;
        }
    }
}

impl Drop for LocalConnection {
    fn drop(&mut self) {
        let lanes = self.lanes.take();
        let ui_client = self.ui_client;
        let was_ready = self.snapshot_ready.load(Ordering::Acquire);
        if lanes.is_none() && !ui_client {
            return;
        }
        let service = Arc::clone(&self.service);
        self.runtime.spawn(async move {
            if ui_client {
                service.ui_disconnected(was_ready).await;
            }
            if let Some(lanes) = lanes {
                lanes.shutdown().await;
            }
        });
    }
}

/// 把通道产生的响应送回等待它的 `call`；所有通道结束后唤醒仍在等待的调用方（得到 Unavailable）。
async fn route_responses(mut responses: mpsc::Receiver<RpcResponse>, waiters: Arc<Waiters>) {
    while let Some(response) = responses.recv().await {
        let Some(id) = response_id(&response).cloned() else {
            tracing::warn!("local agent connection dropped a response without an id");
            continue;
        };
        let waiter = lock(&waiters).remove(&id);
        match waiter {
            Some(waiter) => {
                if waiter.send(response).is_err() {
                    tracing::trace!("local caller dropped its response receiver");
                }
            }
            None => tracing::debug!(?id, "local agent response has no waiting caller"),
        }
    }
    lock(&waiters).clear();
}

fn lock(waiters: &Waiters) -> MutexGuard<'_, HashMap<RequestId, oneshot::Sender<RpcResponse>>> {
    waiters.lock().unwrap_or_else(PoisonError::into_inner)
}

fn response_id(response: &RpcResponse) -> Option<&RequestId> {
    match response {
        RpcResponse::Success(success) => Some(&success.id),
        RpcResponse::Failure(failure) => failure.id.as_ref(),
    }
}

fn with_id(mut response: RpcResponse, id: RequestId) -> RpcResponse {
    match &mut response {
        RpcResponse::Success(success) => success.id = id,
        RpcResponse::Failure(failure) => failure.id = Some(id),
    }
    response
}

fn unavailable(id: RequestId) -> RpcResponse {
    RpcResponse::failure(
        id,
        RpcErrorObject::application(
            "agent RPC failed",
            RpcErrorData::new(ApplicationErrorCode::Unavailable, true),
        ),
    )
}
