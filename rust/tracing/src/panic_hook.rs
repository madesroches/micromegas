//! Reports panics as fatal log entries and shuts down the telemetry system
use std::io::Write;
use std::panic::{PanicHookInfo, take_hook};
use std::sync::Mutex;

use crate::fatal;
use crate::guards::shutdown_telemetry;

pub fn init_panic_hook() {
    type BoxedHook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;
    static PREVIOUS_HOOK: Mutex<Option<BoxedHook>> = Mutex::new(None);

    {
        let mut previous_hook_lock = PREVIOUS_HOOK.lock().unwrap();
        assert!(previous_hook_lock.is_none());
        *previous_hook_lock = Some(take_hook());
    }

    std::panic::set_hook(Box::new(|panic_info| {
        fatal!("panic: {:?}", panic_info);
        shutdown_telemetry();
        if let Ok(guard) = PREVIOUS_HOOK.lock()
            && let Some(hook) = guard.as_ref()
        {
            std::io::stdout().flush().unwrap();
            hook(panic_info);
        }
    }));
}
