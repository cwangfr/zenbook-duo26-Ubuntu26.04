pub mod client;
pub mod daemon;
pub mod logger;
pub mod logind;
pub mod monitor;
pub mod paths;
pub mod policy;
pub mod probe;
pub mod runtime_dir;
pub mod session_agent;
pub mod state;

pub mod display_safety;
pub mod store;

#[cfg(test)]
pub(crate) mod test_utils {
    use super::{state::RuntimeState, store::RuntimeStore};
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    pub fn state() -> RuntimeState {
        RuntimeState {
            storage: Arc::new(RuntimeStore::new(
                std::env::temp_dir()
                    .join(format!(
                        "duo-test-{}-{}",
                        std::process::id(),
                        NEXT.fetch_add(1, Ordering::Relaxed)
                    ))
                    .join("state.json"),
            )),
            ..RuntimeState::default()
        }
    }
}
pub(crate) mod display_backend;
pub(crate) mod dock_transaction;
