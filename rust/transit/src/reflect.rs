use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Member {
    pub name: Arc<String>,
    pub type_name: String,
    pub offset: usize,
    pub size: usize,
    pub is_reference: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserDefinedType {
    pub name: Arc<String>,
    pub size: usize,
    pub members: Vec<Member>,
    pub is_reference: bool,
    #[serde(skip)]
    pub secondary_udts: Vec<UserDefinedType>, // udts of members
}

pub trait Reflect {
    fn reflect() -> UserDefinedType;
}
