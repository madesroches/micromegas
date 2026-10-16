//! Internal macros for tracing functionality (logging, spans, metrics)

#[macro_export]
macro_rules! static_span_desc {
    ($var_name:ident, $name:expr) => {
        static $var_name: $crate::spans::SpanMetadata = $crate::spans::SpanMetadata {
            name: $name,
            location: $crate::spans::SpanLocation {
                lod: $crate::levels::Verbosity::Max,
                target: module_path!(),
                module_path: module_path!(),
                file: file!(),
                line: line!(),
            },
        };
    };
}

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
    ($static_var_name:ident, $name:expr) => {
        $crate::static_span_desc!($static_var_name, $name);
        let guard_named = $crate::guards::ThreadSpanGuard::new(&$static_var_name);
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
    ($static_var_name:ident, $name:expr) => {
        static $static_var_name: $crate::spans::SpanLocation = $crate::spans::SpanLocation {
            lod: $crate::levels::Verbosity::Max,
            target: module_path!(),
            module_path: module_path!(),
            file: file!(),
            line: line!(),
        };
        let guard_named = $crate::guards::ThreadNamedSpanGuard::new(&$static_var_name, $name);
    };
    ($name:expr) => {
        $crate::span_scope_named!(_METADATA_NAMED, $name);
    };
}

/// Creates a static SpanLocation for use with named async spans.
/// This is an internal implementation detail - users should use `instrument_named!` instead.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # #[tokio::main]
/// # async fn main() {
/// // Don't use this directly - use instrument_named! instead
/// static_span_location!(LOCATION);
/// // Users should prefer: instrument_named!(future, "name")
/// # }
/// ```
#[macro_export]
macro_rules! static_span_location {
    ($var_name:ident) => {
        static $var_name: $crate::spans::SpanLocation = $crate::spans::SpanLocation {
            lod: $crate::levels::Verbosity::Max,
            target: module_path!(),
            module_path: module_path!(),
            file: file!(),
            line: line!(),
        };
    };
}

/// Instruments an async block with a named span using a static string.
/// This creates both the SpanLocation and instruments the future in one macro call.
///
/// # Examples
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # #[tokio::main]
/// # async fn main() {
/// let result = span_async_named!("database_query", async {
///     // async work here
///     42
/// }).await;
/// # }
/// ```
#[macro_export]
macro_rules! span_async_named {
    ($name:expr, $async_block:expr) => {{
        use $crate::spans::InstrumentFuture as _;
        $crate::static_span_location!(_ASYNC_LOCATION);
        $async_block.__instrument_named_internal(&_ASYNC_LOCATION, $name)
    }};
}

/// Instruments a future with a named span.
/// Usage: `instrument_named!(future, "span_name")`
///
/// # Examples
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # #[tokio::main]
/// # async fn main() {
/// let future = async { 42 };
/// let result = instrument_named!(future, "operation_name").await;
/// # }
/// ```
#[macro_export]
macro_rules! instrument_named {
    ($future:expr, $name:expr) => {{
        use $crate::spans::InstrumentFuture as _;
        $crate::static_span_location!(_INSTRUMENT_LOCATION);
        $future.__instrument_named_internal(&_INSTRUMENT_LOCATION, $name)
    }};
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
///
/// Properties go between the unit and the value, as `key = value` pairs. A key is an
/// identifier or a string literal. Literal values are interned once per callsite, unless a `..set`
/// is spread, in which case the literals are copied into the block (the spread set's own values
/// are not); any other value is attached to the measure and copied into the current block. A
/// bare value must implement [`PropertyValue`](crate::property_value::PropertyValue) (strings
/// do); use `%value` (`Display`) or `?value` (`Debug`) for the rest, at the cost of one `String`
/// allocation.
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # fn main() {
/// let correlation_id = String::from("c0ffee");
/// let rows = 3;
/// imetric!("rows_fetched", "count", correlation_id = correlation_id, region = "eu", rows = %rows, 3);
/// imetric!("rows_fetched", "count", "http.method" = "GET", 3);
/// # }
/// ```
///
/// A precomputed `&'static PropertySet` is spread with `..set`. It must come last in the
/// property list, and `key = value` pairs override its entries:
///
/// ```
/// use micromegas_tracing::prelude::*;
/// use micromegas_tracing::property_set::{Property, PropertySet};
///
/// # fn main() {
/// let set = PropertySet::find_or_create(vec![Property::new("region", "eu")]);
/// imetric!("rows_fetched", "count", ..set, 3);
/// imetric!("rows_fetched", "count", correlation_id = "c0ffee", ..set, 3);
/// # }
/// ```
///
/// `target: "name"` replaces the module path as the target of the metric:
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # fn main() {
/// imetric!(target: "my_crate::cache", "hits", "count", kind = "disk", 1);
/// # }
/// ```
///
/// `target`, `name` and `unit` are reserved keys, because analytics treats them as column
/// overrides. They can still be set through a `..set` spread:
///
/// ```compile_fail
/// use micromegas_tracing::prelude::*;
///
/// imetric!("rows_fetched", "count", unit = "rows", 3);
/// ```
///
/// A literal value goes through `concat!`, so `1_000` is recorded as "1000" and `0x10` as "16",
/// while a float keeps its spelling (`0.50` stays "0.50"); `%value` uses `Display` instead.
///
/// A bare integer needs `%`:
///
/// ```compile_fail
/// use micromegas_tracing::prelude::*;
///
/// let n = 3_u32;
/// imetric!("rows_fetched", "count", rows = n, 3);
/// ```
///
/// A bare negated expression has no `PropertyValue` impl either:
///
/// ```compile_fail
/// use micromegas_tracing::prelude::*;
///
/// let n = 3_i32;
/// imetric!("rows_fetched", "count", delta = -n, 3);
/// ```
///
/// `..set` can't be followed by other entries, and appears at most once:
///
/// ```compile_fail
/// use micromegas_tracing::prelude::*;
/// use micromegas_tracing::property_set::PropertySet;
///
/// let set = PropertySet::empty();
/// imetric!("rows_fetched", "count", ..set, kind = "a", 3);
/// ```
///
/// ```compile_fail
/// use micromegas_tracing::prelude::*;
/// use micromegas_tracing::property_set::PropertySet;
///
/// let set = PropertySet::empty();
/// imetric!("rows_fetched", "count", ..set, ..set, 3);
/// ```
#[macro_export]
macro_rules! imetric {
    (target: $target:expr, $name:expr, $unit:expr, $($rest:tt)+) => {
        $crate::__props!(@m metric
            [$name, $unit, $target, int_metric, tagged_integer_metric_dyn]
            [] [] [] [] $($rest)+)
    };
    ($name:expr, $unit:expr, $($rest:tt)+) => {
        $crate::__props!(@m metric
            [$name, $unit, module_path!(), int_metric, tagged_integer_metric_dyn]
            [] [] [] [] $($rest)+)
    };
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
///
/// Properties are `key = value` pairs between the unit and the value, as with [`imetric!`]:
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # fn main() {
/// let elapsed_ms = 12.5;
/// fmetric!("fetch_ms", "ms", "http.method" = "GET", elapsed_ms);
/// fmetric!("delta", "ms", -elapsed_ms);
/// # }
/// ```
#[macro_export]
macro_rules! fmetric {
    (target: $target:expr, $name:expr, $unit:expr, $($rest:tt)+) => {
        $crate::__props!(@m metric
            [$name, $unit, $target, float_metric, tagged_float_metric_dyn]
            [] [] [] [] $($rest)+)
    };
    ($name:expr, $unit:expr, $($rest:tt)+) => {
        $crate::__props!(@m metric
            [$name, $unit, module_path!(), float_metric, tagged_float_metric_dyn]
            [] [] [] [] $($rest)+)
    };
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
///
/// Properties go before the format string, as `key = value` pairs. A key is an identifier or a
/// string literal. A literal value is interned once per callsite, unless a `..set` is spread, in
/// which case the literals are copied into the block (the spread set's own values are not). Any
/// other value (a request or correlation id, for example) is copied into the current block, so
/// nothing leaks. A bare value
/// must implement [`PropertyValue`](crate::property_value::PropertyValue) (strings do); use
/// `%value` (`Display`) or `?value` (`Debug`) for the rest, at the cost of one `String`
/// allocation. Dynamic values are only evaluated when the entry is enabled.
///
/// ```
/// use micromegas_tracing::prelude::*;
///
/// # fn main() {
/// let correlation_id = String::from("c0ffee");
/// let rows = 3;
/// info!(correlation_id = correlation_id, region = "eu", rows = %rows, "fetched {} rows", rows);
/// info!(target: "app_events", "http.method" = "GET", "request done");
/// # }
/// ```
///
/// A precomputed `&'static PropertySet` is spread with `..set`. It must come last in the
/// property list, and `key = value` pairs override its entries:
///
/// ```
/// use micromegas_tracing::prelude::*;
/// use micromegas_tracing::property_set::{Property, PropertySet};
///
/// # fn main() {
/// let set = PropertySet::find_or_create(vec![Property::new("region", "eu")]);
/// info!(..set, "from a set");
/// info!(correlation_id = "c0ffee", ..set, "from a set and a pair");
/// # }
/// ```
///
/// `level` and `target` are reserved keys, because analytics treats them as column overrides.
/// They can still be set through a `..set` spread:
///
/// ```compile_fail
/// use micromegas_tracing::prelude::*;
///
/// info!(level = "debug", "message");
/// ```
///
/// A literal value goes through `concat!`, so `1_000` is recorded as "1000" and `0x10` as "16",
/// while a float keeps its spelling (`0.50` stays "0.50"); `%value` uses `Display` instead.
///
/// A bare integer needs `%`:
///
/// ```compile_fail
/// use micromegas_tracing::prelude::*;
///
/// let n = 3_u32;
/// info!(rows = n, "fetched");
/// ```
///
/// A bare negated expression has no `PropertyValue` impl either:
///
/// ```compile_fail
/// use micromegas_tracing::prelude::*;
///
/// let n = 3_i32;
/// info!(delta = -n, "fetched");
/// ```
///
/// `..set` can't be followed by other entries, and appears at most once:
///
/// ```compile_fail
/// use micromegas_tracing::prelude::*;
/// use micromegas_tracing::property_set::PropertySet;
///
/// let set = PropertySet::empty();
/// info!(..set, kind = "a", "message");
/// ```
///
/// ```compile_fail
/// use micromegas_tracing::prelude::*;
/// use micromegas_tracing::property_set::PropertySet;
///
/// let set = PropertySet::empty();
/// info!(..set, ..set, "message");
/// ```
#[macro_export]
macro_rules! log {
    (target: $target:expr, $lvl:expr, $($rest:tt)+) => (
        $crate::__props!(@m log [$target, $lvl] [] [] [] [] $($rest)+)
    );
    ($lvl:expr, $($rest:tt)+) => (
        $crate::log!(target: $crate::__log_module_path!(), $lvl, $($rest)+)
    );
}

/// Muncher behind the property lists of `log!`, `imetric!` and `fmetric!`.
///
/// Accumulators, in order: literal pairs `(key, (value))` for the per-callsite interned set,
/// dynamic entries `(key, kind, value)`, every entry in source order (used when a `..set` is
/// spread, so that the last pair wins), and the spread set.
#[doc(hidden)]
#[macro_export]
macro_rules! __props {
    // `..set` is only accepted as the last property.
    (@m $kind:ident $ctx:tt $s:tt $d:tt $a:tt $set:tt
        .. $new_set:expr, $k:tt = $($x:tt)*) => {
        compile_error!("`..set` must be the last entry of the property list")
    };
    (@m $kind:ident $ctx:tt $s:tt $d:tt $a:tt $set:tt
        .. $new_set:expr, .. $($x:tt)*) => {
        compile_error!("`..set` can only appear once, as the last entry of the property list")
    };
    (@m $kind:ident $ctx:tt $s:tt $d:tt $a:tt $set:tt
        .. $new_set:expr, $($rest:tt)+) => {
        $crate::__props!(@end $kind $ctx $s $d $a [$new_set] $($rest)+)
    };
    // The negative literal arm precedes the `-` fallback, which precedes the plain literal arm:
    // `$v:literal` is a hard parse error on `-x`.
    (@m $kind:ident $ctx:tt [$($s:tt)*] [$($d:tt)*] [$($a:tt)*] $set:tt
        $k:tt = - $v:literal $(, $($rest:tt)*)?) => {
        $crate::__props!(@m $kind $ctx
            [$($s)* ($k, (concat!("-", $v)))] [$($d)*]
            [$($a)* ($k, lit, (concat!("-", $v)))] $set $($($rest)*)?)
    };
    (@m $kind:ident $ctx:tt $s:tt $d:tt $a:tt $set:tt
        $k:tt = - $($rest:tt)*) => {
        $crate::__props!(@e $kind $ctx $s $d $a $set $k = - $($rest)*)
    };
    (@m $kind:ident $ctx:tt [$($s:tt)*] [$($d:tt)*] [$($a:tt)*] $set:tt
        $k:tt = % $v:expr $(, $($rest:tt)*)?) => {
        $crate::__props!(@m $kind $ctx
            [$($s)*] [$($d)* ($k, disp, $v)] [$($a)* ($k, disp, $v)] $set $($($rest)*)?)
    };
    (@m $kind:ident $ctx:tt [$($s:tt)*] [$($d:tt)*] [$($a:tt)*] $set:tt
        $k:tt = ? $v:expr $(, $($rest:tt)*)?) => {
        $crate::__props!(@m $kind $ctx
            [$($s)*] [$($d)* ($k, dbg, $v)] [$($a)* ($k, dbg, $v)] $set $($($rest)*)?)
    };
    (@m $kind:ident $ctx:tt [$($s:tt)*] [$($d:tt)*] [$($a:tt)*] $set:tt
        $k:tt = $v:literal $(, $($rest:tt)*)?) => {
        $crate::__props!(@m $kind $ctx
            [$($s)* ($k, (concat!($v)))] [$($d)*]
            [$($a)* ($k, lit, (concat!($v)))] $set $($($rest)*)?)
    };
    (@m $kind:ident $ctx:tt $s:tt $d:tt $a:tt $set:tt
        $k:tt = $($rest:tt)+) => {
        $crate::__props!(@e $kind $ctx $s $d $a $set $k = $($rest)+)
    };
    // A bare value
    (@e $kind:ident $ctx:tt [$($s:tt)*] [$($d:tt)*] [$($a:tt)*] $set:tt
        $k:tt = $v:expr $(, $($rest:tt)*)?) => {
        $crate::__props!(@m $kind $ctx
            [$($s)*] [$($d)* ($k, bare, $v)] [$($a)* ($k, bare, $v)] $set $($($rest)*)?)
    };
    // The end of the property list: a format string for logs, a value for metrics.
    (@m log $ctx:tt $s:tt $d:tt $a:tt $set:tt) => {
        compile_error!("log macros require a format string")
    };
    (@m log $ctx:tt $s:tt $d:tt $a:tt $set:tt $($fmt:tt)+) => {
        $crate::__props!(@end log $ctx $s $d $a $set $($fmt)+)
    };
    (@m metric $ctx:tt $s:tt $d:tt $a:tt $set:tt $value:expr $(,)?) => {
        $crate::__props!(@end metric $ctx $s $d $a $set $value)
    };

    // The cheapest call that carries the properties: no properties at all, a spread set only,
    // the per-callsite literal set only, or any of those plus dynamic values.
    (@end log $ctx:tt [] [] [] [] $($fmt:tt)+) => {
        $crate::__log_call!(plain $ctx [$($fmt)+])
    };
    (@end log $ctx:tt [$($s:tt)*] [] [$($a:tt)*] [$set:tt] $($fmt:tt)+) => {
        $crate::__log_call!(nogate $ctx [$($fmt)+] $set, $crate::__dyn_slice!(log $($a)*))
    };
    (@end log $ctx:tt [$($s:tt)*] [$($d:tt)+] [$($a:tt)*] [$set:tt] $($fmt:tt)+) => {
        $crate::__log_call!(gate $ctx [$($fmt)+] $set, $crate::__dyn_slice!(log $($a)*))
    };
    (@end log $ctx:tt [$($s:tt)+] [] [$($a:tt)*] [] $($fmt:tt)+) => {
        $crate::__log_call!(nogate $ctx [$($fmt)+]
            $crate::__callsite_set!(log $($s)+), $crate::__dyn_slice!(log))
    };
    (@end log $ctx:tt [$($s:tt)+] [$($d:tt)+] [$($a:tt)*] [] $($fmt:tt)+) => {
        $crate::__log_call!(gate $ctx [$($fmt)+]
            $crate::__callsite_set!(log $($s)+), $crate::__dyn_slice!(log $($d)+))
    };
    (@end log $ctx:tt [] [$($d:tt)+] [$($a:tt)*] [] $($fmt:tt)+) => {
        $crate::__log_call!(gate $ctx [$($fmt)+]
            $crate::property_set::PropertySet::empty(), $crate::__dyn_slice!(log $($d)+))
    };

    (@end metric $ctx:tt [] [] [] [] $value:expr) => {
        $crate::__metric_call!(plain $ctx $value)
    };
    (@end metric $ctx:tt [$($s:tt)*] [$($d:tt)*] [$($a:tt)*] [$set:tt] $value:expr $(,)?) => {
        $crate::__metric_call!(tagged $ctx
            $set, $crate::__dyn_slice!(metric $($a)*), $value)
    };
    (@end metric $ctx:tt [$($s:tt)+] [$($d:tt)*] [$($a:tt)*] [] $value:expr) => {
        $crate::__metric_call!(tagged $ctx
            $crate::__callsite_set!(metric $($s)+), $crate::__dyn_slice!(metric $($d)*), $value)
    };
    (@end metric $ctx:tt [] [$($d:tt)+] [$($a:tt)*] [] $value:expr) => {
        $crate::__metric_call!(tagged $ctx
            $crate::property_set::PropertySet::empty(), $crate::__dyn_slice!(metric $($d)+), $value)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_call {
    (plain [$target:expr, $lvl:expr] [$($fmt:tt)+]) => ({
        static LOG_DESC: $crate::logs::LogMetadata = $crate::logs::LogMetadata {
            level: $lvl,
            level_filter: std::sync::atomic::AtomicU32::new($crate::logs::FILTER_LEVEL_UNSET_VALUE),
            fmt_str: $crate::__first_arg!($($fmt)+),
            target: $target,
            module_path: $crate::__log_module_path!(),
            file: file!(),
            line: line!(),
        };
        if $lvl <= $crate::levels::STATIC_MAX_LEVEL && $lvl <= $crate::levels::max_level() {
            $crate::dispatch::log(&LOG_DESC, format_args!($($fmt)+));
        }
    });
    (nogate [$target:expr, $lvl:expr] [$($fmt:tt)+] $set:expr, $slice:expr) => ({
        static LOG_DESC: $crate::logs::LogMetadata = $crate::logs::LogMetadata {
            level: $lvl,
            level_filter: std::sync::atomic::AtomicU32::new($crate::logs::FILTER_LEVEL_UNSET_VALUE),
            fmt_str: $crate::__first_arg!($($fmt)+),
            target: $target,
            module_path: $crate::__log_module_path!(),
            file: file!(),
            line: line!(),
        };
        if $lvl <= $crate::levels::STATIC_MAX_LEVEL && $lvl <= $crate::levels::max_level() {
            $crate::dispatch::log_dyn_tagged(&LOG_DESC, $set, $slice, format_args!($($fmt)+));
        }
    });
    // Evaluating a dynamic value can allocate, so only do it for an enabled entry.
    (gate [$target:expr, $lvl:expr] [$($fmt:tt)+] $set:expr, $slice:expr) => ({
        static LOG_DESC: $crate::logs::LogMetadata = $crate::logs::LogMetadata {
            level: $lvl,
            level_filter: std::sync::atomic::AtomicU32::new($crate::logs::FILTER_LEVEL_UNSET_VALUE),
            fmt_str: $crate::__first_arg!($($fmt)+),
            target: $target,
            module_path: $crate::__log_module_path!(),
            file: file!(),
            line: line!(),
        };
        if $lvl <= $crate::levels::STATIC_MAX_LEVEL && $lvl <= $crate::levels::max_level() {
            if let Some(sink) = $crate::dispatch::enabled_log_sink(&LOG_DESC) {
                $crate::dispatch::log_dyn_tagged_enabled(
                    sink,
                    &LOG_DESC,
                    $set,
                    $slice,
                    format_args!($($fmt)+),
                );
            }
        }
    });
}

#[doc(hidden)]
#[macro_export]
macro_rules! __metric_call {
    (plain [$name:expr, $unit:expr, $target:expr, $plain:ident, $dynf:ident] $value:expr) => {{
        static METRIC_METADATA: $crate::metrics::StaticMetricMetadata =
            $crate::metrics::StaticMetricMetadata {
                lod: $crate::levels::Verbosity::Max,
                name: $name,
                unit: $unit,
                target: $target,
                file: file!(),
                line: line!(),
            };
        $crate::dispatch::$plain(&METRIC_METADATA, $value);
    }};
    (tagged [$name:expr, $unit:expr, $target:expr, $plain:ident, $dynf:ident]
        $set:expr, $slice:expr, $value:expr) => {{
        static METRIC_METADATA: $crate::metrics::StaticMetricMetadata =
            $crate::metrics::StaticMetricMetadata {
                lod: $crate::levels::Verbosity::Max,
                name: $name,
                unit: $unit,
                target: $target,
                file: file!(),
                line: line!(),
            };
        $crate::dispatch::$dynf(&METRIC_METADATA, $set, $slice, $value);
    }};
}

/// Interns the literal properties of a callsite on its first execution.
#[doc(hidden)]
#[macro_export]
macro_rules! __callsite_set {
    ($kind:ident $(($k:tt, ($($v:tt)*)))+) => {{
        static CELL: ::std::sync::OnceLock<&'static $crate::property_set::PropertySet> =
            ::std::sync::OnceLock::new();
        static PAIRS: &[(&str, &str)] = &[$(($crate::__prop_key!($kind $k), $($v)*)),+];
        $crate::property_set::callsite_set(&CELL, PAIRS)
    }};
}

// The temporaries created for `bare`, `disp` and `dbg` values live until the end of the
// statement that makes the dispatch call, which is what lets the slice borrow them.
#[doc(hidden)]
#[macro_export]
macro_rules! __dyn_slice {
    ($kind:ident $(($k:tt, $vk:ident, $v:tt))*) => {
        &[$($crate::__dyn_prop!($kind $k $vk $v)),*]
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __dyn_prop {
    ($kind:ident $k:tt lit $v:tt) => {
        $crate::property_set::DynProperty {
            name: $crate::__prop_key!($kind $k),
            value: $v,
        }
    };
    ($kind:ident $k:tt bare $v:tt) => {
        $crate::property_set::DynProperty {
            name: $crate::__prop_key!($kind $k),
            value: &*$crate::property_value::PropertyValue::property_value(&$v),
        }
    };
    ($kind:ident $k:tt disp $v:tt) => {
        $crate::property_set::DynProperty {
            name: $crate::__prop_key!($kind $k),
            value: &*::std::format!("{}", $v),
        }
    };
    ($kind:ident $k:tt dbg $v:tt) => {
        $crate::property_set::DynProperty {
            name: $crate::__prop_key!($kind $k),
            value: &*::std::format!("{:?}", $v),
        }
    };
}

/// Turns a property key into a `&'static str`. Analytics treats some names as column
/// overrides, so the macros reject them; a `..set` spread or a runtime `DynProperty` slice is
/// the way to set them.
#[doc(hidden)]
#[macro_export]
macro_rules! __prop_key {
    (log level) => {
        compile_error!(
            "`level` is reserved: the level is chosen by the macro (`info!`, `log!(Level::..)`)"
        )
    };
    (log "level") => {
        compile_error!(
            "`level` is reserved: the level is chosen by the macro (`info!`, `log!(Level::..)`)"
        )
    };
    (log target) => {
        compile_error!("`target` is reserved: pass it as `target: ..` before the other arguments")
    };
    (log "target") => {
        compile_error!("`target` is reserved: pass it as `target: ..` before the other arguments")
    };
    (metric target) => {
        compile_error!("`target` is reserved: pass it as `target: ..` before the metric name")
    };
    (metric "target") => {
        compile_error!("`target` is reserved: pass it as `target: ..` before the metric name")
    };
    (metric name) => {
        compile_error!("`name` is reserved: it is the first argument of the metric")
    };
    (metric "name") => {
        compile_error!("`name` is reserved: it is the first argument of the metric")
    };
    (metric unit) => {
        compile_error!("`unit` is reserved: it is the second argument of the metric")
    };
    (metric "unit") => {
        compile_error!("`unit` is reserved: it is the second argument of the metric")
    };
    ($kind:ident $k:ident) => {
        stringify!($k)
    };
    ($kind:ident $k:literal) => {
        $k
    };
    ($kind:ident $($k:tt)*) => {
        compile_error!("a property key must be an identifier or a string literal")
    };
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

/// Logs a message representing a crash or panic
#[macro_export]
macro_rules! fatal {
    (target: $target:expr, $($arg:tt)+) => (
        $crate::log!(target: $target, $crate::levels::Level::Fatal, $($arg)+)
    );
    ($($arg:tt)+) => (
        $crate::log!($crate::levels::Level::Fatal, $($arg)+)
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
