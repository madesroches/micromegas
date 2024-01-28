use super::{Error, Result};

use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::Display,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

/// A type that is specialized in loading and parsing `OpenAPIv3` documents.
#[derive(Debug, Default)]
pub struct OpenApiLoader {
    raw_documents: RefCell<HashMap<OpenApiRefLocation, serde_json::Value>>,
}

impl OpenApiLoader {
    /// Load an `OpenAPI` document.
    ///
    /// # Errors
    ///
    /// Returns an error if the document could not be loaded.
    pub fn load_openapi(&self, ref_loc: OpenApiRefLocation) -> Result<OpenApi<'_>> {
        self.resolve_reference(ref_loc.into())
    }

    /// Resolve a reference, using the specified current context.
    ///
    /// # Errors
    ///
    /// Returns an error if the reference could not be resolved.
    pub fn resolve_reference<'s, T>(&'s self, ref_: OpenApiRef) -> Result<OpenApiElement<'s, T>>
    where
        T: for<'de> serde::Deserialize<'de> + Send + Sync + 'static,
    {
        let raw_reference = self.resolve_raw_reference(&ref_)?;
        let element: T = serde_json::from_value(raw_reference)?;

        Ok(OpenApiElement {
            loader: self,
            ref_,
            item: OpenApiElementItem::Owned(element),
        })
    }

    pub fn get_all_files(&self) -> Vec<PathBuf> {
        self.raw_documents
            .borrow()
            .keys()
            .map(OpenApiRefLocation::path)
            .cloned()
            .collect()
    }

    /// Import an element.
    ///
    /// # Errors
    ///
    /// Returns an error if the document could not be serialized or if a
    /// document already exists at the implicit location.
    #[cfg(test)]
    pub(crate) fn import<'e, E>(
        &'e self,
        file_name: impl Into<PathBuf>,
        element: &'e E,
    ) -> Result<OpenApiElement<'e, E>>
    where
        E: serde::Serialize,
    {
        let ref_location = OpenApiRefLocation::new(std::env::current_dir()?, file_name.into());
        let mut raw_documents = self.raw_documents.borrow_mut();

        if raw_documents.contains_key(&ref_location) {
            return Err(Error::DocumentAlreadyExists(ref_location));
        }

        let raw_document = serde_json::to_value(element)?;
        raw_documents.insert(ref_location.clone(), raw_document);

        Ok(OpenApiElement {
            loader: self,
            ref_: ref_location.into(),
            item: OpenApiElementItem::Ref(element),
        })
    }

    /// Import an element from its YAML representation.
    ///
    /// # Errors
    ///
    /// Returns an error if the document could not be serialized or if a
    /// document already exists at the implicit location.
    #[cfg(test)]
    pub(crate) fn import_from_yaml<E>(
        &self,
        file_name: impl Into<PathBuf>,
        element_yaml: &str,
    ) -> Result<OpenApiElement<'_, E>>
    where
        E: for<'de> serde::Deserialize<'de>,
    {
        let ref_location = OpenApiRefLocation::new(std::env::current_dir()?, file_name.into());
        let mut raw_documents = self.raw_documents.borrow_mut();

        if raw_documents.contains_key(&ref_location) {
            return Err(Error::DocumentAlreadyExists(ref_location));
        }

        let raw_document: serde_json::Value = serde_yaml::from_str(element_yaml)?;
        raw_documents.insert(ref_location.clone(), raw_document.clone());
        let element: E = serde_json::from_value(raw_document)?;

        Ok(OpenApiElement {
            loader: self,
            ref_: ref_location.into(),
            item: OpenApiElementItem::Owned(element),
        })
    }

    /// Load a document from a reference location.
    ///
    /// # Errors
    ///
    /// Returns an error if the document could not be loaded.
    fn load_raw_document(&self, ref_location: OpenApiRefLocation) -> Result<serde_json::Value> {
        let mut raw_documents = self.raw_documents.borrow_mut();

        Ok(match raw_documents.entry(ref_location) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let raw_document = v.key().load()?;

                v.insert(raw_document)
            }
        }
        .clone())
    }

    /// Resolve a reference, using the specified current context.
    ///
    /// # Errors
    ///
    /// Returns an error if the reference could not be resolved.
    fn resolve_raw_reference(&self, ref_: &OpenApiRef) -> Result<serde_json::Value> {
        let document = self.load_raw_document(ref_.ref_location.clone())?;
        document
            .pointer(&ref_.json_pointer.to_string())
            .cloned()
            .ok_or_else(|| Error::BrokenReference(ref_.clone()))
    }
}

/// Encapsulates a reference to an `OpenAPIv3` element, its location and the
/// associated loader.
#[derive(Debug, Clone)]
pub struct OpenApiElement<'e, E> {
    loader: &'e OpenApiLoader,
    ref_: OpenApiRef,
    item: OpenApiElementItem<'e, E>,
}

#[derive(Debug, Clone)]
enum OpenApiElementItem<'e, E> {
    Owned(E),
    Ref(&'e E),
}

impl<'e, E> AsRef<E> for OpenApiElementItem<'e, E> {
    fn as_ref(&self) -> &E {
        match self {
            OpenApiElementItem::Owned(e) => e,
            OpenApiElementItem::Ref(e) => e,
        }
    }
}

impl<'e, E> std::ops::Deref for OpenApiElementItem<'e, E>
where
    E: std::ops::Deref,
    E::Target: Sized,
{
    type Target = E::Target;

    fn deref(&self) -> &Self::Target {
        match self {
            OpenApiElementItem::Owned(e) => &**e,
            OpenApiElementItem::Ref(e) => *e,
        }
    }
}

impl<'e, E: 'static> OpenApiElement<'e, E> {
    /// Get the loader associated with this element.
    pub fn loader(&self) -> &'e OpenApiLoader {
        self.loader
    }

    /// Get the location of the `OpenAPI` document.
    pub fn ref_(&self) -> &OpenApiRef {
        &self.ref_
    }

    /// Resolve a reference-or, using the current element context.
    ///
    /// # Errors
    ///
    /// Returns an error if the reference could not be resolved.
    pub fn resolve_reference_or<T>(
        &'e self,
        sub_path: impl Into<JsonPointer>,
        ref_: &'e openapiv3::ReferenceOr<T>,
    ) -> Result<OpenApiElement<'e, T>>
    where
        T: for<'de> serde::Deserialize<'de> + Send + Sync + 'static,
    {
        match ref_ {
            openapiv3::ReferenceOr::Reference { reference } => {
                let ref_ = OpenApiRef::new(self.ref_.ref_location(), reference)?;
                self.loader.resolve_reference(ref_)
            }
            openapiv3::ReferenceOr::Item(item) => Ok(self.as_element_ref(sub_path, item)),
        }
    }

    pub fn as_self_element_ref<T>(&self, element: &'e T) -> OpenApiElement<'_, T> {
        self.as_element_ref(JsonPointer::default(), element)
    }

    /// Returns a value as an `OpenApiElement` that shares the same reference
    /// location as the current instance.
    pub fn as_element_ref<T>(
        &self,
        sub_path: impl Into<JsonPointer>,
        element: &'e T,
    ) -> OpenApiElement<'_, T> {
        OpenApiElement {
            loader: self.loader,
            ref_: self.ref_.join(sub_path),
            item: OpenApiElementItem::Ref(element),
        }
    }
}

impl<'e, E> AsRef<E> for OpenApiElement<'e, E> {
    fn as_ref(&self) -> &E {
        self.item.as_ref()
    }
}

impl<'e, E> std::ops::Deref for OpenApiElement<'e, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        self.item.as_ref()
    }
}

/// An helper type for encapsulated `OpenAPIv3` documents.
pub type OpenApi<'e> = OpenApiElement<'e, openapiv3::OpenAPI>;

/// An `OpenAPIv3` reference.
///
/// Supports local, remote and URL references.
///
/// Use `parse()` on a string type to build.
#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Clone)]
pub struct OpenApiRef {
    ref_location: OpenApiRefLocation,
    json_pointer: JsonPointer,
}

impl Display for OpenApiRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.ref_location, self.json_pointer)
    }
}

impl From<OpenApiRefLocation> for OpenApiRef {
    fn from(ref_location: OpenApiRefLocation) -> Self {
        Self {
            ref_location,
            json_pointer: JsonPointer::default(),
        }
    }
}

impl OpenApiRef {
    pub fn new(current_ref_loc: &OpenApiRefLocation, s: &str) -> Result<Self> {
        let parts = s.splitn(2, '#').collect::<Vec<_>>();

        if parts.len() != 2 {
            return Err(Error::InvalidOpenApiRef(s.to_string()));
        }

        if parts[0].is_empty() {
            Ok(Self {
                ref_location: current_ref_loc.clone(),
                json_pointer: parts[1].parse()?,
            })
        } else {
            Ok(Self {
                ref_location: OpenApiRefLocation::new(
                    current_ref_loc.path().parent().unwrap(),
                    parts[0].into(),
                ),
                json_pointer: parts[1].parse()?,
            })
        }
    }

    pub fn with_json_pointer(self, json_pointer: JsonPointer) -> Self {
        Self {
            json_pointer,
            ..self
        }
    }

    /// Returns the reference location.
    pub fn ref_location(&self) -> &OpenApiRefLocation {
        &self.ref_location
    }

    /// Get the file name of the reference.
    pub fn file_name(&self) -> Result<String> {
        self.ref_location.file_name()
    }

    /// Returns the JSON pointer.
    pub fn json_pointer(&self) -> &JsonPointer {
        &self.json_pointer
    }

    /// Returns the type name pointed to by the reference.
    pub fn type_name(&self) -> &str {
        self.json_pointer.type_name()
    }

    pub fn join(&self, sub_path: impl Into<JsonPointer>) -> OpenApiRef {
        OpenApiRef {
            ref_location: self.ref_location.clone(),
            json_pointer: self.json_pointer.join(sub_path),
        }
    }
}

/// An `OpenAPIv3` reference type.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub struct OpenApiRefLocation(PathBuf);

impl Display for OpenApiRefLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl OpenApiRefLocation {
    pub fn new(cwd: impl AsRef<Path>, path: PathBuf) -> Self {
        Self(normalize_path(if !path.is_absolute() {
            cwd.as_ref().join(path)
        } else {
            path
        }))
    }

    pub fn path(&self) -> &PathBuf {
        &self.0
    }

    pub fn file_name(&self) -> Result<String> {
        self.0
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .ok_or_else(|| Error::InvalidOpenApiRefLocation(self.0.display().to_string()))
    }

    /// Loads the document referenced by this reference.
    pub fn load(&self) -> Result<serde_json::Value> {
        let file = match std::fs::File::open(self.path()) {
            Ok(file) => file,
            Err(err) => match err.kind() {
                std::io::ErrorKind::NotFound => {
                    return Err(Error::BrokenReference(self.clone().into()))
                }
                _ => return Err(err.into()),
            },
        };
        serde_yaml::from_reader(file).map_err(Into::into)
    }
}

pub fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    let mut components = path.as_ref().components().peekable();
    let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().copied() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };

    for component in components {
        match component {
            Component::Prefix(..) => unreachable!(),
            Component::RootDir => {
                ret.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                ret.pop();
            }
            Component::Normal(c) => {
                ret.push(c);
            }
        }
    }
    ret
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JsonPointer {
    parts: Vec<String>,
}

impl Display for JsonPointer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.parts.is_empty() {
            write!(f, "")
        } else {
            write!(
                f,
                "/{}",
                self.parts
                    .iter()
                    .map(|s| s.replace('~', "~0").replace('/', "~1"))
                    .collect::<Vec<_>>()
                    .join("/")
            )
        }
    }
}

impl FromStr for JsonPointer {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(Self::default());
        }

        if !s.starts_with('/') {
            return Err(Error::InvalidJsonPointer(s.to_string()));
        }

        let parts = s
            .split('/')
            .filter_map(|s| {
                if s.is_empty() {
                    None
                } else {
                    Some(s.replace("~1", "/").replace("~0", "~"))
                }
            })
            .collect::<Vec<_>>();

        Ok(Self { parts })
    }
}

impl<T: AsRef<str>> From<[T; 1]> for JsonPointer {
    fn from(parts: [T; 1]) -> Self {
        Self {
            parts: parts.into_iter().map(|s| s.as_ref().to_string()).collect(),
        }
    }
}

impl<T: AsRef<str>> From<[T; 2]> for JsonPointer {
    fn from(parts: [T; 2]) -> Self {
        Self {
            parts: parts.into_iter().map(|s| s.as_ref().to_string()).collect(),
        }
    }
}

impl<T: AsRef<str>> From<[T; 3]> for JsonPointer {
    fn from(parts: [T; 3]) -> Self {
        Self {
            parts: parts.into_iter().map(|s| s.as_ref().to_string()).collect(),
        }
    }
}

impl JsonPointer {
    pub fn parts(&self) -> &[String] {
        &self.parts
    }

    pub fn type_name(&self) -> &str {
        self.parts.last().unwrap()
    }

    pub fn join(&self, other: impl Into<JsonPointer>) -> Self {
        let mut parts = self.parts.clone();
        parts.extend(other.into().parts);
        Self { parts }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openapi_loader_load_document() {
        let loader = OpenApiLoader::default();

        let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let ref_loc = OpenApiRefLocation::new(&cwd, "../../tests/api-codegen/cars.yaml".into());

        // Tests are always run from the crate's root.
        let _openapi = loader.load_openapi(ref_loc.clone()).unwrap();

        let ref_ = OpenApiRef::new(&ref_loc, "#/info/title").unwrap();
        let title: OpenApiElement<'_, String> = loader.resolve_reference(ref_).unwrap();

        assert_eq!(*title, "Test API");
    }

    #[test]
    fn test_openapi_ref_loc() {
        let cwd = PathBuf::from("/foo/bar");
        let ref_loc_a = OpenApiRefLocation::new(&cwd, "../a.yaml".into());
        let ref_loc_a2 = OpenApiRefLocation::new(&cwd, "/foo/a.yaml".into());
        let ref_loc_b = OpenApiRefLocation::new(&cwd, "../bar/b.yaml".into());
        let ref_loc_b2 = OpenApiRefLocation::new(&cwd, "b.yaml".into());

        assert_eq!(ref_loc_a, ref_loc_a2);
        assert_eq!(ref_loc_b, ref_loc_b2);
    }

    #[test]
    fn test_openapi_ref() {
        let cwd = PathBuf::from("/foo/bar");
        let ref_loc_a = OpenApiRefLocation::new(&cwd, "a.yaml".into());
        let ref_a = OpenApiRef::new(&ref_loc_a, "#/alpha/beta").unwrap();
        let ref_a2 = OpenApiRef::new(&ref_loc_a, "/foo/bar/a.yaml#/alpha/beta").unwrap();
        let ref_a3 = OpenApiRef::new(&ref_loc_a, "../bar/a.yaml#/alpha/beta").unwrap();

        assert_eq!(ref_a, ref_a2);
        assert_eq!(ref_a, ref_a3);
    }

    #[test]
    fn test_json_pointer() {
        let root: JsonPointer = "".parse().unwrap();
        assert!(&root.parts.is_empty());
        assert_eq!(root.to_string(), "");

        // Root pointers are coerced to an empty string.
        let root: JsonPointer = "/".parse().unwrap();
        assert!(&root.parts.is_empty());
        assert_eq!(root.to_string(), "");

        let a: JsonPointer = "/foo/bar".parse().unwrap();
        assert_eq!(a.parts(), &["foo", "bar"]);
        assert_eq!(a.to_string(), "/foo/bar");

        let b: JsonPointer = "/application~1json/~0lol~01".parse().unwrap();
        assert_eq!(b.parts(), &["application/json", "~lol~1"]);
        assert_eq!(b.to_string(), "/application~1json/~0lol~01");

        // JSON pointers must start with a /
        "invalid".parse::<JsonPointer>().unwrap_err();
    }
}
