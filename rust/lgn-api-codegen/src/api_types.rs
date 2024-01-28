use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    iter::Chain,
    path::PathBuf,
    slice::Iter,
    str::FromStr,
};

use crate::{
    openapi_loader::{JsonPointer, OpenApiRef, OpenApiRefLocation},
    Error, Result,
};

/// Apis is the top level container for all APIs and shared models.
#[derive(Debug, PartialEq)]
pub struct GenerationContext<Options = ()> {
    pub root: PathBuf,
    pub location_contexts: BTreeMap<OpenApiRefLocation, LocationContext>,
    pub options: Options,
}

impl GenerationContext<()> {
    pub fn with_options<Options>(self, options: Options) -> GenerationContext<Options> {
        GenerationContext {
            root: self.root,
            location_contexts: self.location_contexts,
            options,
        }
    }
}

impl GenerationContext<RustOptions> {
    pub fn ref_loc_to_rust_module_path(&self, ref_loc: &OpenApiRefLocation) -> Result<ModulePath> {
        let file_path = ref_loc.path();

        Ok(
            if let Some(module_path) = self.options.module_mappings.get(file_path) {
                module_path.clone()
            } else {
                let file_path = file_path.strip_prefix(&self.root).map_err(|_err| {
                    Error::DocumentOutOfRoot {
                        document_path: file_path.clone(),
                        root: self.root.clone(),
                    }
                })?;

                ModulePath {
                    absolute: false,
                    parts: file_path
                        .with_extension("")
                        .display()
                        .to_string()
                        .split('/')
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                }
            },
        )
    }

    pub fn as_local_rust_modules(&self) -> Result<BTreeMap<ModulePath, &LocationContext>> {
        self.location_contexts
            .iter()
            .filter_map(
                |(ref_loc, api_ctx)| match self.ref_loc_to_rust_module_path(ref_loc) {
                    Ok(module_path) => {
                        if module_path.is_relative() {
                            Some(Ok((module_path, api_ctx)))
                        } else {
                            None
                        }
                    }
                    Err(err) => Some(Err(err)),
                },
            )
            .collect()
    }

    pub fn as_local_rust_modules_with_apis(
        &self,
    ) -> Result<BTreeMap<ModulePath, &LocationContext>> {
        Ok(self
            .as_local_rust_modules()?
            .into_iter()
            .filter(|(_, location_ctx)| location_ctx.api.is_some())
            .collect())
    }
}

impl GenerationContext<TypeScriptOptions> {
    pub fn ref_loc_to_typescript_module_path(
        &self,
        ref_loc: &OpenApiRefLocation,
    ) -> Result<ModulePath> {
        let file_path = ref_loc.path();

        if let Some(module_path) = self.options.alias_mappings.0.get(file_path) {
            return Ok(module_path.clone());
        }

        let file_path =
            file_path
                .strip_prefix(&self.root)
                .map_err(|_err| Error::DocumentOutOfRoot {
                    document_path: file_path.clone(),
                    root: self.root.clone(),
                })?;

        Ok(ModulePath {
            absolute: false,
            parts: file_path
                .with_extension("")
                .to_string_lossy()
                .split('/')
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        })
    }

    pub fn as_typescript_modules(&self) -> Result<BTreeMap<ModulePath, &LocationContext>> {
        self.location_contexts
            .iter()
            .map(
                |(ref_loc, api_ctx)| match self.ref_loc_to_typescript_module_path(ref_loc) {
                    Ok(module_path) => Ok((module_path, api_ctx)),
                    Err(err) => Err(err),
                },
            )
            .collect()
    }
}

impl<Options: Default> GenerationContext<Options> {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            location_contexts: BTreeMap::new(),
            options: Options::default(),
        }
    }

    pub fn ref_loc_to_module_path(&self, ref_loc: &OpenApiRefLocation) -> Result<ModulePath> {
        let file_path = ref_loc.path();

        let file_path =
            file_path
                .strip_prefix(&self.root)
                .map_err(|_err| Error::DocumentOutOfRoot {
                    document_path: file_path.clone(),
                    root: self.root.clone(),
                })?;

        Ok(ModulePath {
            absolute: false,
            parts: file_path
                .with_extension("")
                .display()
                .to_string()
                .split('/')
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        })
    }

    /// Returns the location context as modules relative to the root.
    pub fn as_modules(&self) -> Result<BTreeMap<ModulePath, &LocationContext>> {
        self.location_contexts
            .iter()
            .map(|(ref_loc, api_ctx)| {
                let module_path = self.ref_loc_to_module_path(ref_loc)?;

                Ok((module_path, api_ctx))
            })
            .collect()
    }

    pub fn get_model(&self, ref_: &OpenApiRef) -> Result<&Model> {
        let location_context = self
            .location_contexts
            .get(ref_.ref_location())
            .ok_or_else(|| Error::BrokenReference(ref_.clone()))?;

        location_context
            .models
            .get(ref_.json_pointer())
            .ok_or_else(|| Error::BrokenReference(ref_.clone()))
    }
}

#[derive(Debug, Clone, Default)]
pub struct RustOptions {
    /// A mapping of files to Rust modules.
    pub module_mappings: HashMap<PathBuf, ModulePath>,
}

impl RustOptions {
    /// Adds a mapping of a file to a Rust module.
    ///
    /// # Errors
    ///
    /// If the paths are invalid, an error is returned.
    pub fn add_module_mapping(
        &mut self,
        file_path: impl AsRef<std::path::Path>,
        module_path: &str,
    ) -> Result<Option<ModulePath>> {
        let file_path = file_path.as_ref();

        Ok(self.module_mappings.insert(
            if file_path.is_relative() {
                file_path.to_path_buf()
            } else {
                std::env::current_dir()?.join(file_path)
            }
            .canonicalize()?,
            ModulePath::from_absolute_rust_module_path(module_path),
        ))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TypeScriptAliasMappings(HashMap<PathBuf, ModulePath>);

impl TypeScriptAliasMappings {
    /// Adds a mapping of a file to a TypeScript namespace.
    ///
    /// # Errors
    ///
    /// If the paths are invalid, an error is returned.
    pub fn add_module_mapping(
        &mut self,
        file_path: impl AsRef<std::path::Path>,
        module_path: &str,
    ) -> Result<Option<ModulePath>> {
        let file_path = file_path.as_ref();

        Ok(self.0.insert(
            if file_path.is_relative() {
                file_path.to_path_buf()
            } else {
                std::env::current_dir()?.join(file_path)
            }
            .canonicalize()?,
            ModulePath::from_absolute_typescript_namespace(module_path),
        ))
    }
}

impl<K, V> TryFrom<HashMap<K, V>> for TypeScriptAliasMappings
where
    K: AsRef<std::path::Path>,
    V: AsRef<str>,
{
    type Error = Error;

    fn try_from(map: HashMap<K, V>) -> Result<Self> {
        let mut alias_mappings = Self::default();

        for (k, v) in map {
            alias_mappings.add_module_mapping(k, v.as_ref())?;
        }

        Ok(alias_mappings)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TypeScriptOptions {
    pub prettier_config_path: Option<PathBuf>,
    pub with_package_json: bool,
    pub skip_format: bool,
    pub alias_mappings: TypeScriptAliasMappings,
    pub filename: String,
}

#[derive(Debug, Clone)]
pub enum Language {
    Rust(RustOptions),
    TypeScript(TypeScriptOptions),
    Python,
}

impl Language {
    pub(crate) fn generate(
        self,
        ctx: GenerationContext,
        output_dir: &std::path::Path,
    ) -> Result<()> {
        match self {
            Language::Rust(options) => Self::generate_rust(ctx, options, output_dir),
            Language::TypeScript(options) => Self::generate_typescript(ctx, options, output_dir),
            Language::Python => Self::generate_python(ctx, output_dir),
        }
    }
}

/// Represents a module path, agnostic of the language.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModulePath {
    absolute: bool,
    parts: Vec<String>,
}

impl Display for ModulePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.absolute {
            write!(f, "/{}", self.parts.join("/"))
        } else {
            write!(f, "{}", self.parts.join("/"))
        }
    }
}

impl FromStr for ModulePath {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let absolute = s.starts_with('/');

        let parts = if absolute { &s[1..] } else { s }
            .split('/')
            .filter_map(|s| {
                if s.is_empty() {
                    None
                } else {
                    Some(s.to_string())
                }
            })
            .collect();

        Ok(Self { absolute, parts })
    }
}

#[allow(clippy::fallible_impl_from)]
impl<'a> From<&'a str> for ModulePath {
    fn from(s: &'a str) -> Self {
        Self::from_str(s).unwrap()
    }
}

#[allow(clippy::fallible_impl_from)]
impl From<String> for ModulePath {
    fn from(s: String) -> Self {
        Self::from_str(&s).unwrap()
    }
}

impl ModulePath {
    pub fn from_absolute_rust_module_path(s: &str) -> Self {
        Self {
            absolute: true,
            parts: s
                .split("::")
                .filter_map(|s| {
                    if s.is_empty() {
                        None
                    } else {
                        Some(s.to_string())
                    }
                })
                .collect(),
        }
    }

    pub fn from_absolute_typescript_namespace(s: &str) -> Self {
        Self {
            absolute: true,
            parts: s
                .split('.')
                .filter_map(|s| {
                    if s.is_empty() {
                        None
                    } else {
                        Some(s.to_string())
                    }
                })
                .collect(),
        }
    }

    pub fn to_rust_module_path(&self) -> String {
        self.parts()
            .iter()
            .map(|s| (if s == ".." { "super" } else { s }).to_string())
            .collect::<Vec<String>>()
            .join("::")
    }

    #[inline]
    pub fn is_absolute(&self) -> bool {
        self.absolute
    }

    #[inline]
    pub fn is_relative(&self) -> bool {
        !self.absolute
    }

    #[inline]
    pub fn parts(&self) -> &[String] {
        &self.parts
    }

    /// Join this module path with another.
    ///
    /// If the other module path is absolute, this path is ignored.
    #[must_use]
    pub fn join(&self, module_path: impl Into<ModulePath>) -> Self {
        let module_path = module_path.into();

        if module_path.is_absolute() {
            return module_path;
        }

        let mut parts = self.parts.clone();
        parts.extend(module_path.parts);

        Self {
            absolute: self.absolute,
            parts,
        }
    }

    /// Return the parent of this module path.
    pub fn parent(&self) -> Option<ModulePath> {
        if self.parts.is_empty() {
            None
        } else {
            Some(Self {
                absolute: self.absolute,
                parts: self.parts[..self.parts.len() - 1].to_vec(),
            })
        }
    }

    /// Return a module path such `self.join(module_path) == other`.
    ///
    /// As a special case, if only one of the paths is absolute, it will be
    /// returned as is.
    #[must_use]
    pub fn relative_to(&self, other: &ModulePath) -> Self {
        // We can only ever compare absolute paths and non-absolute paths with
        // one another.
        if self.absolute != other.absolute {
            return if self.absolute {
                self.clone()
            } else {
                other.clone()
            };
        }

        for (i, part) in self.parts.iter().enumerate() {
            if i >= other.parts.len() {
                // The other is a prefix of us: just strip the beginnning.
                return Self {
                    absolute: false,
                    parts: self.parts[i..].to_vec(),
                };
            }

            if part != &other.parts[i] {
                let mut parts = other.parts[i..]
                    .iter()
                    .map(|_| "..".to_string())
                    .collect::<Vec<_>>();

                parts.extend(self.parts[i..].iter().cloned());

                return Self {
                    absolute: false,
                    parts,
                };
            }
        }

        Self {
            absolute: false,
            parts: other.parts[self.parts.len()..]
                .iter()
                .map(|_| "..".to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct LocationContext {
    pub api: Option<Api>,
    pub models: BTreeMap<JsonPointer, Model>,
}

/// API is the resolved type that is fed to templates and contains helper
/// methods to ease their writing.
#[derive(Debug, Default, PartialEq)]
pub struct Api {
    pub title: String,
    pub description: Option<String>,
    pub version: String,
    pub paths: BTreeMap<Path, Vec<Route>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Type {
    Any,
    Int32,
    Int64,
    UInt32,
    UInt64,
    String,
    Boolean,
    Float32,
    Float64,
    DateTime,
    Date,
    Bytes,
    Binary,
    Array(Box<Self>),
    HashSet(Box<Self>),
    Map(Box<Self>),
    Named(OpenApiRef),
    Enum {
        variants: Vec<String>,
    },
    Struct {
        fields: BTreeMap<String, Field>,
        map: Option<Box<Self>>,
    },
    OneOf {
        types: Vec<Self>,
    },
    Box(Box<Self>),
}

impl Type {
    pub fn requires_model(&self) -> bool {
        matches!(
            self,
            Type::Enum { .. } | Type::Struct { .. } | Type::OneOf { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelOrigin {
    /// The model is defined as a schema.
    Schemas,
    /// The model is defined as a property of an object.
    ObjectProperty { object_pointer: JsonPointer },
    /// The model is auto-generated from a request body type.
    RequestBody { operation_name: String },
    /// The model is auto-generated from a response body type.
    ResponseBody {
        operation_name: String,
        status_code: StatusCode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Model {
    pub ref_: OpenApiRef,
    pub description: Option<String>,
    pub origin: ModelOrigin,
    pub type_: Type,
}

impl Model {
    pub fn to_named_type(&self) -> Type {
        Type::Named(self.ref_.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Field {
    pub name: String,
    pub description: Option<String>,
    pub type_: Type,
    pub required: bool,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub struct Path(pub String);

impl From<&str> for Path {
    fn from(path: &str) -> Self {
        Self(path.to_string())
    }
}

#[derive(Debug, PartialEq)]
pub struct Route {
    pub name: String,
    pub method: Method,
    pub summary: Option<String>,
    pub request_body: Option<RequestBody>,
    pub parameters: Parameters,
    pub responses: BTreeMap<StatusCode, Response>,
}

impl Route {
    pub fn has_empty_request(&self) -> bool {
        self.request_body.is_none() && self.parameters.is_empty()
    }

    pub fn has_no_responses_content(&self) -> bool {
        for resp in self.responses.values() {
            if resp.content.is_some() {
                return false;
            }
        }

        true
    }
}

#[derive(Debug, PartialEq, Default)]
pub struct Parameters {
    pub path: Vec<Parameter>,
    pub query: Vec<Parameter>,
    pub header: Vec<Parameter>,
    pub cookie: Vec<Parameter>,
}

impl Parameters {
    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
            && self.query.is_empty()
            && self.header.is_empty()
            && self.cookie.is_empty()
    }
}

impl<'a> IntoIterator for &'a Parameters {
    type Item = &'a Parameter;
    type IntoIter = Chain<
        Chain<Chain<Iter<'a, Parameter>, Iter<'a, Parameter>>, Iter<'a, Parameter>>,
        Iter<'a, Parameter>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.path
            .iter()
            .chain(self.query.iter())
            .chain(self.header.iter())
            .chain(self.cookie.iter())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatusCode(http::StatusCode);

impl From<http::StatusCode> for StatusCode {
    fn from(status_code: http::StatusCode) -> Self {
        Self(status_code)
    }
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format!("{}", self.0.as_u16()))
    }
}

impl StatusCode {
    pub(crate) fn as_u16(&self) -> u16 {
        self.0.as_u16()
    }
}

#[derive(Debug, PartialEq)]
pub struct Response {
    pub description: String,
    pub content: Option<Content>,
    pub headers: BTreeMap<String, Header>,
}

#[derive(Debug, PartialEq)]
pub struct Content {
    pub media_type: MediaType,
    pub type_: Type,
}

#[derive(Debug, PartialEq)]
pub struct Header {
    pub description: Option<String>,
    pub type_: Type,
}

#[derive(Debug, PartialEq)]
pub enum MediaType {
    Bytes,
    Json,
}

impl FromStr for MediaType {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "application/octet-stream" => Ok(Self::Bytes),
            "application/json" => Ok(Self::Json),
            _ => Err(Error::UnsupportedMediaType(s.to_string())),
        }
    }
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Bytes => "application/octet-stream",
            Self::Json => "application/json",
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum Method {
    Get,
    Post,
    Delete,
    Put,
    Patch,
    Head,
    Options,
    Trace,
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Delete => "DELETE",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Trace => "TRACE",
        })
    }
}

impl std::str::FromStr for Method {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_uppercase().as_str() {
            "GET" => Ok(Self::Get),
            "POST" => Ok(Self::Post),
            "DELETE" => Ok(Self::Delete),
            "PUT" => Ok(Self::Put),
            "PATCH" => Ok(Self::Patch),
            "HEAD" => Ok(Self::Head),
            "OPTIONS" => Ok(Self::Options),
            "TRACE" => Ok(Self::Trace),
            _ => Err(Error::UnsupportedMethod(s.to_string())),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct RequestBody {
    pub description: Option<String>,
    pub required: bool,
    pub content: Content,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub description: Option<String>,
    pub type_: Type,
    pub required: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_path() {
        assert_eq!(
            ModulePath {
                absolute: false,
                parts: vec!["foo".to_string(), "bar".to_string()]
            },
            "foo/bar".parse().unwrap()
        );
        assert_eq!(
            ModulePath {
                absolute: true,
                parts: vec!["foo".to_string(), "bar".to_string()]
            },
            "/foo/bar".parse().unwrap()
        );
    }

    #[test]
    fn test_module_path_relative_to() {
        let current: ModulePath = "foo/bar/baz".parse().unwrap();
        let other: ModulePath = "foo/bar".parse().unwrap();
        let abs: ModulePath = "/i/am/absolute".parse().unwrap();

        assert_eq!(other.relative_to(&current), "..".parse().unwrap());
        assert_eq!(current.relative_to(&other), "baz".parse().unwrap());
        assert_eq!(other.relative_to(&abs), "/i/am/absolute".parse().unwrap());
        assert_eq!(abs.relative_to(&other), "/i/am/absolute".parse().unwrap());
        assert_eq!(other.relative_to(&other), "".parse().unwrap());
        assert_eq!(abs.relative_to(&abs), "".parse().unwrap());
    }
}
