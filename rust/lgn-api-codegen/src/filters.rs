use convert_case::{Case, Casing};

#[allow(clippy::unnecessary_wraps)]
pub fn snake_case<T: std::fmt::Display>(s: T) -> ::askama::Result<String> {
    Ok(s.to_string().to_case(Case::Snake))
}

#[allow(clippy::unnecessary_wraps)]
pub fn camel_case<T: std::fmt::Display>(s: T) -> ::askama::Result<String> {
    Ok(s.to_string().to_case(Case::Camel))
}

#[allow(clippy::unnecessary_wraps)]
pub fn pascal_case<T: std::fmt::Display>(s: T) -> ::askama::Result<String> {
    Ok(s.to_string().to_case(Case::Pascal))
}

#[allow(clippy::unnecessary_wraps)]
pub fn multilinecomment<T: std::fmt::Display>(s: T) -> ::askama::Result<String> {
    let s = s.to_string();
    Ok(s.replace('\n', "\n// "))
}
