//! One shared async runtime for phone (MTP) work, so the engine does not depend on a window toolkit.

use std::future::Future;
use std::sync::OnceLock;
use tokio::runtime::{Builder, Runtime};

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("backup-engine")
            .enable_all()
            .build()
            .expect("the engine runtime starts")
    })
}

/// Runs a future to completion on the engine runtime. Call it from a plain thread, not from async code.
pub fn block_on<F: Future>(future: F) -> F::Output {
    runtime().block_on(future)
}

/// Starts a future in the background on the engine runtime.
pub fn spawn<F>(future: F)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(future);
}
