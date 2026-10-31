//! 票据验签：通过、过期、作用域、对端、未知 kid、被篡改等。

use fluxdown_share::ticket::{encode_public_key, sign_ticket};
use fluxdown_share::{
    KeyEntry, KeyRing, SCOPE_BRIDGE, SCOPE_RECV, SCOPE_SWARM, SecretKey, TicketError,
    TicketExpectation, parse_ticket, verify_ticket,
};

use crate::common::payload;

const NOW: u64 = 1_800_000_000;

struct Fx {
    issuer: SecretKey,
    ring: KeyRing,
    receiver: SecretKey,
}

fn fx() -> Fx {
    let issuer = SecretKey::generate();
    let ring = KeyRing::new();
    ring.replace(&[KeyEntry {
        kid: "k1".into(),
        public_key: encode_public_key(&issuer.public()),
        not_after: None,
    }])
    .unwrap();
    Fx {
        issuer,
        ring,
        receiver: SecretKey::generate(),
    }
}

fn check(
    fx: &Fx,
    raw: &str,
    share: Option<&str>,
    peer: &SecretKey,
    now: u64,
) -> Result<String, TicketError> {
    let ticket = parse_ticket(raw)?;
    verify_ticket(
        &fx.ring,
        &ticket,
        &TicketExpectation {
            now_unix: now,
            share_id: share,
            peer: peer.public(),
            scopes: &[SCOPE_RECV, SCOPE_BRIDGE],
        },
    )
    .map(|p| p.sid.clone())
}

fn good(fx: &Fx) -> String {
    sign_ticket(
        &fx.issuer,
        &payload("share-1", &fx.receiver.public().to_z32(), NOW + 60),
    )
    .unwrap()
}

#[test]
fn good_ticket_verifies() {
    let fx = fx();
    let raw = good(&fx);
    assert_eq!(
        check(&fx, &raw, Some("share-1"), &fx.receiver, NOW).as_deref(),
        Ok("share-1")
    );
    // 不核对分享 ID 时也通过（发送方先验票再查分享）。
    assert!(check(&fx, &raw, None, &fx.receiver, NOW).is_ok());
    // z32 大小写不敏感。
    let upper = sign_ticket(
        &fx.issuer,
        &payload(
            "share-1",
            &fx.receiver.public().to_z32().to_uppercase(),
            NOW + 60,
        ),
    )
    .unwrap();
    assert!(check(&fx, &upper, None, &fx.receiver, NOW).is_ok());
}

#[test]
fn expired_ticket_is_rejected() {
    let fx = fx();
    let raw = good(&fx);
    assert_eq!(
        check(&fx, &raw, None, &fx.receiver, NOW + 59).map(drop),
        Ok(())
    );
    assert_eq!(
        check(&fx, &raw, None, &fx.receiver, NOW + 60),
        Err(TicketError::Expired)
    );
    assert_eq!(
        check(&fx, &raw, None, &fx.receiver, NOW + 10_000),
        Err(TicketError::Expired)
    );
}

#[test]
fn wrong_scope_is_rejected() {
    let fx = fx();
    let mut p = payload("share-1", &fx.receiver.public().to_z32(), NOW + 60);
    p.scope = "admin".into();
    let raw = sign_ticket(&fx.issuer, &p).unwrap();
    assert_eq!(
        check(&fx, &raw, None, &fx.receiver, NOW),
        Err(TicketError::WrongScope("admin".into()))
    );
}

#[test]
fn bridge_scope_is_accepted_for_the_gateway_endpoint() {
    let fx = fx();
    let mut p = payload("share-1", &fx.receiver.public().to_z32(), NOW + 60);
    p.scope = "bridge".into();
    let raw = sign_ticket(&fx.issuer, &p).unwrap();
    assert_eq!(
        check(&fx, &raw, Some("share-1"), &fx.receiver, NOW).as_deref(),
        Ok("share-1")
    );
    // 桥接票据同样只发给 `rep` 指定的网关端点。
    assert_eq!(
        check(&fx, &raw, None, &SecretKey::generate(), NOW),
        Err(TicketError::WrongPeer)
    );
}

#[test]
fn wrong_peer_is_rejected() {
    let fx = fx();
    let raw = good(&fx);
    let stranger = SecretKey::generate();
    assert_eq!(
        check(&fx, &raw, None, &stranger, NOW),
        Err(TicketError::WrongPeer)
    );
}

#[test]
fn wrong_share_is_rejected() {
    let fx = fx();
    let raw = good(&fx);
    assert_eq!(
        check(&fx, &raw, Some("share-2"), &fx.receiver, NOW),
        Err(TicketError::WrongShare)
    );
}

#[test]
fn unknown_kid_and_retired_key_are_rejected() {
    let fx = fx();
    let mut p = payload("share-1", &fx.receiver.public().to_z32(), NOW + 60);
    p.kid = "k-unknown".into();
    let raw = sign_ticket(&fx.issuer, &p).unwrap();
    assert_eq!(
        check(&fx, &raw, None, &fx.receiver, NOW),
        Err(TicketError::UnknownKey("k-unknown".into()))
    );

    // notAfter 之后拒绝。
    fx.ring
        .replace(&[KeyEntry {
            kid: "k1".into(),
            public_key: encode_public_key(&fx.issuer.public()),
            not_after: Some(NOW - 1),
        }])
        .unwrap();
    assert_eq!(
        check(&fx, &good(&fx), None, &fx.receiver, NOW),
        Err(TicketError::KeyRetired("k1".into()))
    );
}

#[test]
fn tampered_tickets_are_rejected() {
    let fx = fx();
    let raw = good(&fx);
    let (payload_b64, sig_b64) = raw.split_once('.').unwrap();

    // 换一份 payload（延长有效期）但保留原签名。
    let forged = sign_ticket(
        &fx.issuer,
        &payload("share-1", &fx.receiver.public().to_z32(), NOW + 99_999),
    )
    .unwrap();
    let forged_payload = forged.split_once('.').unwrap().0;
    assert_eq!(
        check(
            &fx,
            &format!("{forged_payload}.{sig_b64}"),
            None,
            &fx.receiver,
            NOW
        ),
        Err(TicketError::BadSignature)
    );

    // 翻转签名的一个字符。
    let mut sig = sig_b64.to_owned().into_bytes();
    let mid = sig.len() / 2;
    sig[mid] = if sig[mid] == b'A' { b'B' } else { b'A' };
    let tampered = format!("{payload_b64}.{}", String::from_utf8(sig).unwrap());
    assert_eq!(
        check(&fx, &tampered, None, &fx.receiver, NOW),
        Err(TicketError::BadSignature)
    );

    // 由另一把密钥签、冒用 k1。
    let other = SecretKey::generate();
    let spoofed = sign_ticket(
        &other,
        &payload("share-1", &fx.receiver.public().to_z32(), NOW + 60),
    )
    .unwrap();
    assert_eq!(
        check(&fx, &spoofed, None, &fx.receiver, NOW),
        Err(TicketError::BadSignature)
    );
}

#[test]
fn malformed_tickets_are_rejected() {
    let fx = fx();
    for raw in ["", "nodot", "a.b.c", "!!!.???", "e30.e30", "e30."] {
        assert!(
            matches!(
                check(&fx, raw, None, &fx.receiver, NOW),
                Err(TicketError::Malformed(_))
            ),
            "{raw:?}"
        );
    }
}

#[test]
fn key_ring_replace_is_atomic() {
    let fx = fx();
    let bad = [
        KeyEntry {
            kid: "k2".into(),
            public_key: encode_public_key(&SecretKey::generate().public()),
            not_after: None,
        },
        KeyEntry {
            kid: "k3".into(),
            public_key: "not-base64-!!".into(),
            not_after: None,
        },
    ];
    assert!(fx.ring.replace(&bad).is_err());
    // 失败后原密钥仍可用。
    assert!(check(&fx, &good(&fx), None, &fx.receiver, NOW).is_ok());
    assert!(!fx.ring.is_empty());
}

/// swarm 票据（带 `root`）只在调用方列出 `swarm` 作用域时被接受；recv 票据在 swarm 端被拒绝。
#[test]
fn swarm_scope_is_only_accepted_when_listed() {
    let fx = fx();
    let mut p = payload("share-1", &fx.receiver.public().to_z32(), NOW + 60);
    p.scope = "swarm".into();
    p.root = Some("ab".repeat(32));
    let raw = sign_ticket(&fx.issuer, &p).unwrap();
    // 默认（recv / bridge）口径下被拒绝。
    assert_eq!(
        check(&fx, &raw, None, &fx.receiver, NOW),
        Err(TicketError::WrongScope("swarm".into()))
    );
    let ticket = parse_ticket(&raw).unwrap();
    let expect = |scopes: &'static [&'static str]| TicketExpectation {
        now_unix: NOW,
        share_id: None,
        peer: fx.receiver.public(),
        scopes,
    };
    let payload = verify_ticket(&fx.ring, &ticket, &expect(&[SCOPE_SWARM])).unwrap();
    assert_eq!(payload.root.as_deref(), Some("ab".repeat(32).as_str()));
    // recv 票据在只接受 swarm 的一端同样被拒绝。
    let recv = parse_ticket(&good(&fx)).unwrap();
    assert_eq!(
        verify_ticket(&fx.ring, &recv, &expect(&[SCOPE_SWARM])).map(drop),
        Err(TicketError::WrongScope("recv".into()))
    );
    // recv / bridge 票据不带 root 字段（字节与旧版本一致）。
    let json = serde_json::to_value(recv.payload()).unwrap();
    assert!(json.get("root").is_none());
}
