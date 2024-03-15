use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Member {
    pub name: String,
    pub type_name: String,
    pub offset: usize,
    pub size: usize,
    pub is_reference: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserDefinedType {
    pub name: String,
    pub size: usize,
    pub members: Vec<Member>,
    pub is_reference: bool,
    pub secondary_udts: Vec<UserDefinedType>, // udts of members
}

pub trait Reflect {
    fn reflect() -> UserDefinedType;
}
