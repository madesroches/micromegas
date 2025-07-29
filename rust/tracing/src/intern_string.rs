//! Store dynamically-created strings in a global container.
//! Strings are never released from the container.
use internment::Intern;

pub fn intern_string(input: &str) -> &'static str {
    Intern::<String>::from_ref(input).as_ref()
}
