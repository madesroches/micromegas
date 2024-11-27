//! Manual parsing of dynamically sized events
use anyhow::{Context, Result};
use micromegas_transit::{
    advance_window, parse_pod_instance, read_advance_string, read_consume_pod,
    value::{Object, Value},
    CustomReaderMap, InProcSerialize, LegacyDynString, UserDefinedType,
};
use std::{collections::HashMap, sync::Arc};

use crate::property_set::PROPERTY_SET_DEP_TYPE_NAME;

lazy_static::lazy_static! {
    static ref TIME: Arc<String> = Arc::new("time".into());
    static ref LEVEL: Arc<String> = Arc::new("level".into());
    static ref TARGET: Arc<String> = Arc::new("target".into());
    static ref MSG: Arc<String> = Arc::new("msg".into());
    static ref DESC: Arc<String> = Arc::new("desc".into());
    static ref PROPERTIES: Arc<String> = Arc::new("properties".into());
}

fn parse_log_string_event(
    udt: &UserDefinedType,
    _udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value>,
    mut object_window: &[u8],
) -> Result<Value> {
    let desc_id: u64 = read_consume_pod(&mut object_window);
    let time: i64 = read_consume_pod(&mut object_window);
    let msg = String::from_utf8(object_window.to_vec()).with_context(|| "parsing legacy string")?;
    let desc = dependencies
        .get(&desc_id)
        .with_context(|| format!("desc member {} of LogStringEvent not found", desc_id))?;
    let members = vec![
        (TIME.clone(), Value::I64(time)),
        (MSG.clone(), Value::String(Arc::new(msg))),
        (DESC.clone(), desc.clone()),
    ];
    Ok(Value::Object(Arc::new(Object {
        type_name: udt.name.clone(),
        members,
    })))
}

fn parse_log_string_event_v2(
    udt: &UserDefinedType,
    _udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value>,
    mut object_window: &[u8],
) -> Result<Value> {
    let desc_id: u64 = read_consume_pod(&mut object_window);
    let time: i64 = read_consume_pod(&mut object_window);
    let msg = read_advance_string(&mut object_window).with_context(|| "parsing string")?;
    let desc: Value = dependencies
        .get(&desc_id)
        .with_context(|| format!("desc member {} of LogStringEvent not found", desc_id))?
        .clone();
    let members = vec![
        (TIME.clone(), Value::I64(time)),
        (MSG.clone(), Value::String(Arc::new(msg))),
        (DESC.clone(), desc),
    ];
    Ok(Value::Object(Arc::new(Object {
        type_name: udt.name.clone(),
        members,
    })))
}

fn parse_log_string_interop_event_v3(
    udt: &UserDefinedType,
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value>,
    mut object_window: &[u8],
) -> Result<Value> {
    let string_ref_metadata = udts
        .iter()
        .find(|t| *t.name == "StaticStringRef")
        .with_context(|| {
            "Can't parse log string interop event with no metadata for StaticStringRef"
        })?;
    let time: i64 = read_consume_pod(&mut object_window);
    let level: u8 = read_consume_pod(&mut object_window);
    let target = parse_pod_instance(
        string_ref_metadata,
        udts,
        dependencies,
        &object_window[0..string_ref_metadata.size],
    )
    .with_context(|| "parse_pod_instance")?;
    object_window = advance_window(object_window, string_ref_metadata.size);
    let msg = read_advance_string(&mut object_window)?;
    let members = vec![
        (TIME.clone(), Value::I64(time)),
        (LEVEL.clone(), Value::U8(level)),
        (TARGET.clone(), target),
        (MSG.clone(), Value::String(Arc::new(msg))),
    ];
    Ok(Value::Object(Arc::new(Object {
        type_name: udt.name.clone(),
        members,
    })))
}

fn parse_tagged_log_interop_event(
    udt: &UserDefinedType,
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value>,
    mut object_window: &[u8],
) -> Result<Value> {
    let string_ref_metadata = udts
        .iter()
        .find(|t| *t.name == "StaticStringRef")
        .with_context(|| {
            "Can't parse log string interop event with no metadata for StaticStringRef"
        })?;
    let time: i64 = read_consume_pod(&mut object_window);
    let level: u8 = read_consume_pod(&mut object_window);
    let target = parse_pod_instance(
        string_ref_metadata,
        udts,
        dependencies,
        &object_window[0..string_ref_metadata.size],
    )
    .with_context(|| "parse_pod_instance")?;
    object_window = advance_window(object_window, string_ref_metadata.size);
    let properties_id: u64 = read_consume_pod(&mut object_window);
    let properties = dependencies
        .get(&properties_id)
        .with_context(|| "fetching properties in parse_tagged_log_interop_event")?
        .clone();
    let msg = read_advance_string(&mut object_window)?;
    let members = vec![
        (TIME.clone(), Value::I64(time)),
        (LEVEL.clone(), Value::U8(level)),
        (TARGET.clone(), target),
        (PROPERTIES.clone(), properties),
        (MSG.clone(), Value::String(Arc::new(msg))),
    ];
    Ok(Value::Object(Arc::new(Object {
        type_name: udt.name.clone(),
        members,
    })))
}

fn parse_tagged_log_string(
    udt: &UserDefinedType,
    _udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value>,
    mut object_window: &[u8],
) -> Result<Value> {
    let desc_id: u64 = read_consume_pod(&mut object_window);
    let desc = dependencies
        .get(&desc_id)
        .with_context(|| "fetching desc in parse_tagged_log_string")?
        .clone();
    let properties_id: u64 = read_consume_pod(&mut object_window);
    let properties = dependencies
        .get(&properties_id)
        .with_context(|| "fetching property set in parse_tagged_log_string")?
        .clone();
    let time: i64 = read_consume_pod(&mut object_window);
    let msg = read_advance_string(&mut object_window)?;

    let members = vec![
        (TIME.clone(), Value::I64(time)),
        (DESC.clone(), desc),
        (PROPERTIES.clone(), properties),
        (MSG.clone(), Value::String(Arc::new(msg))),
    ];
    Ok(Value::Object(Arc::new(Object {
        type_name: udt.name.clone(),
        members,
    })))
}

fn parse_log_string_interop_event(
    udt: &UserDefinedType,
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value>,
    mut object_window: &[u8],
) -> Result<Value> {
    let stringid_metadata = udts
        .iter()
        .find(|t| *t.name == "StringId")
        .with_context(|| "Can't parse log string interop event with no metadata for StringId")?;
    unsafe {
        let time: i64 = read_consume_pod(&mut object_window);
        let level: u32 = read_consume_pod(&mut object_window);
        let target = parse_pod_instance(
            stringid_metadata,
            udts,
            dependencies,
            &object_window[0..stringid_metadata.size],
        )
        .with_context(|| "parse_pod_instance")?;
        object_window = advance_window(object_window, stringid_metadata.size);
        let msg = <LegacyDynString as InProcSerialize>::read_value(object_window);
        let members = vec![
            (TIME.clone(), Value::I64(time)),
            (LEVEL.clone(), Value::U32(level)),
            (TARGET.clone(), target),
            (MSG.clone(), Value::String(Arc::new(msg.0))),
        ];
        Ok(Value::Object(Arc::new(Object {
            type_name: udt.name.clone(),
            members,
        })))
    }
}

fn parse_property_set(
    _udt: &UserDefinedType,
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value>,
    mut window: &[u8],
) -> Result<Value> {
    let property_layout = udts
        .iter()
        .find(|t| *t.name == "Property")
        .with_context(|| "could not find Property layout")?;

    let object_id: u64 = read_consume_pod(&mut window);
    let nb_properties: u32 = read_consume_pod(&mut window);
    let mut members = vec![];
    for i in 0..nb_properties {
        let property_size = property_layout.size;
        let begin = i as usize * property_size;
        let property_window = &window[begin..begin + property_size];
        if let Value::Object(obj) =
            parse_pod_instance(property_layout, udts, dependencies, property_window)?
        {
            members.push((
                obj.get::<Arc<String>>("name")?,
                Value::String(obj.get::<Arc<String>>("value")?),
            ));
        } else {
            anyhow::bail!("invalid property in propertyset");
        }
    }

    lazy_static! {
        static ref PROPERTY_SET_TYPE_NAME: Arc<String> = Arc::new("property_set".into());
        static ref ID: Arc<String> = Arc::new("id".into());
        static ref VALUE: Arc<String> = Arc::new("value".into());
    }

    let set = Arc::new(Object {
        type_name: PROPERTY_SET_TYPE_NAME.clone(),
        members,
    });
    Ok(Value::Object(Arc::new(Object {
        type_name: PROPERTY_SET_DEP_TYPE_NAME.clone(),
        members: vec![
            (ID.clone(), Value::U64(object_id)),
            (VALUE.clone(), Value::Object(set)),
        ],
    })))
}

/// Dictionnary of custom readers for dynamically sized events
pub fn make_custom_readers() -> CustomReaderMap {
    let mut custom_readers: CustomReaderMap = HashMap::new();
    custom_readers.insert("LogStringEvent".into(), Arc::new(parse_log_string_event));
    custom_readers.insert(
        "LogStringEventV2".into(),
        Arc::new(parse_log_string_event_v2),
    );
    custom_readers.insert(
        "LogStringInteropEventV2".into(),
        Arc::new(parse_log_string_interop_event),
    );
    custom_readers.insert(
        "LogStringInteropEventV3".into(),
        Arc::new(parse_log_string_interop_event_v3),
    );
    custom_readers.insert("TaggedLogString".into(), Arc::new(parse_tagged_log_string));
    custom_readers.insert(
        PROPERTY_SET_DEP_TYPE_NAME.to_string(),
        Arc::new(parse_property_set),
    );
    custom_readers.insert(
        "TaggedLogInteropEvent".into(),
        Arc::new(parse_tagged_log_interop_event),
    );
    custom_readers
}
