/// Records a sync span as two thread events
///
/// # Examples
///
/// ```
/// use micromegas_tracing::span_scope;
///
/// # fn main() {
/// span_scope!("scope");
/// # }
/// ```
#[macro_export]
macro_rules! span_scope {
    ($scope_name:ident, $name:expr) => {
        static $scope_name: $crate::spans::SpanMetadata = $crate::spans::SpanMetadata {
            name: $name,
            location: $crate::spans::SpanLocation {
                lod: $crate::levels::Verbosity::Max,
                target: module_path!(),
                module_path: module_path!(),
                file: file!(),
                line: line!(),
            },
        };
        let guard_named = $crate::guards::ThreadSpanGuard::new(&$scope_name);
    };
    ($name:expr) => {
        $crate::span_scope!(_METADATA_NAMED, $name);
    };
}

/// Records a span with a name that is determined at runtime.
/// The span name still needs to be statically allocated.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::span_scope_named;
///
/// # fn get_name() -> &'static str {
/// #   return "name";
/// # }
/// #
/// # fn main() {
/// span_scope_named!(get_name());
/// # }
/// ```
#[macro_export]
macro_rules! span_scope_named {
    ($scope_name:ident, $name:expr) => {
        static $scope_name: $crate::spans::SpanLocation = $crate::spans::SpanLocation {
            lod: $crate::levels::Verbosity::Max,
            target: module_path!(),
            module_path: module_path!(),
            file: file!(),
            line: line!(),
        };
        let guard_named = $crate::guards::ThreadNamedSpanGuard::new(&$scope_name, $name);
    };
    ($name:expr) => {
        $crate::span_scope_named!(_METADATA_NAMED, $name);
    };
}

/// async_span_scope is not supported yet
#[macro_export]
macro_rules! async_span_scope {
    ($scope_name:ident, $name:expr) => {
        static $scope_name: $crate::spans::SpanMetadata = $crate::spans::SpanMetadata {
            name: $name,
            location: $crate::spans::SpanLocation {
                lod: $crate::levels::Verbosity::Max,
                target: module_path!(),
                module_path: module_path!(),
                file: file!(),
                line: line!(),
            },
        };
        let guard_named = $crate::guards::AsyncSpanGuard::new(&$scope_name);
    };
    ($name:expr) => {
        $crate::async_span_scope!(_METADATA_NAMED, $name);
    };
}

/// async_span_scope_named is not supported yet
#[macro_export]
macro_rules! async_span_scope_named {
    ($scope_name:ident, $name:expr) => {
        static $scope_name: $crate::spans::SpanLocation = $crate::spans::SpanLocation {
            lod: $crate::levels::Verbosity::Max,
            target: module_path!(),
            module_path: module_path!(),
            file: file!(),
            line: line!(),
        };
        let guard_named = $crate::guards::AsyncNamedSpanGuard::new(&$scope_name, $name);
    };
    ($name:expr) => {
        $crate::async_span_scope_named!(_METADATA_NAMED, $name);
    };
}

/// Records a integer metric.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::imetric;
///
/// # fn main() {
/// #
/// imetric!("Frame Time", "ticks", 1000);
/// # }
/// ```
#[macro_export]
macro_rules! imetric {
    ($name:literal, $unit:literal, $value:expr) => {{
        static METRIC_METADATA: $crate::metrics::MetricMetadata = $crate::metrics::MetricMetadata {
            lod: $crate::levels::Verbosity::Max,
            name: $name,
            unit: $unit,
            target: module_path!(),
            module_path: module_path!(),
            file: file!(),
            line: line!(),
        };
        $crate::dispatch::int_metric(&METRIC_METADATA, $value);
    }};
}

/// Records a float metric.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::fmetric;
///
/// # fn main() {
/// #
/// fmetric!("Frame Time", "ticks", 1000.0);
/// # }
/// ```
#[macro_export]
macro_rules! fmetric {
    ($name:literal, $unit:literal, $value:expr) => {{
        static METRIC_METADATA: $crate::metrics::MetricMetadata = $crate::metrics::MetricMetadata {
            lod: $crate::levels::Verbosity::Max,
            name: $name,
            unit: $unit,
            target: module_path!(),
            module_path: module_path!(),
            file: file!(),
            line: line!(),
        };
        $crate::dispatch::float_metric(&METRIC_METADATA, $value);
    }};
}

/// The standard logging macro.
///
/// This macro will generically log with the specified `Level` and `format!`
/// based argument list.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # fn main() {
/// let data = (42, "Forty-two");
/// let private_data = "private";
///
/// log!(Level::Error, "Received errors: {}, {}", data.0, data.1);
/// log!(target: "app_events", Level::Warn, "App warning: {}, {}, {}",
///     data.0, data.1, private_data);
/// # }
/// ```
#[macro_export]
macro_rules! log {
    (target: $target:expr, $lvl:expr, $($arg:tt)+) => ({
        static LOG_DESC: $crate::logs::LogMetadata = $crate::logs::LogMetadata {
            level: $lvl,
            level_filter: std::sync::atomic::AtomicU32::new($crate::logs::FILTER_LEVEL_UNSET_VALUE),
            fmt_str: $crate::__first_arg!($($arg)+),
            target: $target,
            module_path: $crate::__log_module_path!(),
            file: file!(),
            line: line!(),
        };
        if $lvl <= $crate::levels::STATIC_MAX_LEVEL && $lvl <= $crate::levels::max_level() {
            $crate::dispatch::log(&LOG_DESC, format_args!($($arg)+));
        }
    });
    ($lvl:expr, $($arg:tt)+) => ($crate::log!(target: $crate::__log_module_path!(), $lvl, $($arg)+))
}
/// Logs a message at the error level.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # fn main() {
/// let (err_info, port) = ("No connection", 22);
///
/// error!("Error: {} on port {}", err_info, port);
/// error!(target: "app_events", "App Error: {}, Port: {}", err_info, 22);
/// # }
/// ```
#[macro_export]
macro_rules! error {
    (target: $target:expr, $($arg:tt)+) => (
        $crate::log!(target: $target, $crate::levels::Level::Error, $($arg)+)
    );
    ($($arg:tt)+) => (
        $crate::log!($crate::levels::Level::Error, $($arg)+)
    )
}

/// Logs a message at the warn level.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # fn main() {
/// let warn_description = "Invalid Input";
///
/// warn!("Warning! {}!", warn_description);
/// warn!(target: "input_events", "App received warning: {}", warn_description);
/// # }
/// ```
#[macro_export]
macro_rules! warn {
    (target: $target:expr, $($arg:tt)+) => (
        $crate::log!(target: $target, $crate::levels::Level::Warn, $($arg)+)
    );
    ($($arg:tt)+) => (
        $crate::log!($crate::levels::Level::Warn, $($arg)+)
    )
}

/// Logs a message at the info level.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # fn main() {
/// # struct Connection { port: u32, speed: f32 }
/// let conn_info = Connection { port: 40, speed: 3.20 };
///
/// info!("Connected to port {} at {} Mb/s", conn_info.port, conn_info.speed);
/// info!(target: "connection_events", "Successfull connection, port: {}, speed: {}",
///       conn_info.port, conn_info.speed);
/// # }
/// ```
#[macro_export]
macro_rules! info {
    (target: $target:expr, $($arg:tt)+) => (
        $crate::log!(target: $target, $crate::levels::Level::Info, $($arg)+)
    );
    ($($arg:tt)+) => (
        $crate::log!($crate::levels::Level::Info, $($arg)+)
    )
}

/// Logs a message at the debug level.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::debug;
///
/// # fn main() {
/// # struct Position { x: f32, y: f32 }
/// let pos = Position { x: 3.234, y: -1.223 };
///
/// debug!("New position: x: {}, y: {}", pos.x, pos.y);
/// debug!(target: "app_events", "New position: x: {}, y: {}", pos.x, pos.y);
/// # }
/// ```
#[macro_export]
macro_rules! debug {
    (target: $target:expr, $($arg:tt)+) => (
        $crate::log!(target: $target, $crate::levels::Level::Debug, $($arg)+)
    );
    ($($arg:tt)+) => (
        $crate::log!($crate::levels::Level::Debug, $($arg)+)
    )
}

/// Logs a message at the trace level.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::trace;
///
/// # fn main() {
/// # struct Position { x: f32, y: f32 }
/// let pos = Position { x: 3.234, y: -1.223 };
///
/// trace!("Position is: x: {}, y: {}", pos.x, pos.y);
/// trace!(target: "app_events", "x is {} and y is {}",
///        if pos.x >= 0.0 { "positive" } else { "negative" },
///        if pos.y >= 0.0 { "positive" } else { "negative" });
/// # }
/// ```
#[macro_export]
macro_rules! trace {
    (target: $target:expr, $($arg:tt)+) => (
        $crate::log!(target: $target, $crate::levels::Level::Trace, $($arg)+)
    );
    ($($arg:tt)+) => (
        $crate::log!($crate::levels::Level::Trace, $($arg)+)
    )
}

/// Determines if a message logged at the specified level in that module will
/// be logged.
///
/// This can be used to avoid expensive computation of log message arguments if
/// the message would be ignored anyway.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # fn foo() {
/// if log_enabled!(Level::Debug) {
///     let data = expensive_call();
///     debug!("expensive debug data: {} {}", data.x, data.y);
/// }
/// if log_enabled!(target: "Global", Level::Debug) {
///    let data = expensive_call();
///    debug!(target: "Global", "expensive debug data: {} {}", data.x, data.y);
/// }
/// # }
/// # struct Data { x: u32, y: u32 }
/// # fn expensive_call() -> Data { Data { x: 0, y: 0 } }
/// # fn main() {}
/// ```
#[macro_export(local_inner_macros)]
macro_rules! log_enabled {
    (target: $target:expr, $lvl:expr) => {{
        let lvl = $lvl;
        static LOG_ENABLED_METADATA: $crate::logs::LogMetadata = $crate::logs::LogMetadata {
            level: $lvl,
            level_filter: std::sync::atomic::AtomicU32::new($crate::logs::FILTER_LEVEL_UNSET_VALUE),
            fmt_str: "",
            target: $target,
            module_path: $crate::__log_module_path!(),
            file: $crate::__log_file!(),
            line: $crate::__log_line!(),
        };
        lvl <= $crate::levels::STATIC_MAX_LEVEL
            && lvl <= $crate::levels::max_level()
            && $crate::dispatch::log_enabled(&LOG_ENABLED_METADATA)
    }};
    ($lvl:expr) => {
        $crate::log_enabled!(target: $crate::__log_module_path!(), $lvl)
    };
}

//pub const fn type_name_of<T>(_: &T) -> &'static str {
//    //until type_name_of_val is out of nightly-only
//    std::any::type_name::<T>()
//}

#[doc(hidden)]
#[macro_export]
macro_rules! __function_name {
    () => {{
        // Okay, this is ugly, I get it. However, this is the best we can get on a stable rust.
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        // `3` is the length of the `::f`.
        &name[..name.len() - 3]
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __first_arg {
    ($first:tt) => {
        $first
    };
    ($first:tt, $($args:tt)*) => {
        $crate::__first_arg!($first)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_format_args {
    ($($args:tt)*) => {
        format_args!($($args)*)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_module_path {
    () => {
        module_path!()
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_file {
    () => {
        file!()
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_line {
    () => {
        line!()
    };
}
