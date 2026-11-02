//! Shared application state and error types.

pub mod flag;
pub mod machines;
pub mod releases;
pub mod session;
pub mod submissions;
pub mod writeups;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HmvError {
    #[error("Authentication failed. Please check your username and password.")]
    AuthFailed,
    #[error(
        "This build talks to endpoints that no longer exist (HTTP 404) — reinstall hmv: \
         `cargo install --path .` (or `--git ... --force`)."
    )]
    EndpointGone,
    #[error("Error: Machine '{0}' not found.")]
    MachineNotFound(String),
}
