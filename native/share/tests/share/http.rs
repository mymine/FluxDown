//! 适配器的 HTTP 语义与错误映射：409 / 410 / 403 / 503、Range 边界、Host 校验、对端不符。

use std::time::{Duration, SystemTime};

use fluxdown_share::{SessionEvent, ShareState, ticket::sign_ticket};
use reqwest::StatusCode;

use crate::common::{http, payload, rig, rig_with};

const MIB: usize = 1024 * 1024;

fn status_tag(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("x-fluxshare-status")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

#[tokio::test]
async fn changed_source_file_yields_409() {
    let mut rig = rig("share-changed", MIB + 17).await;
    let url = rig.url();
    // 先确认正常。
    assert_eq!(
        http().get(&url).send().await.unwrap().status(),
        StatusCode::OK
    );

    // 内容与大小都变了。
    let mut longer = rig.file.data.clone();
    longer.push(0);
    std::fs::write(&rig.file.path, &longer).unwrap();
    let resp = http().get(&url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(status_tag(&resp), "changed");
    // HEAD 同样暴露。
    assert_eq!(
        http().head(&url).send().await.unwrap().status(),
        StatusCode::CONFLICT
    );

    // 还原内容，但只改修改时间（大小不变）：同样判 changed。
    std::fs::write(&rig.file.path, &rig.file.data).unwrap();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&rig.file.path)
        .unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
    drop(file);
    assert_eq!(
        http().get(&url).send().await.unwrap().status(),
        StatusCode::CONFLICT
    );

    // 文件被删除：也是 409。
    std::fs::remove_file(&rig.file.path).unwrap();
    assert_eq!(
        http().get(&url).send().await.unwrap().status(),
        StatusCode::CONFLICT
    );

    // 每次 changed 都经事件通道通知宿主（正常请求不产生该事件）。
    let mut changed = 0;
    while let Ok(event) = rig.sender.events.try_recv() {
        if let SessionEvent::SourceChanged { share_id } = event {
            assert_eq!(share_id, "share-changed");
            changed += 1;
        }
    }
    assert!(changed >= 1, "宿主应收到 SourceChanged，实际 {changed}");
}

#[tokio::test]
async fn stopped_and_expired_shares_yield_410() {
    let rig = rig("share-stop", MIB).await;
    let url = rig.url();
    assert_eq!(
        http().get(&url).send().await.unwrap().status(),
        StatusCode::OK
    );

    // 分享被停止：已建立的会话上的下一个请求立刻得到 410。
    rig.sender
        .registry
        .set_state("share-stop", ShareState::Stopped);
    let resp = http().get(&url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::GONE);
    assert_eq!(status_tag(&resp), "stopped");

    // 到期时间已过（登记表按时间判定）。
    rig.sender
        .registry
        .set_state("share-stop", ShareState::Active);
    assert_eq!(
        http().get(&url).send().await.unwrap().status(),
        StatusCode::OK
    );
    rig.sender.registry.set_expiry("share-stop", Some(1));
    let resp = http().get(&url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::GONE);
    assert_eq!(status_tag(&resp), "expired");
    assert_eq!(rig.sender.registry.active_count(), 0);
}

#[tokio::test]
async fn expired_ticket_and_stopped_share_are_refused_at_hello() {
    // 票据已过期：握手即被拒，适配器 410。
    let r = rig("share-late", MIB).await;
    let ticket = sign_ticket(
        &r.signer.secret,
        &payload(
            "share-late",
            &r.receiver.ep.id_z32(),
            fluxdown_share::timefmt::now_unix_secs() - 5,
        ),
    )
    .unwrap();
    let mut params =
        fluxdown_share::ReceiveParams::new(r.file.manifest.clone(), ticket, r.sender.ep.id());
    params.addr_hint = Some(crate::common::loopback_hint(&r.sender.ep).await);
    let session = fluxdown_share::ReceiveSession::new(r.receiver.ep.clone(), params);
    let token = r.adapter.register(session);
    let resp = http().get(r.adapter.url(&token, 0)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::GONE);
    assert_eq!(status_tag(&resp), "expired");

    // 分享在握手前就已停止。
    r.sender
        .registry
        .set_state("share-late", ShareState::Stopped);
    let fresh = fluxdown_share::ReceiveSession::new(r.receiver.ep.clone(), {
        let t = sign_ticket(
            &r.signer.secret,
            &payload(
                "share-late",
                &r.receiver.ep.id_z32(),
                fluxdown_share::timefmt::now_unix_secs() + 600,
            ),
        )
        .unwrap();
        let mut p =
            fluxdown_share::ReceiveParams::new(r.file.manifest.clone(), t, r.sender.ep.id());
        p.addr_hint = Some(crate::common::loopback_hint(&r.sender.ep).await);
        p
    });
    let token = r.adapter.register(fresh);
    let resp = http().get(r.adapter.url(&token, 0)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::GONE);
    assert_eq!(status_tag(&resp), "stopped");
}

#[tokio::test]
async fn bad_capability_token_yields_403() {
    let rig = rig("share-token", 4096).await;
    let base = rig.adapter.local_addr();
    for path in [
        "/deadbeef/0".to_owned(),
        "/".to_owned(),
        format!("/{}x/0", rig.token),
        "/../0".to_owned(),
    ] {
        let resp = http()
            .get(format!("http://{base}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{path}");
        assert_eq!(status_tag(&resp), "badToken");
    }
    // 注销后原令牌失效。
    assert_eq!(
        http().get(rig.url()).send().await.unwrap().status(),
        StatusCode::OK
    );
    assert!(rig.adapter.unregister(&rig.token));
    assert_eq!(
        http().get(rig.url()).send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn host_header_and_method_are_validated() {
    let rig = rig("share-host", 4096).await;
    let url = rig.url();
    // DNS 重绑定防护：非回环 Host 头一律 403。
    let resp = http()
        .get(&url)
        .header("Host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(status_tag(&resp), "host");
    let resp = http()
        .get(&url)
        .header("Host", "localhost:1234")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // 只读方法。
    assert_eq!(
        http().post(&url).send().await.unwrap().status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    // 超出清单的文件下标。
    let other = rig.adapter.url(&rig.token, 7);
    assert_eq!(
        http().get(other).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    // 适配器只监听回环地址。
    assert!(rig.adapter.local_addr().ip().is_loopback());
}

#[tokio::test]
async fn ticket_for_another_peer_is_rejected() {
    let r = rig("share-peer", 4096).await;
    // 票据签给了另一个 endpoint：QUIC 对端（本接收方）与 rep 不符。
    let stranger = fluxdown_share::SecretKey::generate();
    let ticket = sign_ticket(
        &r.signer.secret,
        &payload(
            "share-peer",
            &stranger.public().to_z32(),
            fluxdown_share::timefmt::now_unix_secs() + 600,
        ),
    )
    .unwrap();
    let mut params =
        fluxdown_share::ReceiveParams::new(r.file.manifest.clone(), ticket, r.sender.ep.id());
    params.addr_hint = Some(crate::common::loopback_hint(&r.sender.ep).await);
    let session = fluxdown_share::ReceiveSession::new(r.receiver.ep.clone(), params);
    let token = r.adapter.register(session.clone());
    let resp = http().get(r.adapter.url(&token, 0)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(status_tag(&resp), "rejected");
    let err = session.probe(0).await.unwrap_err();
    assert!(format!("{err}").contains("PeerMismatch"), "{err}");

    // 签名被篡改 / 未知 kid：同样是 rejected。
    let garbled = format!("{}x", r.signer.secret.public());
    let mut params =
        fluxdown_share::ReceiveParams::new(r.file.manifest.clone(), garbled, r.sender.ep.id());
    params.addr_hint = Some(crate::common::loopback_hint(&r.sender.ep).await);
    let session = fluxdown_share::ReceiveSession::new(r.receiver.ep.clone(), params);
    let err = session.probe(0).await.unwrap_err();
    assert!(format!("{err}").contains("BadTicket"), "{err}");
}

#[tokio::test]
async fn offline_sender_yields_503_with_retry_after() {
    let r = rig_with("share-offline", 4096, |p| {
        p.connect_timeout = Duration::from_secs(3)
    })
    .await;
    let url = r.url();
    assert_eq!(
        http().get(&url).send().await.unwrap().status(),
        StatusCode::OK
    );

    // 分享方关闭端点（模拟离线）：接收方重连失败 → 503 + Retry-After。
    r.sender.ep.shutdown().await;
    let resp = http().get(&url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(resp.headers()["retry-after"], "5");
    assert_eq!(status_tag(&resp), "offline");
}

#[tokio::test]
async fn range_edge_cases() {
    let rig = rig("share-range", 10_000).await;
    let url = rig.url();
    let data = &rig.file.data;
    let get = |range: &'static str| {
        let url = url.clone();
        async move { http().get(url).header("Range", range).send().await.unwrap() }
    };

    // 闭区间、开放区间、后缀区间、越界截断。
    let r = get("bytes=100-199").await;
    assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.headers()["content-range"], "bytes 100-199/10000");
    assert_eq!(&r.bytes().await.unwrap()[..], &data[100..200]);

    let r = get("bytes=9990-").await;
    assert_eq!(r.headers()["content-range"], "bytes 9990-9999/10000");
    assert_eq!(&r.bytes().await.unwrap()[..], &data[9990..]);

    let r = get("bytes=-50").await;
    assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(&r.bytes().await.unwrap()[..], &data[9950..]);

    let r = get("bytes=9000-99999").await;
    assert_eq!(r.headers()["content-range"], "bytes 9000-9999/10000");

    // 单字节。
    let r = get("bytes=0-0").await;
    assert_eq!(r.headers()["content-length"], "1");
    assert_eq!(&r.bytes().await.unwrap()[..], &data[..1]);

    // 不可满足：416 + `bytes */size`。
    let r = get("bytes=10000-").await;
    assert_eq!(r.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(r.headers()["content-range"], "bytes */10000");
    assert_eq!(
        get("bytes=-0").await.status(),
        StatusCode::RANGE_NOT_SATISFIABLE
    );

    // 多区间 / 非 bytes 单位 / 语法错误：忽略 Range，整文件 200。
    for ignored in ["bytes=0-1,5-6", "items=0-1", "bytes=abc", "bytes=5-2"] {
        let r = get(ignored).await;
        assert_eq!(r.status(), StatusCode::OK, "{ignored}");
        assert_eq!(r.bytes().await.unwrap().len(), 10_000, "{ignored}");
    }

    // If-Range：匹配 ETag 时尊重 Range，不匹配时返回整文件。
    let etag = format!("\"{}\"", rig.file.manifest.files[0].root_hash);
    let r = http()
        .get(&url)
        .header("Range", "bytes=0-9")
        .header("If-Range", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
    let r = http()
        .get(&url)
        .header("Range", "bytes=0-9")
        .header("If-Range", "\"stale\"")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.bytes().await.unwrap().len(), 10_000);

    // Content-Disposition 带文件名。
    let r = http().head(&url).send().await.unwrap();
    assert_eq!(
        r.headers()["content-disposition"],
        "attachment; filename*=UTF-8''payload.bin"
    );
}

#[tokio::test]
async fn zero_byte_file_is_served() {
    let rig = rig("share-empty", 0).await;
    let resp = http().get(rig.url()).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-length"], "0");
    assert!(resp.bytes().await.unwrap().is_empty());
}
