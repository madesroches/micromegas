use micromegas_analytics::log_msg_template::render_msg_template;
use micromegas_transit::value::{Object, Value};

fn render(fmt_str: &str, members: &[(&str, &str)]) -> String {
    let members: Vec<(&str, Value)> = members
        .iter()
        .map(|(k, v)| (*k, Value::String(v)))
        .collect();
    let properties = Object {
        type_name: "PropertySet",
        members: &members,
    };
    render_msg_template(fmt_str, &properties)
}

#[test]
fn text_and_escapes() {
    assert_eq!(render("plain text", &[]), "plain text");
    assert_eq!(render("", &[]), "");
    assert_eq!(render("a {{b}} c", &[]), "a {b} c");
    assert_eq!(render("{{}}", &[]), "{}");
    assert_eq!(render("{{{user}}}", &[("user", "x")]), "{x}");
}

#[test]
fn placeholders() {
    let props = [("user", "alice"), ("zone", "eu")];
    assert_eq!(render("login for {user}", &props), "login for alice");
    assert_eq!(render("{user} in {zone}", &props), "alice in eu");
    assert_eq!(render("{user}{zone}", &props), "aliceeu");
    assert_eq!(render("{user}", &props), "alice");
    assert_eq!(render("{user} {user}", &props), "alice alice");
}

#[test]
fn debug_placeholders_use_the_stored_string() {
    assert_eq!(render("{v:?}", &[("v", "[1, 2]")]), "[1, 2]");
    assert_eq!(render("{v:?} {v}", &[("v", "\"x\"")]), "\"x\" \"x\"");
}

#[test]
fn a_missing_property_is_kept_verbatim() {
    assert_eq!(render("hi {who}", &[("user", "alice")]), "hi {who}");
    assert_eq!(render("hi {who:?}", &[]), "hi {who:?}");
}

#[test]
fn an_unsupported_placeholder_is_kept_verbatim() {
    let props = [("n", "3")];
    assert_eq!(render("{} {0} {n:>8}", &props), "{} {0} {n:>8}");
    assert_eq!(render("{n:.2} {n}", &props), "{n:.2} 3");
    assert_eq!(render("{ n }", &props), "{ n }");
    assert_eq!(render("dangling { brace", &props), "dangling { brace");
    assert_eq!(render("dangling } brace", &props), "dangling } brace");
}

#[test]
fn names_are_case_sensitive() {
    assert_eq!(render("{User}", &[("user", "alice")]), "{User}");
}

#[test]
fn a_property_that_is_not_a_string_is_kept_verbatim() {
    let members = [("n", Value::U32(3))];
    let properties = Object {
        type_name: "PropertySet",
        members: &members,
    };
    assert_eq!(render_msg_template("{n}", &properties), "{n}");
}
