use std::io::Write;
use std::panic::{take_hook, PanicInfo};

use crate::error;
use crate::guards::shutdown_telemetry;

pub fn init_panic_hook() {
    type BoxedHook = Box<dyn Fn(&PanicInfo<'_>) + Sync + Send + 'static>;
    static mut PREVIOUS_HOOK: Option<BoxedHook> = None;
    unsafe {
        assert!(PREVIOUS_HOOK.is_none());
        PREVIOUS_HOOK = Some(take_hook());
    }

    std::panic::set_hook(Box::new(|panic_info| unsafe {
        error!("panic: {:?}", panic_info);
        shutdown_telemetry();
        if let Some(hook) = PREVIOUS_HOOK.as_ref() {
            std::io::stdout().flush().unwrap();
            hook(panic_info);
        }
    }));
}
