//! Legacy snapshot migration: pre-`#22` JSONL snapshots (cleartext
//! `api_key`, no trailing `checksum` line) are hashed into the lookup on
//! first load instead of being rejected.
//!
//! Split out of [`crate::persistence`] to keep that module under the
//! repository's 500-line file limit.

use crate::http_api::B2bState;
use crate::persistence::{hash_credential, parse_tier};
use crate::webhooks::{json_str, json_u64};
use std::io;

/// Parses a pre-`#22` legacy snapshot (cleartext `api_key`, no `checksum`
/// line), hashing each cleartext credential into the lookup on first load.
///
/// Returns `Ok` with the migrated tenant count logged; truncated or
/// malformed content fails closed with `InvalidData` and leaves `state`
/// untouched.
pub fn load_legacy_snapshot_str(state: &mut B2bState, content: &str) -> io::Result<()> {
    let mut tenants: Vec<crate::http_api::RestoredTenant> = Vec::new();
    let mut revoked: Vec<String> = Vec::new();
    let mut migrated_cleartext = 0u64;
    for line in content.lines().map(str::trim).filter(|l| !l.is_empty()) {
        // Fail closed on truncation: a kill-during-write leaves unbalanced
        // lines that must not parse as a "legacy" snapshot.
        if !(line.starts_with('{') && line.ends_with('}') && braces_balanced(line)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot truncated or malformed",
            ));
        }
        match json_str(line, "kind").as_deref() {
            Some("tenant") => {
                let did = json_str(line, "did").unwrap_or_default();
                let (key_hash, was_cleartext) =
                    match json_str(line, "api_key_sha256").filter(|h| !h.trim().is_empty()) {
                        Some(hash) => (hash, false),
                        None => {
                            let clear = json_str(line, "api_key").unwrap_or_default();
                            if clear.trim().is_empty() {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "snapshot tenant entry invalid",
                                ));
                            }
                            (hash_credential(clear.trim()), true)
                        }
                    };
                let tier = parse_tier(&json_str(line, "tier").unwrap_or_default());
                let max_seats = json_u64(line, "max_seats").unwrap_or(10) as u32;
                if did.trim().is_empty() || key_hash.trim().is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "snapshot tenant entry invalid",
                    ));
                }
                if was_cleartext {
                    migrated_cleartext += 1;
                }
                tenants.push(crate::http_api::RestoredTenant {
                    did,
                    api_key_hash: key_hash,
                    tier,
                    max_seats,
                    promo_requests: json_u64(line, "promo_requests").unwrap_or(0),
                    promo_window_start: json_u64(line, "promo_window_start").unwrap_or(0),
                    promo_window_count: json_u64(line, "promo_window_count").unwrap_or(0),
                    card_actions: json_u64(line, "card_actions").unwrap_or(0),
                    messages_sent: json_u64(line, "messages_sent").unwrap_or(0),
                });
            }
            Some("revoked") => {
                if let Some(k) = json_str(line, "key")
                    && !k.is_empty()
                {
                    revoked.push(k);
                }
            }
            _ => {}
        }
    }
    for tenant in tenants {
        state.register_hashed_tenant(tenant);
    }
    for k in revoked {
        state.revoked_consents.insert(k);
    }
    eprintln!(
        "{{\"level\":\"WARN\",\"service\":\"sonaraim-b2b\",\"component\":\"persistence\",\"migrated_tenants\":{migrated_cleartext},\"note\":\"legacy_cleartext_snapshot_migrated_to_hashed\"}}"
    );
    Ok(())
}

/// Reports whether every `{` in `line` is closed by a later `}` (naive
/// brace balance for single-line JSONL records; braces inside quoted
/// strings are rare in snapshots and only make the check stricter).
fn braces_balanced(line: &str) -> bool {
    let mut depth = 0i32;
    for ch in line.chars() {
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth < 0 {
                return false;
            }
        }
    }
    depth == 0
}
