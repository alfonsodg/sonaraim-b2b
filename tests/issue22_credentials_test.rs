//! TDD tests for issue #22: tenant credentials are hashed at rest with a
//! separate HMAC signing secret, snapshot writes are atomic (tmp + fsync +
//! rename), and a corrupted snapshot fails closed with a clean load.

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::B2bState;
use sonaraim_b2b::persistence::{load_snapshot, save_snapshot};
use std::time::{SystemTime, UNIX_EPOCH};

fn sample_config() -> B2bConfig {
    B2bConfig {
        http_host: "127.0.0.1".to_string(),
        http_port: 8041,
        grpc_port: 8042,
        node_rpc_url: "http://127.0.0.1:18031".to_string(),
        instant_clip_base_url: "https://go.sonaraim.com".to_string(),
        promo_rate_limit_rps: 50,
        storage_path: "/var/lib/sonaraim-b2b/test-state.jsonl".to_string(),
        allow_insecure_callbacks: false,
    }
}

fn snap_path(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir()
        .join(format!(
            "sonaraim_b2b_22_{tag}_{nanos}_{}.jsonl",
            std::process::id()
        ))
        .to_string_lossy()
        .to_string()
}

#[test]
fn api_keys_are_hashed_at_rest_with_separate_signing_secret() {
    let path = snap_path("hash");
    let mut state = B2bState::new(sample_config());
    state.register_tenant(
        "did:web:vault.example.com",
        "sk_super_secret_key",
        SaasPlanTier::SmbPro,
        5,
    );
    save_snapshot(&state, &path).expect("save snapshot");
    let raw = std::fs::read_to_string(&path).expect("read snapshot");

    // The cleartext Bearer key must never hit disk.
    assert!(
        !raw.contains("sk_super_secret_key"),
        "cleartext api key leaked to snapshot"
    );
    // A key hash and a distinct signing secret hash are persisted instead.
    assert!(raw.contains("api_key_sha256"));
    assert!(raw.contains("signing_secret_sha256"));

    // Reload works: Bearer auth still resolves via the hashed lookup.
    let mut restored = B2bState::new(sample_config());
    load_snapshot(&mut restored, &path).expect("reload snapshot");
    assert!(restored.tenants.contains_key("did:web:vault.example.com"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn snapshot_write_is_atomic_and_corruption_fails_closed() {
    let path = snap_path("atomic");
    let mut state = B2bState::new(sample_config());
    state.register_tenant(
        "did:web:atomic.example.com",
        "sk_atomic_1",
        SaasPlanTier::Free,
        1,
    );
    save_snapshot(&state, &path).expect("save snapshot");

    // Simulate a kill-during-write: truncate the file mid-record.
    let raw = std::fs::read_to_string(&path).expect("read snapshot");
    let half = raw.len() / 2;
    std::fs::write(&path, &raw[..half]).expect("truncate snapshot");

    // Load fails closed (checksum/integrity error) instead of half-loading.
    let mut restored = B2bState::new(sample_config());
    assert!(
        load_snapshot(&mut restored, &path).is_err(),
        "corrupt snapshot must fail closed"
    );
    assert!(
        restored.tenants.is_empty(),
        "no partial tenant state after corrupt load"
    );
    let _ = std::fs::remove_file(&path);
}
