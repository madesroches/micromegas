use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use indexmap::IndexMap;
use openapiv3::AdditionalProperties;

use super::api_types::{
    Api, Field, GenerationContext, Method, Model, Parameter, RequestBody, Response, Route,
    StatusCode, Type,
};
use crate::{
    api_types::{Content, Header, ModelOrigin, Parameters},
    openapi_loader::OpenApiRef,
    Error, OpenApi, OpenApiElement, Result,
};

#[derive(Debug)]
pub struct Visitor {
    ctx: GenerationContext,
}

impl Visitor {
    pub fn new(root: PathBuf) -> Self {
        Self {
            ctx: GenerationContext::new(root),
        }
    }

    pub fn visit(mut self, openapis: &[OpenApi<'_>]) -> Result<GenerationContext> {
        for openapi in openapis {
            let api = self.visit_openapi(openapi)?;

            self.ctx
                .location_contexts
                .entry(openapi.ref_().ref_location().clone())
                .or_default()
                .api = Some(api);
        }

        Ok(self.ctx)
    }

    fn visit_openapi(&mut self, openapi: &OpenApi<'_>) -> Result<Api> {
        let mut api = Api {
            title: openapi.info.title.clone(),
            description: openapi.info.description.clone(),
            version: openapi.info.version.clone(),
            paths: BTreeMap::new(),
        };

        // Let's first resolve and register schemas.
        if openapi.components.is_some() {
            for (name, schema_ref) in &openapi.components.as_ref().unwrap().schemas {
                let schema =
                    openapi.resolve_reference_or(["components", "schemas", name], schema_ref)?;
                self.register_model_from_schema(ModelOrigin::Schemas, &schema, HashSet::new())?;
            }
        }

        // Then resolve paths.
        for (path, path_item_ref) in &openapi.paths.paths {
            let path_item = openapi.resolve_reference_or(["paths", path], path_item_ref)?;

            let mut routes = Vec::new();

            for (method, operation) in path_item.iter() {
                let http_method: Method = method.parse()?;
                let route = self.visit_operation(
                    &path_item,
                    http_method,
                    &path_item.as_element_ref([method], operation),
                )?;

                routes.push(route);
            }

            api.paths.insert(path.as_str().into(), routes);
        }

        Ok(api)
    }

    fn visit_operation(
        &mut self,
        path_item: &OpenApiElement<'_, openapiv3::PathItem>,
        method: Method,
        operation: &OpenApiElement<'_, openapiv3::Operation>,
    ) -> Result<Route> {
        if operation.security.is_some() {
            return Err(Error::Unsupported(
                operation.ref_().join(["security"]),
                "security specifiers".to_string(),
            ));
        }

        // We enforce an operation id for now.
        let operation_name = match &operation.operation_id {
            Some(name) => name,
            None => {
                return Err(Error::MissingOperationID(path_item.ref_().clone()));
            }
        };

        // Visit parameters.
        let mut parameters = Parameters::default();

        // Let's iterate over all parameters, in order, with the global path
        // parameters first and remove duplicates.
        let raw_parameters = path_item
            .parameters
            .iter()
            .map(|parameter_ref| {
                path_item
                    .resolve_reference_or(["parameters"], parameter_ref)
                    .map(|parameter| (parameter.parameter_data_ref().name.clone(), parameter))
            })
            .chain(operation.parameters.iter().map(|parameter_ref| {
                operation
                    .resolve_reference_or(["parameters"], parameter_ref)
                    .map(|parameter| (parameter.parameter_data_ref().name.clone(), parameter))
            }))
            .collect::<Result<IndexMap<_, _>>>()?;

        for parameter in raw_parameters.into_values() {
            match parameter.as_ref() {
                openapiv3::Parameter::Path { parameter_data, .. } => {
                    parameters.path.push(self.visit_parameter(
                        &parameter.as_element_ref([&parameter_data.name], parameter_data),
                        None,
                    )?);
                }
                openapiv3::Parameter::Query { parameter_data, .. } => {
                    parameters.query.push(self.visit_parameter(
                        &parameter.as_element_ref([&parameter_data.name], parameter_data),
                        None,
                    )?);
                }
                openapiv3::Parameter::Header { parameter_data, .. } => {
                    let allowed_types = Some(vec![
                        Type::String,
                        Type::Int32,
                        Type::Int64,
                        Type::Boolean,
                        Type::Float32,
                        Type::Float64,
                        Type::Bytes,
                    ]);

                    if http::header::HeaderName::from_lowercase(parameter_data.name.as_bytes())
                        .is_err()
                    {
                        return Err(Error::InvalidHeaderName(parameter_data.name.clone()));
                    }

                    parameters.header.push(self.visit_parameter(
                        &parameter.as_element_ref([&parameter_data.name], parameter_data),
                        allowed_types,
                    )?);
                }
                // We don't support cookie parameters for now.
                openapiv3::Parameter::Cookie { parameter_data, .. } => {
                    return Err(Error::Unsupported(
                        parameter.ref_().join([&parameter_data.name]),
                        "cookie parameters".to_string(),
                    ));
                }
            };
        }

        // Visit request body.
        let request_body = match &operation.request_body {
            Some(request_body) => {
                let request_body = operation.resolve_reference_or(["requestBody"], request_body)?;

                let (type_, media_type) = match request_body.content.len() {
                    0 => {
                        return Err(Error::Invalid(
                            request_body.ref_().clone(),
                            "requests with no media-types".to_string(),
                        ))
                    }
                    1 => {
                        let (media_type, media_type_data) =
                            request_body.content.iter().next().unwrap();

                        let type_ = match &media_type_data.schema {
                            Some(schema_ref) => self.resolve_type_ref(
                                ModelOrigin::RequestBody {
                                    operation_name: operation_name.clone(),
                                },
                                &request_body
                                    .as_element_ref(["content", media_type, "schema"], schema_ref),
                                HashSet::new(),
                            )?,
                            None => {
                                return Err(Error::Invalid(
                                    request_body.ref_().join(["content", media_type]),
                                    "no schema".to_string(),
                                ))
                            }
                        };

                        (type_, media_type)
                    }
                    _ => {
                        return Err(Error::Invalid(
                            request_body.ref_().join(["content"]),
                            "requests with multiple media-types".to_string(),
                        ))
                    }
                };

                Some(RequestBody {
                    description: request_body.description.clone(),
                    required: request_body.required,
                    content: Content {
                        media_type: media_type.parse()?,
                        type_,
                    },
                })
            }
            None => None,
        };

        // Visit responses.
        let mut responses = BTreeMap::new();

        for (status_code, response_ref) in &operation.responses.responses {
            let response = operation
                .resolve_reference_or(["responses", &status_code.to_string()], response_ref)?;

            let status_code: StatusCode = match status_code {
                openapiv3::StatusCode::Code(v) => http::StatusCode::from_u16(*v)
                    .map_err(|e| {
                        Error::Invalid(
                            response.ref_().clone(),
                            format!("inconvertible status codes ({})", e),
                        )
                    })?
                    .into(),
                openapiv3::StatusCode::Range(_) => {
                    return Err(Error::Unsupported(
                        response.ref_().clone(),
                        "status code ranges".to_string(),
                    ));
                }
            };

            let (media_type, type_) = match response.content.len() {
                0 => (None, None),
                1 => {
                    let (media_type, media_type_data) = response.content.iter().next().unwrap();

                    let type_ = match &media_type_data.schema {
                        Some(schema_ref) => Some({
                            self.resolve_type_ref(
                                ModelOrigin::ResponseBody {
                                    operation_name: operation_name.clone(),
                                    status_code: status_code.clone(),
                                },
                                &response
                                    .as_element_ref(["content", media_type, "schema"], schema_ref),
                                HashSet::new(),
                            )?
                        }),
                        None => None,
                    };

                    (Some(media_type), type_)
                }
                _ => {
                    return Err(Error::Invalid(
                        response.ref_().join(["content"]),
                        "responses with multiple media-types".to_string(),
                    ))
                }
            };

            let mut headers = BTreeMap::new();

            for (header_name, header_ref) in &response.headers {
                let header = response.resolve_reference_or(["headers", header_name], header_ref)?;

                if http::header::HeaderName::from_lowercase(header_name.as_bytes()).is_err() {
                    return Err(Error::InvalidHeaderName(header_name.clone()));
                }

                headers.insert(
                    header_name.clone(),
                    Header {
                        description: header.description.clone(),
                        type_: match &header.format {
                            openapiv3::ParameterSchemaOrContent::Schema(schema_ref) => self
                                .resolve_type_ref(
                                    ModelOrigin::Schemas,
                                    &header.as_element_ref(["format"], schema_ref),
                                    HashSet::new(),
                                )?,
                            openapiv3::ParameterSchemaOrContent::Content(_) => {
                                return Err(Error::Unsupported(
                                    header.ref_().join(["format"]),
                                    "header content formats".to_string(),
                                ));
                            }
                        },
                    },
                );
            }

            responses.insert(
                status_code,
                Response {
                    description: response.description.clone(),
                    content: match media_type {
                        Some(media_type) => Some(Content {
                            media_type: media_type.parse()?,
                            type_: type_.ok_or_else(|| {
                                Error::Invalid(
                                    response.ref_().clone(),
                                    "content without a schema".to_string(),
                                )
                            })?,
                        }),
                        None => None,
                    },
                    headers,
                },
            );
        }

        let route = Route {
            name: operation_name.clone(),
            method,
            summary: operation.summary.clone(),
            request_body,
            parameters,
            responses,
        };

        Ok(route)
    }

    fn visit_parameter(
        &mut self,
        parameter_data: &OpenApiElement<'_, openapiv3::ParameterData>,
        allowed_types: Option<Vec<Type>>,
    ) -> Result<Parameter> {
        let type_ = match &parameter_data.format {
            openapiv3::ParameterSchemaOrContent::Schema(schema_ref) => self.resolve_type_ref(
                ModelOrigin::Schemas,
                &parameter_data.as_element_ref(["format", "schema"], schema_ref),
                HashSet::new(),
            )?,
            openapiv3::ParameterSchemaOrContent::Content(_) => {
                return Err(Error::Unsupported(
                    parameter_data.ref_().join(["format", "content"]),
                    "parameter content specifiers".to_string(),
                ));
            }
        };

        if allowed_types.is_some() && !allowed_types.unwrap().contains(&type_) {
            return Err(Error::Unsupported(
                parameter_data.ref_().join(["format"]),
                format!("parameters of type `{:?}`", type_),
            ));
        }

        Ok(Parameter {
            name: parameter_data.name.clone(),
            description: parameter_data.description.clone(),
            type_,
            required: parameter_data.required,
        })
    }

    fn register_model_from_schema(
        &mut self,
        origin: ModelOrigin,
        schema: &OpenApiElement<'_, openapiv3::Schema>,
        visited_refs: HashSet<OpenApiRef>,
    ) -> Result<Type> {
        let model = Model {
            ref_: schema.ref_().clone(),
            description: schema.schema_data.description.clone(),
            origin,
            type_: self.resolve_type_from_schema(schema, visited_refs)?,
        };

        Ok(self.register_model(model))
    }

    fn register_model(&mut self, model: Model) -> Type {
        let type_ = model.to_named_type();

        let models = &mut self
            .ctx
            .location_contexts
            .entry(model.ref_.ref_location().clone())
            .or_default()
            .models;

        if !models.contains_key(model.ref_.json_pointer()) {
            models.insert(model.ref_.json_pointer().clone(), model);
        }

        type_
    }

    fn resolve_integer(integer_type: &OpenApiElement<'_, openapiv3::IntegerType>) -> Result<Type> {
        if !integer_type.enumeration.is_empty() {
            return Err(Error::Unsupported(
                integer_type.ref_().clone(),
                "integer enums".to_string(),
            ));
        }

        if let Some(minimum) = integer_type.minimum {
            if minimum == 0 {
                return Ok(match &integer_type.format {
                    openapiv3::VariantOrUnknownOrEmpty::Item(format) => match format {
                        openapiv3::IntegerFormat::Int32 => Type::UInt32,
                        openapiv3::IntegerFormat::Int64 => Type::UInt64,
                    },
                    _ => Type::UInt32,
                });
            }
        }

        Ok(match &integer_type.format {
            openapiv3::VariantOrUnknownOrEmpty::Item(format) => match format {
                openapiv3::IntegerFormat::Int32 => Type::Int32,
                openapiv3::IntegerFormat::Int64 => Type::Int64,
            },
            _ => Type::Int32,
        })
    }

    fn resolve_number(number_type: &OpenApiElement<'_, openapiv3::NumberType>) -> Result<Type> {
        if !number_type.enumeration.is_empty() {
            return Err(Error::Unsupported(
                number_type.ref_().clone(),
                "number enums".to_string(),
            ));
        }

        Ok(match &number_type.format {
            openapiv3::VariantOrUnknownOrEmpty::Item(format) => match format {
                openapiv3::NumberFormat::Float => Type::Float32,
                openapiv3::NumberFormat::Double => Type::Float64,
            },
            _ => Type::Float64,
        })
    }

    fn resolve_array(
        &mut self,
        array_type: &OpenApiElement<'_, openapiv3::ArrayType>,
        visited_refs: HashSet<OpenApiRef>,
    ) -> Result<Type> {
        match &array_type.items {
            None => Err(Error::Invalid(
                array_type.ref_().clone(),
                "no items".to_string(),
            )),
            Some(items) => {
                let type_ = self.resolve_type_ref(
                    ModelOrigin::Schemas,
                    &array_type.as_element_ref(["items"], &items.clone().unbox()),
                    visited_refs,
                )?;

                if array_type.unique_items {
                    Ok(Type::HashSet(Box::new(type_)))
                } else {
                    Ok(Type::Array(Box::new(type_)))
                }
            }
        }
    }

    fn resolve_string(string_type: &OpenApiElement<'_, openapiv3::StringType>) -> Result<Type> {
        Ok(if !string_type.enumeration.is_empty() {
            Type::Enum {
                variants: string_type
                    .enumeration
                    .iter()
                    .filter(|&s| s.is_some())
                    .map(|s| s.clone().unwrap())
                    .collect(),
            }
        } else {
            match &string_type.format {
                openapiv3::VariantOrUnknownOrEmpty::Item(format) => match format {
                    openapiv3::StringFormat::Byte => Type::Bytes,
                    openapiv3::StringFormat::Binary => Type::Binary,
                    openapiv3::StringFormat::Date => Type::Date,
                    openapiv3::StringFormat::DateTime => Type::DateTime,
                    openapiv3::StringFormat::Password => {
                        return Err(Error::Unsupported(
                            string_type.ref_().join(["format"]),
                            "password strings".to_string(),
                        ))
                    }
                },
                _ => Type::String,
            }
        })
    }

    fn resolve_object(
        &mut self,
        object_type: &OpenApiElement<'_, openapiv3::ObjectType>,
        visited_refs: &HashSet<OpenApiRef>,
    ) -> Result<Type> {
        // New model is created on the fly for each object type.

        let additional_properties = match &object_type.additional_properties {
            None | Some(AdditionalProperties::Any(false)) => None,
            Some(AdditionalProperties::Any(true)) => Some(Type::Any),
            Some(AdditionalProperties::Schema(property_ref)) => Some(self.resolve_type_ref(
                ModelOrigin::Schemas,
                &object_type.as_element_ref(["additionalProperties"], &*property_ref),
                visited_refs.clone(),
            )?),
        };

        if object_type.properties.is_empty() {
            if let Some(additional_properties) = additional_properties {
                return Ok(Type::Map(Box::new(additional_properties)));
            }
        }

        let mut properties = BTreeMap::new();

        for (property_name, property_ref) in &object_type.properties {
            let required = object_type.required.contains(property_name);
            let type_ = self.resolve_type_ref(
                ModelOrigin::ObjectProperty {
                    object_pointer: object_type.ref_().json_pointer().clone(),
                },
                &object_type
                    .as_element_ref(["properties", property_name], &property_ref.clone().unbox()),
                visited_refs.clone(),
            )?;
            let property = Field {
                name: property_name.to_string(),
                description: None, // TODO: Revisit this.
                type_,
                required,
            };

            properties.insert(property.name.clone(), property);
        }

        Ok(Type::Struct {
            fields: properties,
            map: additional_properties.map(Box::new),
        })
    }

    fn resolve_one_of(
        &mut self,
        one_of: &OpenApiElement<'_, Vec<openapiv3::ReferenceOr<openapiv3::Schema>>>,
        visited_refs: &HashSet<OpenApiRef>,
    ) -> Result<Type> {
        Ok(Type::OneOf {
            types: one_of
                .iter()
                .enumerate()
                .map(|(i, schema_ref)| {
                    self.resolve_type_ref(
                        ModelOrigin::Schemas,
                        &one_of.as_element_ref([&i.to_string()], schema_ref),
                        visited_refs.clone(),
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        })
    }

    fn resolve_type_from_schema(
        &mut self,
        schema: &OpenApiElement<'_, openapiv3::Schema>,
        visited_refs: HashSet<OpenApiRef>,
    ) -> Result<Type> {
        match &schema.schema_kind {
            openapiv3::SchemaKind::Type(type_) => match type_ {
                openapiv3::Type::Boolean {} => Ok(Type::Boolean),
                openapiv3::Type::Integer(t) => {
                    Self::resolve_integer(&schema.as_self_element_ref(t))
                }
                openapiv3::Type::Number(t) => Self::resolve_number(&schema.as_self_element_ref(t)),
                openapiv3::Type::String(t) => Self::resolve_string(&schema.as_self_element_ref(t)),
                openapiv3::Type::Array(t) => {
                    self.resolve_array(&schema.as_self_element_ref(t), visited_refs)
                }
                openapiv3::Type::Object(t) => {
                    self.resolve_object(&schema.as_self_element_ref(t), &visited_refs)
                }
            },
            openapiv3::SchemaKind::OneOf { one_of } => {
                self.resolve_one_of(&schema.as_element_ref(["one_of"], one_of), &visited_refs)
            }
            _ => Err(Error::Unsupported(
                schema.ref_().clone(),
                format!("schemas of kind `{:?}`", &schema.schema_kind),
            )),
        }
    }

    fn resolve_type_ref(
        &mut self,
        origin: ModelOrigin,
        schema: &OpenApiElement<'_, openapiv3::ReferenceOr<openapiv3::Schema>>,
        mut visited_refs: HashSet<OpenApiRef>,
    ) -> Result<Type> {
        match schema.as_ref() {
            openapiv3::ReferenceOr::Item(inner_schema) => {
                let schema = &schema.as_self_element_ref(inner_schema);
                let type_ = self.resolve_type_from_schema(schema, visited_refs.clone())?;

                // TODO: If we find another way to check for that, we may
                // simplify the whole function using `resolve_reference_or`
                // followed by `register_model_from_schema`.
                //
                // Moreover, this check is pretty language-specific (Rust) so it
                // feels wrong. Perhaps we should inject the logic of that check
                // into the visitor, depending on the language.
                if type_.requires_model() {
                    self.register_model_from_schema(origin, schema, visited_refs)
                } else {
                    Ok(type_)
                }
            }
            openapiv3::ReferenceOr::Reference { reference } => {
                let ref_ = OpenApiRef::new(schema.ref_().ref_location(), reference)?;

                let schema = schema
                    .loader()
                    .resolve_reference::<openapiv3::Schema>(ref_.clone())?;

                if visited_refs.insert(ref_.clone()) {
                    self.register_model_from_schema(ModelOrigin::Schemas, &schema, visited_refs)
                } else {
                    Ok(Type::Box(Box::new(Type::Named(ref_))))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        api_types::{Content, LocationContext, MediaType, Path},
        openapi_loader::{JsonPointer, OpenApiLoader},
    };

    use super::*;

    #[test]
    fn test_resolve_integer() {
        let loader = OpenApiLoader::default();
        let int = loader
            .import_from_yaml(
                "int",
                r#"
            type: integer
            "#,
            )
            .unwrap();
        let int32 = loader
            .import_from_yaml(
                "int32",
                r#"
            type: integer
            format: int32
            "#,
            )
            .unwrap();
        let int64 = loader
            .import_from_yaml(
                "int64",
                r#"
            type: integer
            format: int64
            "#,
            )
            .unwrap();
        let uint = loader
            .import_from_yaml(
                "uint",
                r#"
            type: integer
            minimum: 0
            "#,
            )
            .unwrap();
        let uint32 = loader
            .import_from_yaml(
                "uint32",
                r#"
            type: integer
            format: int32
            minimum: 0
            "#,
            )
            .unwrap();
        let uint64 = loader
            .import_from_yaml(
                "uint64",
                r#"
            type: integer
            format: int64
            minimum: 0
            "#,
            )
            .unwrap();

        assert_eq!(Visitor::resolve_integer(&int).unwrap(), Type::Int32);
        assert_eq!(Visitor::resolve_integer(&int32).unwrap(), Type::Int32);
        assert_eq!(Visitor::resolve_integer(&int64).unwrap(), Type::Int64);
        assert_eq!(Visitor::resolve_integer(&uint).unwrap(), Type::UInt32);
        assert_eq!(Visitor::resolve_integer(&uint32).unwrap(), Type::UInt32);
        assert_eq!(Visitor::resolve_integer(&uint64).unwrap(), Type::UInt64);
    }

    #[test]
    fn test_resolve_number() {
        let loader = OpenApiLoader::default();
        let num = loader
            .import_from_yaml(
                "num",
                r#"
            type: number
            "#,
            )
            .unwrap();
        let num32 = loader
            .import_from_yaml(
                "num32",
                r#"
            type: number
            format: float
            "#,
            )
            .unwrap();
        let num64 = loader
            .import_from_yaml(
                "num64",
                r#"
            type: number
            format: double
            "#,
            )
            .unwrap();

        assert_eq!(Visitor::resolve_number(&num).unwrap(), Type::Float64);
        assert_eq!(Visitor::resolve_number(&num32).unwrap(), Type::Float32);
        assert_eq!(Visitor::resolve_number(&num64).unwrap(), Type::Float64);
    }

    #[test]
    fn test_resolve_string() {
        let loader = OpenApiLoader::default();
        let str = loader
            .import_from_yaml(
                "str",
                r#"
            type: string
            "#,
            )
            .unwrap();
        let str_date = loader
            .import_from_yaml(
                "str_date",
                r#"
            type: string
            format: date
            "#,
            )
            .unwrap();
        let str_date_time = loader
            .import_from_yaml(
                "str_date_time",
                r#"
            type: string
            format: date-time
            "#,
            )
            .unwrap();
        let str_bytes = loader
            .import_from_yaml(
                "str_bytes",
                r#"
            type: string
            format: byte
            "#,
            )
            .unwrap();
        let str_binary = loader
            .import_from_yaml(
                "str_binary",
                r#"
            type: string
            format: binary
            "#,
            )
            .unwrap();

        assert_eq!(Visitor::resolve_string(&str).unwrap(), Type::String);
        assert_eq!(Visitor::resolve_string(&str_date).unwrap(), Type::Date);
        assert_eq!(
            Visitor::resolve_string(&str_date_time).unwrap(),
            Type::DateTime
        );
        assert_eq!(Visitor::resolve_string(&str_bytes).unwrap(), Type::Bytes);
        assert_eq!(Visitor::resolve_string(&str_binary).unwrap(), Type::Binary);
    }

    #[test]
    fn test_resolve_string_enum() {
        let components = serde_yaml::from_str::<openapiv3::Components>(
            r#"
            schemas:
              MyStruct:
                type: object
                properties:
                  my_enum:
                    type: string
                    enum:
                      - foo
                      - bar
            "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            components: Some(components),
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api.yaml", &api).unwrap();
        let ctx = Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas.clone()])
            .unwrap();

        let my_struct_ref = oas.ref_().join(
            "/components/schemas/MyStruct"
                .parse::<JsonPointer>()
                .unwrap(),
        );
        let my_struct_my_enum_ref = oas.ref_().join(
            "/components/schemas/MyStruct/properties/my_enum"
                .parse::<JsonPointer>()
                .unwrap(),
        );

        let expected_struct = Model {
            ref_: my_struct_ref.clone(),
            description: None,
            origin: ModelOrigin::Schemas,
            type_: Type::Struct {
                fields: BTreeMap::from([(
                    "my_enum".to_string(),
                    Field {
                        name: "my_enum".to_string(),
                        description: None,
                        type_: Type::Named(my_struct_my_enum_ref.clone()),
                        required: false,
                    },
                )]),
                map: None,
            },
        };

        let expected_enum = Model {
            ref_: my_struct_my_enum_ref.clone(),
            description: None,
            origin: ModelOrigin::ObjectProperty {
                object_pointer: my_struct_ref.json_pointer().clone(),
            },
            type_: Type::Enum {
                variants: vec!["foo".to_string(), "bar".to_string()],
            },
        };

        let models = &ctx
            .location_contexts
            .get(oas.ref_().ref_location())
            .unwrap()
            .models;

        assert_eq!(models.len(), 2);
        assert_eq!(
            models.get(my_struct_ref.json_pointer()),
            Some(&expected_struct)
        );
        assert_eq!(
            models.get(my_struct_my_enum_ref.json_pointer()),
            Some(&expected_enum)
        );
    }

    #[test]
    fn test_resolve_array() {
        let loader = OpenApiLoader::default();
        let array = loader
            .import_from_yaml(
                "array",
                r#"
            type: array
            items:
              type: string
            "#,
            )
            .unwrap();

        let mut v = Visitor::new(std::env::current_dir().unwrap());

        assert_eq!(
            v.resolve_array(&array, HashSet::new()).unwrap(),
            Type::Array(Box::new(Type::String))
        );
    }

    #[test]
    fn test_resolve_hashset() {
        let loader = OpenApiLoader::default();
        let array = loader
            .import_from_yaml(
                "array",
                r#"
            type: array
            items:
              type: string
            uniqueItems: true
            "#,
            )
            .unwrap();

        let mut v = Visitor::new(std::env::current_dir().unwrap());

        assert_eq!(
            v.resolve_array(&array, HashSet::new()).unwrap(),
            Type::HashSet(Box::new(Type::String))
        );
    }

    #[test]
    fn test_resolve_array_ref() {
        let components = serde_yaml::from_str(
            r#"
            schemas:
              Category:
                type: string
              Tag:
                type: string
              MyStruct:
                type: object
                properties:
                  category:
                    $ref: '#/components/schemas/Category'
                  tags:
                    type: array
                    items:
                      $ref: '#/components/schemas/Tag'
            "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            components: Some(components),
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api", &api).unwrap();
        let ctx = Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas.clone()])
            .unwrap();

        let my_struct_ref = oas.ref_().join(
            "/components/schemas/MyStruct"
                .parse::<JsonPointer>()
                .unwrap(),
        );
        let category_ref = oas.ref_().join(
            "/components/schemas/Category"
                .parse::<JsonPointer>()
                .unwrap(),
        );
        let tag_ref = oas
            .ref_()
            .join("/components/schemas/Tag".parse::<JsonPointer>().unwrap());

        let expected_struct = Model {
            ref_: my_struct_ref.clone(),
            description: None,
            origin: ModelOrigin::Schemas,
            type_: Type::Struct {
                fields: BTreeMap::from([
                    (
                        "category".to_string(),
                        Field {
                            name: "category".to_string(),
                            description: None,
                            type_: Type::Named(category_ref),
                            required: false,
                        },
                    ),
                    (
                        "tags".to_string(),
                        Field {
                            name: "tags".to_string(),
                            description: None,
                            type_: Type::Array(Box::new(Type::Named(tag_ref))),
                            required: false,
                        },
                    ),
                ]),
                map: None,
            },
        };

        let models = &ctx
            .location_contexts
            .get(oas.ref_().ref_location())
            .unwrap()
            .models;

        assert_eq!(models.len(), 3);
        assert_eq!(
            models.get(my_struct_ref.json_pointer()),
            Some(&expected_struct)
        );
    }

    #[test]
    fn test_resolve_struct() {
        let components = serde_yaml::from_str::<openapiv3::Components>(
            r#"
            schemas:
              MyStruct:
                type: object
                properties:
                  my_prop1:
                    type: string
                  my_prop2:
                    type: integer
                required:
                  - my_prop1
            "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            components: Some(components),
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api", &api).unwrap();
        let ctx = Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas.clone()])
            .unwrap();

        let my_struct_ref = oas.ref_().join(
            "/components/schemas/MyStruct"
                .parse::<JsonPointer>()
                .unwrap(),
        );

        let expected_struct = Model {
            ref_: my_struct_ref.clone(),
            description: None,
            origin: ModelOrigin::Schemas,
            type_: Type::Struct {
                fields: BTreeMap::from([
                    (
                        "my_prop1".to_string(),
                        Field {
                            name: "my_prop1".to_string(),
                            description: None,
                            type_: Type::String,
                            required: true,
                        },
                    ),
                    (
                        "my_prop2".to_string(),
                        Field {
                            name: "my_prop2".to_string(),
                            description: None,
                            type_: Type::Int32,
                            required: false,
                        },
                    ),
                ]),
                map: None,
            },
        };

        let models = &ctx
            .location_contexts
            .get(oas.ref_().ref_location())
            .unwrap()
            .models;

        assert_eq!(models.len(), 1);
        assert_eq!(
            models.get(my_struct_ref.json_pointer()),
            Some(&expected_struct)
        );
    }

    #[test]
    fn test_resolve_inner_struct() {
        let components = serde_yaml::from_str::<openapiv3::Components>(
            r#"
            schemas:
              MyStruct:
                type: object
                properties:
                  my_inner_struct:
                    type: object
                    properties:
                      my_inner_prop:
                        type: string
            "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            components: Some(components),
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api", &api).unwrap();
        let ctx = Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas.clone()])
            .unwrap();

        let my_struct_ref = oas.ref_().join(
            "/components/schemas/MyStruct"
                .parse::<JsonPointer>()
                .unwrap(),
        );
        let my_inner_struct_ref = oas.ref_().join(
            "/components/schemas/MyStruct/properties/my_inner_struct"
                .parse::<JsonPointer>()
                .unwrap(),
        );

        let expected_struct = Model {
            ref_: my_struct_ref.clone(),
            description: None,
            origin: ModelOrigin::Schemas,
            type_: Type::Struct {
                fields: BTreeMap::from([(
                    "my_inner_struct".to_string(),
                    Field {
                        name: "my_inner_struct".to_string(),
                        description: None,
                        type_: Type::Named(my_inner_struct_ref.clone()),
                        required: false,
                    },
                )]),
                map: None,
            },
        };

        let expected_inner_struct = Model {
            ref_: my_inner_struct_ref.clone(),
            description: None,
            origin: ModelOrigin::ObjectProperty {
                object_pointer: my_struct_ref.json_pointer().clone(),
            },
            type_: Type::Struct {
                fields: BTreeMap::from([(
                    "my_inner_prop".to_string(),
                    Field {
                        name: "my_inner_prop".to_string(),
                        description: None,
                        type_: Type::String,
                        required: false,
                    },
                )]),
                map: None,
            },
        };

        let models = &ctx
            .location_contexts
            .get(oas.ref_().ref_location())
            .unwrap()
            .models;

        assert_eq!(models.len(), 2);
        assert_eq!(
            models.get(my_struct_ref.json_pointer()),
            Some(&expected_struct)
        );
        assert_eq!(
            models.get(my_inner_struct_ref.json_pointer()),
            Some(&expected_inner_struct)
        );
    }

    #[test]
    fn test_resolve_get_operation() {
        let components = serde_yaml::from_str::<openapiv3::Components>(
            r#"
            schemas:
              Pet:
                type: object
                properties:
                    name:
                      type: string
            "#,
        )
        .unwrap();

        let paths = serde_yaml::from_str::<openapiv3::Paths>(
            r#"
            /pets:
              get:
                summary: Finds pets by tags.
                operationId: findPetsByTags
                parameters:
                  - name: tags
                    in: query
                    description: Tags to filter by
                    required: true
                    schema:
                      type: array
                      items:
                        type: string
                responses:
                  '200':
                    description: Successful
                    content:
                      application/json:
                        schema:
                          type: array
                          items:
                            $ref: '#/components/schemas/Pet'
                       "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            components: Some(components),
            paths,
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api", &api).unwrap();
        let ctx = Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas.clone()])
            .unwrap();

        let my_struct_ref = oas
            .ref_()
            .join("/components/schemas/Pet".parse::<JsonPointer>().unwrap());

        let expected_struct = Model {
            ref_: my_struct_ref.clone(),
            description: None,
            origin: ModelOrigin::Schemas,
            type_: Type::Struct {
                fields: BTreeMap::from([(
                    "name".to_string(),
                    Field {
                        name: "name".to_string(),
                        description: None,
                        type_: Type::String,
                        required: false,
                    },
                )]),
                map: None,
            },
        };

        let expected_route = Route {
            name: "findPetsByTags".to_string(),
            method: Method::Get,
            summary: Some("Finds pets by tags.".to_string()),
            request_body: None,
            parameters: Parameters {
                query: vec![Parameter {
                    name: "tags".to_string(),
                    description: Some("Tags to filter by".to_string()),
                    required: true,
                    type_: Type::Array(Box::new(Type::String)),
                }],
                ..Parameters::default()
            },
            responses: BTreeMap::from([(
                http::StatusCode::OK.into(),
                Response {
                    description: "Successful".to_string(),
                    content: Some(Content {
                        media_type: MediaType::Json,
                        type_: Type::Array(Box::new(Type::Named(my_struct_ref.clone()))),
                    }),
                    headers: BTreeMap::new(),
                },
            )]),
        };

        let LocationContext { api, models } = ctx
            .location_contexts
            .get(oas.ref_().ref_location())
            .unwrap();
        let api = api.as_ref().unwrap();

        assert_eq!(models.len(), 1);
        assert_eq!(
            models.get(my_struct_ref.json_pointer()),
            Some(&expected_struct)
        );

        assert_eq!(api.paths.len(), 1);
        assert_eq!(
            api.paths.get::<Path>(&"/pets".into()),
            Some(&vec![expected_route])
        );
    }

    #[test]
    fn test_resolve_request_body() {
        let components = serde_yaml::from_str::<openapiv3::Components>(
            r#"
            requestBodies:
              Pet:
                content:
                  application/json:
                    schema:
                      type: object
                      properties:
                        pet_data:
                          type: object
                          properties:
                            name:
                              type: string
            "#,
        )
        .unwrap();

        let paths = serde_yaml::from_str::<openapiv3::Paths>(
            r#"
            /pets:
              post:
                operationId: addPet
                responses:
                  '200':
                    description: Successful
                requestBody:
                  $ref: '#/components/requestBodies/Pet'
            "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            components: Some(components),
            paths,
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api", &api).unwrap();
        let ctx = Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas.clone()])
            .unwrap();

        let my_struct_ref = oas.ref_().join(
            "/components/requestBodies/Pet/content/application~1json/schema"
                .parse::<JsonPointer>()
                .unwrap(),
        );
        let my_inner_struct_ref = oas.ref_().join(
            "/components/requestBodies/Pet/content/application~1json/schema/properties/pet_data"
                .parse::<JsonPointer>()
                .unwrap(),
        );

        let expected_struct = Model {
            ref_: my_struct_ref.clone(),
            description: None,
            origin: ModelOrigin::RequestBody {
                operation_name: "addPet".to_string(),
            },
            type_: Type::Struct {
                fields: BTreeMap::from([(
                    "pet_data".to_string(),
                    Field {
                        name: "pet_data".to_string(),
                        description: None,
                        type_: Type::Named(my_inner_struct_ref.clone()),
                        required: false,
                    },
                )]),
                map: None,
            },
        };

        let expected_inner_struct = Model {
            ref_: my_inner_struct_ref.clone(),
            description: None,
            origin: ModelOrigin::ObjectProperty {
                object_pointer: my_struct_ref.json_pointer().clone(),
            },
            type_: Type::Struct {
                fields: BTreeMap::from([(
                    "name".to_string(),
                    Field {
                        name: "name".to_string(),
                        description: None,
                        type_: Type::String,
                        required: false,
                    },
                )]),
                map: None,
            },
        };

        let expected_route = Route {
            name: "addPet".to_string(),
            method: Method::Post,
            summary: None,
            request_body: Some(RequestBody {
                description: None,
                required: false,
                content: Content {
                    media_type: MediaType::Json,
                    type_: Type::Named(my_struct_ref.clone()),
                },
            }),
            parameters: Parameters::default(),
            responses: BTreeMap::from([(
                http::StatusCode::OK.into(),
                Response {
                    description: "Successful".to_string(),
                    content: None,
                    headers: BTreeMap::new(),
                },
            )]),
        };

        let LocationContext { api, models } = ctx
            .location_contexts
            .get(oas.ref_().ref_location())
            .unwrap();
        let api = api.as_ref().unwrap();

        assert_eq!(models.len(), 2);
        assert_eq!(
            models.get(my_struct_ref.json_pointer()),
            Some(&expected_struct)
        );
        assert_eq!(
            models.get(my_inner_struct_ref.json_pointer()),
            Some(&expected_inner_struct)
        );

        assert_eq!(api.paths.len(), 1);
        assert_eq!(
            api.paths.get::<Path>(&"/pets".into()),
            Some(&vec![expected_route])
        );
    }

    #[test]
    fn test_resolve_post_operation() {
        let components = serde_yaml::from_str::<openapiv3::Components>(
            r#"
            requestBodies:
              Pet:
                content:
                  application/json:
                    schema:
                      $ref: '#/components/schemas/Pet'
            schemas:
              Pet:
                type: object
                properties:
                    name:
                      type: string
            "#,
        )
        .unwrap();

        let paths = serde_yaml::from_str::<openapiv3::Paths>(
            r#"
            /pets:
              post:
                summary: Add a new pet to the store.
                operationId: addPet
                responses:
                  '200':
                    description: Successful
                    content:
                      application/json:
                        schema:
                          $ref: '#/components/schemas/Pet'
                  '405':
                    description: Invalid input
                requestBody:
                  $ref: '#/components/requestBodies/Pet'
            "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            components: Some(components),
            paths,
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api", &api).unwrap();
        let ctx = Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas.clone()])
            .unwrap();

        let my_struct_ref = oas
            .ref_()
            .join("/components/schemas/Pet".parse::<JsonPointer>().unwrap());

        let expected_struct = Model {
            ref_: my_struct_ref.clone(),
            description: None,
            origin: ModelOrigin::Schemas,
            type_: Type::Struct {
                fields: BTreeMap::from([(
                    "name".to_string(),
                    Field {
                        name: "name".to_string(),
                        description: None,
                        type_: Type::String,
                        required: false,
                    },
                )]),
                map: None,
            },
        };

        let expected_route = Route {
            name: "addPet".to_string(),
            method: Method::Post,
            summary: Some("Add a new pet to the store.".to_string()),
            request_body: Some(RequestBody {
                description: None,
                required: false,
                content: Content {
                    media_type: MediaType::Json,
                    type_: Type::Named(my_struct_ref.clone()),
                },
            }),
            parameters: Parameters::default(),
            responses: BTreeMap::from([
                (
                    http::StatusCode::OK.into(),
                    Response {
                        description: "Successful".to_string(),
                        content: Some(Content {
                            media_type: MediaType::Json,
                            type_: Type::Named(my_struct_ref.clone()),
                        }),
                        headers: BTreeMap::new(),
                    },
                ),
                (
                    http::StatusCode::METHOD_NOT_ALLOWED.into(),
                    Response {
                        description: "Invalid input".to_string(),
                        content: None,
                        headers: BTreeMap::new(),
                    },
                ),
            ]),
        };

        let LocationContext { api, models } = ctx
            .location_contexts
            .get(oas.ref_().ref_location())
            .unwrap();
        let api = api.as_ref().unwrap();

        assert_eq!(models.len(), 1);
        assert_eq!(
            models.get(my_struct_ref.json_pointer()),
            Some(&expected_struct)
        );

        assert_eq!(api.paths.len(), 1);
        assert_eq!(
            api.paths.get::<Path>(&"/pets".into()),
            Some(&vec![expected_route])
        );
    }

    #[test]
    fn test_unsupported_media_type() {
        let paths = serde_yaml::from_str::<openapiv3::Paths>(
            r#"
            /pets:
              post:
                operationId: addPet
                responses:
                  '200':
                    description: Successful
                requestBody:
                  content:
                    test/test:
                      schema:
                        type: string
            "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            paths,
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api", &api).unwrap();
        Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas])
            .unwrap_err();
    }

    #[test]
    fn test_resolve_one_of() {
        let components = serde_yaml::from_str::<openapiv3::Components>(
            r#"
            schemas:
              Pet:
                type: object
                properties:
                  name:
                    type: string
              Car:
                type: object
                properties:
                  name:
                    type: string
            "#,
        )
        .unwrap();

        let paths = serde_yaml::from_str::<openapiv3::Paths>(
            r#"
            /test-one-of:
              get:
                operationId: testOneOf
                responses:
                  '200':
                    description: Ok.
                    content:
                      application/json:
                        schema:
                          oneOf:
                            - $ref: '#/components/schemas/Pet'
                            - $ref: '#/components/schemas/Car'
            "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            components: Some(components),
            paths,
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api", &api).unwrap();
        let ctx = Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas.clone()])
            .unwrap();

        let response_ref = oas.ref_().join(
            "/paths/~1test-one-of/get/responses/200/content/application~1json/schema"
                .parse::<JsonPointer>()
                .unwrap(),
        );
        let pet_ref = oas
            .ref_()
            .join("/components/schemas/Pet".parse::<JsonPointer>().unwrap());
        let car_ref = oas
            .ref_()
            .join("/components/schemas/Car".parse::<JsonPointer>().unwrap());

        let expected_one_of = Model {
            ref_: response_ref.clone(),
            description: None,
            origin: ModelOrigin::ResponseBody {
                operation_name: "testOneOf".to_string(),
                status_code: http::StatusCode::OK.into(),
            },
            type_: Type::OneOf {
                types: vec![Type::Named(pet_ref), Type::Named(car_ref)],
            },
        };

        let models = &ctx
            .location_contexts
            .get(oas.ref_().ref_location())
            .unwrap()
            .models;

        assert_eq!(models.len(), 3);
        assert_eq!(
            models.get(response_ref.json_pointer()),
            Some(&expected_one_of)
        );
    }

    #[test]
    fn test_resolve_headers() {
        let paths = serde_yaml::from_str::<openapiv3::Paths>(
            r#"
            /test-headers:
              get:
                operationId: testHeaders
                parameters:
                  - name: x-static-header
                    in: header
                    schema:
                      type: string
                responses:
                  '200':
                    description: Ok.
                    headers:
                      x-static-header:
                        schema:
                          type: string
            "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            paths,
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api", &api).unwrap();
        let ctx = Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas.clone()])
            .unwrap();

        let expected_route = Route {
            name: "testHeaders".to_string(),
            method: Method::Get,
            summary: None,
            request_body: None,
            parameters: Parameters {
                header: vec![Parameter {
                    name: "x-static-header".to_string(),
                    description: None,
                    required: false,
                    type_: Type::String,
                }],
                ..Parameters::default()
            },
            responses: BTreeMap::from([(
                http::StatusCode::OK.into(),
                Response {
                    description: "Ok.".to_string(),
                    content: None,
                    headers: BTreeMap::from([(
                        "x-static-header".to_string(),
                        Header {
                            description: None,
                            type_: Type::String,
                        },
                    )]),
                },
            )]),
        };

        let api = ctx
            .location_contexts
            .get(oas.ref_().ref_location())
            .unwrap()
            .api
            .as_ref()
            .unwrap();

        assert_eq!(api.paths.len(), 1);
        assert_eq!(
            api.paths.get::<Path>(&"/test-headers".into()),
            Some(&vec![expected_route])
        );
    }

    #[test]
    fn test_resolve_operation_with_path_level_parameters() {
        let paths = serde_yaml::from_str::<openapiv3::Paths>(
            r#"
            /foo/{a}/bar/{b}:
              parameters:
                - name: a
                  in: path
                  required: true
                  schema:
                    type: string
                - name: b
                  in: path
                  required: true
                  schema:
                    type: string
              get:
                operationId: foo
                parameters:
                  - name: b
                    in: path
                    required: true
                    schema:
                      type: integer
                  - name: c
                    in: query
                    required: true
                    schema:
                      type: string
                responses:
                  '200':
                    description: Successful
            "#,
        )
        .unwrap();

        let loader = OpenApiLoader::default();
        let api = openapiv3::OpenAPI {
            components: None,
            paths,
            ..openapiv3::OpenAPI::default()
        };
        let oas: OpenApi<'_> = loader.import("api", &api).unwrap();
        let ctx = Visitor::new(std::env::current_dir().unwrap())
            .visit(&[oas.clone()])
            .unwrap();

        let expected_route = Route {
            name: "foo".to_string(),
            method: Method::Get,
            summary: None,
            request_body: None,
            parameters: Parameters {
                path: vec![
                    Parameter {
                        name: "a".to_string(),
                        description: None,
                        required: true,
                        type_: Type::String,
                    },
                    Parameter {
                        name: "b".to_string(),
                        description: None,
                        required: true,
                        type_: Type::Int32,
                    },
                ],
                query: vec![Parameter {
                    name: "c".to_string(),
                    description: None,
                    required: true,
                    type_: Type::String,
                }],
                ..Parameters::default()
            },
            responses: BTreeMap::from([(
                http::StatusCode::OK.into(),
                Response {
                    description: "Successful".to_string(),
                    content: None,
                    headers: BTreeMap::new(),
                },
            )]),
        };

        let api = ctx
            .location_contexts
            .get(oas.ref_().ref_location())
            .unwrap()
            .api
            .as_ref()
            .unwrap();

        assert_eq!(api.paths.len(), 1);
        assert_eq!(
            api.paths.get::<Path>(&"/foo/{a}/bar/{b}".into()),
            Some(&vec![expected_route])
        );
    }
}
