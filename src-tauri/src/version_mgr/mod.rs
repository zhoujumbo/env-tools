pub mod download;
pub mod golang;
pub mod java;
pub mod node;
pub mod progress;
pub mod python;
pub mod store;

pub use store::{RuntimeStatus, RuntimeStore};

use anyhow::{bail, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub(crate) static RUNTIME_OP: Mutex<()> = Mutex::new(());
static CANCEL: AtomicBool = AtomicBool::new(false);

pub const CANCEL_MSG: &str = "已取消";

pub fn request_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub(crate) fn reset_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

pub(crate) fn is_cancelled() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

/// 是否有安装/下载在跑。用来挡住「闲置释放 webview」和构建菜单之外的危险动作。
pub fn op_running() -> bool {
    RUNTIME_OP.try_lock().is_err()
}

pub(crate) fn check_cancel() -> Result<()> {
    if is_cancelled() {
        bail!("{CANCEL_MSG}");
    }
    Ok(())
}

pub(crate) fn with_runtime_op<T>(store: &RuntimeStore, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let _op = RUNTIME_OP
        .lock()
        .map_err(|_| anyhow::anyhow!("runtime op lock poisoned"))?;
    reset_cancel();
    store.cleanup_leftovers();
    match f() {
        Ok(v) => Ok(v),
        Err(e) => {
            store.cleanup_leftovers();
            Err(e)
        }
    }
}
