//! Reports panics as fatal log entries and shuts down the telemetry system
use std::io::Write;
use std::panic::{take_hook, PanicHookInfo};

use crate::fatal;
use crate::guards::shutdown_telemetry;

pub fn init_panic_hook() {
    type BoxedHook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;
    static mut PREVIOUS_HOOK: Option<BoxedHook> = None;
    unsafe {
        assert!(PREVIOUS_HOOK.is_none());
        PREVIOUS_HOOK = Some(take_hook());
    }

    std::panic::set_hook(Box::new(|panic_info| unsafe {
        fatal!("panic: {:?}", panic_info);
        shutdown_telemetry();
        if let Some(hook) = PREVIOUS_HOOK.as_ref() {
            std::io::stdout().flush().unwrap();
            hook(panic_info);
        }
    }));
}
