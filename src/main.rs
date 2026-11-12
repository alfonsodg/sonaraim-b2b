//! Entrypoint for the `sonaraim-b2b` Enterprise Bridge binary.

use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::{B2bState, run_server};
use std::process;

/// Loads runtime configuration from environment variables, logs structured JSON status,
/// and starts the Enterprise B2B HTTP API server.
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
            if let Err(err) = run_server(B2bState::new(cfg)) {
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
