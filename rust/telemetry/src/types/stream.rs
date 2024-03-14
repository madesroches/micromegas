use std::collections::BTreeMap;

#[derive(Clone, PartialEq)]
pub struct Stream {
    pub stream_id: String,
    pub process_id: String,
    pub dependencies_metadata: Option<ContainerMetadata>,
    pub objects_metadata: Option<ContainerMetadata>,
    pub tags: Vec<String>,
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, PartialEq)]
pub struct ContainerMetadata {
    pub types: Vec<UserDefinedType>,
}

#[derive(Clone, PartialEq)]
pub struct UserDefinedType {
    pub name: String,
    pub size: u32,
    pub members: Vec<UdtMember>,
    pub is_reference: bool,
}

#[derive(Clone, PartialEq)]
pub struct UdtMember {
    pub name: String,
    pub type_name: String,
    pub offset: u32,
    pub size: u32,
    pub is_reference: bool,
}
