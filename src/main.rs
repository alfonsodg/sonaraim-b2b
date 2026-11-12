//! Entrypoint for the `sonaraim-b2b` Enterprise Bridge binary.

use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::{B2bState, run_server};
use std::process;

/// Loads runtime configuration from environment variables, logs structured JSON status,
/// restores durable state if configured, and starts the Enterprise B2B HTTP API server.
fn main() {
    match B2bConfig::from_env() {
        Ok(cfg) => {
            println!(
                "{{\"level\":\"INFO\",\"service\":\"sonaraim-b2b\",\"status\":\"ready\",\"http_host\":\"{}\",\"http_port\":{},\"grpc_port\":{},\"node_rpc_url\":\"{}\",\"instant_clip_base_url\":\"{}\",\"promo_rate_limit_rps\":{}}}",
                cfg.http_host,
                cfg.http_port,
                cfg.grpc_port,
                cfg.node_rpc_url,
                cfg.instant_clip_base_url,
                cfg.promo_rate_limit_rps
            );
            let mut state = B2bState::new(cfg);
            let storage_path = std::env::var("B2B_STORAGE_PATH")
                .unwrap_or_else(|_| "/tmp/sonaraim-b2b-state.jsonl".to_string());
            state.set_storage_path(&storage_path);
            state.register_bootstrap_tenant(
                std::env::var("B2B_BOOTSTRAP_TENANT_DID").ok().as_deref(),
                std::env::var("B2B_BOOTSTRAP_API_KEY").ok().as_deref(),
            );
            if let Err(err) = run_server(state) {
                eprintln!(
                    "{{\"level\":\"ERROR\",\"service\":\"sonaraim-b2b\",\"error\":\"{err}\"}}"
                );
                process::exit(1);
            }
        }
        Err(err) => {
            eprintln!(
                "{{\"level\":\"ERROR\",\"service\":\"sonaraim-b2b\",\"error\":\"{}\"}}",
                err
            );
            process::exit(1);
        }
    }
}
