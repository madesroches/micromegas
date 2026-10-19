//! Expansion of the log macros: binds `{key}` placeholders to the call's properties and picks
//! how the message is stored (see `LogMsgForm`).

use proc_macro2::{Delimiter, Ident, Span, TokenStream, TokenTree};
use quote::{format_ident, quote};
use syn::{Lit, LitStr};

/// How a placeholder's argument is spelled and what follows the colon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spec {
    Plain,
    Debug,
    Other,
}

#[derive(Debug, PartialEq, Eq)]
enum Arg {
    Named(String),
    /// `{}`, `{0}` or anything that is not an identifier.
    Other,
}

#[derive(Debug, PartialEq, Eq)]
struct Placeholder {
    arg: Arg,
    spec: Spec,
}

fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_alphanumeric()) && syn::parse_str::<Ident>(name).is_ok()
}

/// The placeholders of a format string, or `None` if it is malformed (rustc then reports it).
fn scan_placeholders(fmt: &str) -> Option<Vec<Placeholder>> {
    let mut out = vec![];
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    continue;
                }
                let mut inner = String::new();
                loop {
                    match chars.next()? {
                        '}' => break,
                        '{' => return None,
                        c => inner.push(c),
                    }
                }
                let (arg, spec) = match inner.split_once(':') {
                    None => (inner.as_str(), Spec::Plain),
                    Some((arg, "?")) => (arg, Spec::Debug),
                    Some((arg, _)) => (arg, Spec::Other),
                };
                let arg = if is_ident(arg) {
                    Arg::Named(arg.to_owned())
                } else {
                    Arg::Other
                };
                out.push(Placeholder { arg, spec });
            }
            '}' => match chars.next()? {
                '}' => {}
                _ => return None,
            },
            _ => {}
        }
    }
    Some(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Lit,
    Stat,
    Disp,
    Dbg,
    Bare,
}

struct Entry {
    key: TokenTree,
    kind: Kind,
    /// Everything after the kind: `(concat!(..))` for a literal, the expression otherwise.
    value: TokenStream,
}

impl Entry {
    /// The text a placeholder has to spell to refer to this entry. Raw identifiers can't be
    /// placeholders.
    fn key_text(&self) -> Option<String> {
        match &self.key {
            TokenTree::Ident(id) => {
                let text = id.to_string();
                (!text.starts_with("r#")).then_some(text)
            }
            TokenTree::Literal(lit) => {
                match syn::parse2::<Lit>(TokenStream::from(TokenTree::Literal(lit.clone()))) {
                    Ok(Lit::Str(s)) => Some(s.value()),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// Inlines the invisible groups that `macro_rules!` fragments are wrapped in.
fn flatten(ts: TokenStream) -> Vec<TokenTree> {
    let mut out = vec![];
    for tt in ts {
        match tt {
            TokenTree::Group(g) if g.delimiter() == Delimiter::None => {
                out.extend(flatten(g.stream()));
            }
            tt => out.push(tt),
        }
    }
    out
}

fn is_punct(tt: &TokenTree, c: char) -> bool {
    matches!(tt, TokenTree::Punct(p) if p.as_char() == c)
}

fn bracket_contents(tt: Option<TokenTree>, what: &str) -> syn::Result<TokenStream> {
    match tt {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket => Ok(g.stream()),
        Some(other) => Err(syn::Error::new(
            other.span(),
            format!("__log_impl: expected `[..]` for {what}"),
        )),
        None => Err(syn::Error::new(
            Span::call_site(),
            format!("__log_impl: missing {what}"),
        )),
    }
}

fn parse_entries(stream: TokenStream) -> syn::Result<Vec<Entry>> {
    let mut entries = vec![];
    for tt in stream {
        let TokenTree::Group(g) = tt else {
            return Err(syn::Error::new(tt.span(), "__log_impl: expected an entry"));
        };
        let mut it = g.stream().into_iter();
        let key = it
            .next()
            .ok_or_else(|| syn::Error::new(g.span(), "__log_impl: empty entry"))?;
        it.next();
        let kind = match it.next() {
            Some(TokenTree::Ident(id)) => match id.to_string().as_str() {
                "lit" => Kind::Lit,
                "stat" => Kind::Stat,
                "disp" => Kind::Disp,
                "dbg" => Kind::Dbg,
                "bare" => Kind::Bare,
                other => {
                    return Err(syn::Error::new(
                        id.span(),
                        format!("__log_impl: unknown entry kind `{other}`"),
                    ));
                }
            },
            _ => return Err(syn::Error::new(g.span(), "__log_impl: missing entry kind")),
        };
        it.next();
        entries.push(Entry {
            key,
            kind,
            value: it.collect(),
        });
    }
    Ok(entries)
}

#[derive(Debug, PartialEq, Eq)]
enum LitClass {
    /// The stored property string equals the literal's `Display`.
    Deferrable,
    /// A valid expression, but `concat!` can spell it differently than `Display` does.
    BoundOnly,
    Unbound,
}

struct LitInfo {
    class: LitClass,
    /// The original literal, with its minus sign.
    expr: TokenStream,
}

fn int_range_ok(lit: &syn::LitInt, negated: bool) -> bool {
    let (bits, signed) = match lit.suffix() {
        "" | "i32" => (32, true),
        "i8" => (8, true),
        "i16" => (16, true),
        "i64" | "isize" => (64, true),
        "i128" => (128, true),
        "u8" => (8, false),
        "u16" => (16, false),
        "u32" => (32, false),
        "u64" | "usize" => (64, false),
        "u128" => (128, false),
        _ => return false,
    };
    let Ok(value) = lit.base10_parse::<u128>() else {
        return false;
    };
    match (signed, negated) {
        (false, true) => false,
        (false, false) => bits == 128 || value < (1u128 << bits),
        (true, true) => value <= 1u128 << (bits - 1),
        (true, false) => value < (1u128 << (bits - 1)),
    }
}

/// Reads the literal back out of the `(concat!(..))` wrapper the muncher builds.
fn lit_info(value: &TokenStream) -> LitInfo {
    let unbound = LitInfo {
        class: LitClass::Unbound,
        expr: TokenStream::new(),
    };
    let outer = flatten(value.clone());
    let Some(TokenTree::Group(call)) = outer.last() else {
        return unbound;
    };
    let call = flatten(call.stream());
    let Some(TokenTree::Group(args)) = call.last() else {
        return unbound;
    };
    let args = flatten(args.stream());
    let (negated, lit_tt) = match args.as_slice() {
        [lit] => (false, lit),
        [_, comma, lit] if is_punct(comma, ',') => (true, lit),
        _ => return unbound,
    };
    let Ok(lit) = syn::parse2::<Lit>(TokenStream::from(lit_tt.clone())) else {
        return unbound;
    };
    let class = match (&lit, negated) {
        (Lit::Str(_) | Lit::Char(_) | Lit::Bool(_), false) => LitClass::Deferrable,
        (Lit::Int(i), negated) if int_range_ok(i, negated) => {
            if negated {
                LitClass::BoundOnly
            } else {
                LitClass::Deferrable
            }
        }
        (Lit::Float(_), _) => LitClass::BoundOnly,
        _ => LitClass::Unbound,
    };
    let expr = if negated {
        quote!(-#lit_tt)
    } else {
        quote!(#lit_tt)
    };
    LitInfo { class, expr }
}

/// The tokens after the format string.
struct Args {
    /// Whether there is anything but a trailing comma.
    extra: bool,
    named: Vec<String>,
}

fn parse_args(rest: &[TokenTree]) -> Option<Args> {
    let mut tokens = rest.to_vec();
    if tokens.last().is_some_and(|t| is_punct(t, ',')) {
        tokens.pop();
    }
    if tokens.is_empty() {
        return Some(Args {
            extra: false,
            named: vec![],
        });
    }
    if !is_punct(&tokens[0], ',') {
        return None;
    }
    let mut named = vec![];
    for segment in tokens.split(|t| is_punct(t, ',')) {
        if let [TokenTree::Ident(name), TokenTree::Punct(eq), ..] = segment
            && eq.as_char() == '='
            && eq.spacing() == proc_macro2::Spacing::Alone
        {
            named.push(name.to_string());
        }
    }
    Some(Args { extra: true, named })
}

/// A value of a bound placeholder: what the format string argument evaluates.
struct Binding {
    name: String,
    value: TokenStream,
}

struct Analysis {
    form: Form,
    bindings: Vec<Binding>,
}

#[derive(Clone, Copy)]
enum Form {
    Static,
    Template,
    Formatted,
}

/// How an entry's value reaches the message.
fn entry_binding(entry: &Entry, hoisted: &Ident) -> Option<(TokenStream, bool)> {
    match entry.kind {
        Kind::Lit => {
            let info = lit_info(&entry.value);
            match info.class {
                LitClass::Unbound => None,
                LitClass::BoundOnly => Some((info.expr, false)),
                LitClass::Deferrable => Some((info.expr, true)),
            }
        }
        Kind::Stat | Kind::Disp | Kind::Bare => Some((quote!(#hoisted), true)),
        Kind::Dbg => Some((quote!(#hoisted), true)),
    }
}

fn analyze(
    fmt: &[TokenTree],
    entries: &[(&Entry, Ident)],
    has_properties: bool,
) -> syn::Result<Analysis> {
    let formatted = Analysis {
        form: Form::Formatted,
        bindings: vec![],
    };
    let Some(first) = fmt.first() else {
        return Ok(formatted);
    };
    let Ok(lit) = syn::parse2::<LitStr>(
        flatten(TokenStream::from(first.clone()))
            .into_iter()
            .collect(),
    ) else {
        return Ok(formatted);
    };
    let Some(args) = parse_args(&fmt[1..]) else {
        return Ok(formatted);
    };
    let text = lit.value();
    if !args.extra && !text.contains(['{', '}']) {
        return Ok(Analysis {
            form: Form::Static,
            bindings: vec![],
        });
    }
    let Some(placeholders) = scan_placeholders(&text) else {
        return Ok(formatted);
    };

    let mut bindings: Vec<Binding> = vec![];
    let mut deferrable = has_properties && !args.extra;
    for ph in &placeholders {
        let Arg::Named(name) = &ph.arg else {
            deferrable = false;
            continue;
        };
        if args.named.contains(name) {
            deferrable = false;
            continue;
        }
        let matches: Vec<&(&Entry, Ident)> = entries
            .iter()
            .filter(|(e, _)| e.key_text().is_some_and(|k| k.eq_ignore_ascii_case(name)))
            .collect();
        if matches.len() > 1 {
            return Err(syn::Error::new(
                lit.span(),
                format!(
                    "the placeholder `{{{name}}}` matches more than one property: property \
                     names are case-insensitive, so it would be ambiguous"
                ),
            ));
        }
        let bound = match matches.first() {
            Some((entry, hoisted)) if entry.key_text().as_deref() == Some(name.as_str()) => {
                entry_binding(entry, hoisted).map(|(expr, pairs)| (*entry, expr, pairs))
            }
            _ => None,
        };
        let Some((entry, expr, pairs_with_display)) = bound else {
            deferrable = false;
            continue;
        };
        let fixed = matches!(entry.kind, Kind::Lit | Kind::Stat);
        if fixed && ph.spec != Spec::Plain {
            return Err(syn::Error::new(
                lit.span(),
                format!(
                    "the placeholder `{{{name}}}` has a format spec, but `{name}` is a literal or \
                     `static` property, which is recorded once and never formatted per call; \
                     bind the value to a differently named local and format that \
                     (`let label = ..; \"{{label:?}}\"`)"
                ),
            ));
        }
        let renders_as_stored = match (entry.kind, ph.spec) {
            (Kind::Dbg, Spec::Debug) => true,
            (Kind::Dbg, _) => false,
            (_, Spec::Plain) => pairs_with_display,
            _ => false,
        };
        if !renders_as_stored {
            deferrable = false;
        }
        if !bindings.iter().any(|b| &b.name == name) {
            bindings.push(Binding {
                name: name.clone(),
                value: expr,
            });
        }
    }
    Ok(Analysis {
        form: if deferrable {
            Form::Template
        } else {
            Form::Formatted
        },
        bindings,
    })
}

struct Input {
    krate: TokenTree,
    target: TokenStream,
    lvl: TokenStream,
    statics: Vec<Entry>,
    dyns: Vec<Entry>,
    all: Vec<Entry>,
    set: TokenStream,
    has_static: bool,
    fmt: Vec<TokenTree>,
}

fn parse_input(input: TokenStream) -> syn::Result<Input> {
    let mut it = input.into_iter();
    let krate = it
        .next()
        .ok_or_else(|| syn::Error::new(Span::call_site(), "__log_impl: missing `$crate`"))?;
    let ctx = bracket_contents(it.next(), "the context")?;
    let mut target = TokenStream::new();
    let mut lvl = TokenStream::new();
    let mut in_target = true;
    for tt in ctx {
        if in_target && is_punct(&tt, ',') {
            in_target = false;
        } else if in_target {
            target.extend([tt]);
        } else {
            lvl.extend([tt]);
        }
    }
    let statics = parse_entries(bracket_contents(it.next(), "the literal entries")?)?;
    let dyns = parse_entries(bracket_contents(it.next(), "the dynamic entries")?)?;
    let all = parse_entries(bracket_contents(it.next(), "the ordered entries")?)?;
    let set = bracket_contents(it.next(), "the set")?;
    let has_static = !bracket_contents(it.next(), "the static marker")?.is_empty();
    Ok(Input {
        krate,
        target,
        lvl,
        statics,
        dyns,
        all,
        set,
        has_static,
        fmt: it.collect(),
    })
}

pub fn expand(input: TokenStream) -> TokenStream {
    match try_expand(input) {
        Ok(ts) => ts,
        Err(e) => e.to_compile_error(),
    }
}

fn try_expand(input: TokenStream) -> syn::Result<TokenStream> {
    let Input {
        krate,
        target,
        lvl,
        statics,
        dyns,
        all,
        set,
        has_static,
        fmt,
    } = parse_input(input)?;
    let hidden =
        |prefix: &str, i: usize| Ident::new(&format!("__mm_{prefix}_{i}"), Span::mixed_site());

    // Every non-spread entry, with the local that holds its value when it has one.
    let mut keyed: Vec<(&Entry, Ident)> = vec![];
    for (i, e) in statics.iter().chain(dyns.iter()).enumerate() {
        keyed.push((e, hidden("v", i)));
    }
    let has_properties = !keyed.is_empty() || !set.is_empty();
    let analysis = analyze(&fmt, &keyed, has_properties)?;
    let form_ident = match analysis.form {
        Form::Static => format_ident!("Static"),
        Form::Template => format_ident!("Template"),
        Form::Formatted => format_ident!("Formatted"),
    };

    let fmt_tokens = &fmt;
    let first_arg = quote!(#krate::__first_arg!(#(#fmt_tokens)*));
    let mut forwarded: Vec<TokenTree> = {
        let mut tokens = fmt.clone();
        if tokens.last().is_some_and(|t| is_punct(t, ',')) {
            tokens.pop();
        }
        tokens
    };
    for b in &analysis.bindings {
        let name = Ident::new(&b.name, Span::call_site());
        let value = &b.value;
        let extra = quote!(, #name = #value);
        forwarded.extend(extra);
    }
    let format_args = quote!(::std::format_args!(#(#forwarded)*));

    let desc = quote! {
        static LOG_DESC: #krate::logs::LogMetadata = #krate::logs::LogMetadata {
            level: #lvl,
            level_filter: ::std::sync::atomic::AtomicU32::new(
                #krate::logs::FILTER_LEVEL_UNSET_VALUE,
            ),
            fmt_str: #first_arg,
            target: #target,
            module_path: #krate::__log_module_path!(),
            file: file!(),
            line: line!(),
        };
    };
    let enabled = quote! {
        #lvl <= #krate::levels::STATIC_MAX_LEVEL && #lvl <= #krate::levels::max_level()
    };
    let msg_form = quote!(#krate::logs::LogMsgForm::#form_ident);

    if !has_properties {
        return Ok(quote! {{
            #desc
            if #enabled {
                #krate::dispatch::log(&LOG_DESC, #msg_form, #format_args);
            }
        }});
    }

    // The hoisted locals are evaluated once, for an enabled entry only, and feed both the
    // stored property and the message. Dynamic values are matched on as one scrutinee so
    // temporaries they borrow from (`s().as_str()`) live until the dispatch call finishes.
    let mut hoists = TokenStream::new();
    let mut scrutinee: Vec<TokenStream> = vec![];
    let mut patterns: Vec<Ident> = vec![];
    let mut renders = TokenStream::new();
    let mut dyn_props: Vec<TokenStream> = vec![];
    for (i, e) in dyns.iter().enumerate() {
        let v = &keyed[statics.len() + i].1;
        let p = hidden("p", i);
        let value = &e.value;
        let key = &e.key;
        match e.kind {
            Kind::Disp => renders.extend(quote! {
                let #p = ::std::format!("{}", #v);
            }),
            Kind::Dbg => renders.extend(quote! {
                let #p = ::std::format!("{:?}", #v);
            }),
            _ => renders.extend(quote! {
                let #p = #krate::property_value::PropertyValue::property_value(&#v);
            }),
        }
        scrutinee.push(quote!(&(#value)));
        patterns.push(v.clone());
        dyn_props.push(quote! {
            #krate::property_set::DynProperty::new(#krate::__prop_key!(log #key), &*#p)
        });
    }

    let lit_prop = |e: &Entry| {
        let key = &e.key;
        let value = &e.value;
        quote!(#krate::__dyn_prop!(log #key lit #value))
    };
    // `..set` spread: every non-static entry, in source order, so the last pair wins.
    let ordered_slice = |all: &[Entry]| {
        let mut next_dyn = 0;
        let mut props = vec![];
        for e in all {
            if e.kind == Kind::Lit {
                props.push(lit_prop(e));
            } else {
                props.push(dyn_props[next_dyn].clone());
                next_dyn += 1;
            }
        }
        props
    };

    let (set_expr, slice, gate) = if has_static {
        let mut pairs = TokenStream::new();
        for (i, e) in statics.iter().enumerate() {
            let key = &e.key;
            let value = &e.value;
            match e.kind {
                Kind::Stat => {
                    let v = &keyed[i].1;
                    hoists.extend(quote! {
                        let #v = #krate::property_set::static_value(#value);
                    });
                    pairs.extend(quote!((#key, stat, (#v))));
                }
                _ => pairs.extend(quote!((#key, lit, #value))),
            }
        }
        let set_arg = if set.is_empty() {
            quote!([])
        } else {
            quote!([#set])
        };
        (
            quote!(#krate::__static_set!(log #set_arg #pairs)),
            dyn_props.clone(),
            true,
        )
    } else if !set.is_empty() {
        (set.clone(), ordered_slice(&all), !dyns.is_empty())
    } else if !statics.is_empty() {
        let mut pairs = TokenStream::new();
        for e in &statics {
            let key = &e.key;
            let value = &e.value;
            pairs.extend(quote!((#key, lit, #value)));
        }
        (
            quote!(#krate::__callsite_set!(log #pairs)),
            dyn_props.clone(),
            !dyns.is_empty(),
        )
    } else {
        (
            quote!(#krate::property_set::PropertySet::empty()),
            dyn_props.clone(),
            true,
        )
    };

    let body = if gate {
        let sink = Ident::new("__mm_sink", Span::mixed_site());
        quote! {
            if let Some(#sink) = #krate::dispatch::enabled_log_sink(&LOG_DESC) {
                #hoists
                match (#(#scrutinee,)*) {
                    (#(#patterns,)*) => {
                        #renders
                        #krate::dispatch::log_dyn_tagged_enabled(
                            #sink,
                            &LOG_DESC,
                            #set_expr,
                            &[#(#slice),*],
                            #msg_form,
                            #format_args,
                        );
                    }
                }
            }
        }
    } else {
        quote! {
            #krate::dispatch::log_dyn_tagged(
                &LOG_DESC,
                #set_expr,
                &[#(#slice),*],
                #msg_form,
                #format_args,
            );
        }
    };
    Ok(quote! {{
        #desc
        if #enabled {
            #body
        }
    }})
}
