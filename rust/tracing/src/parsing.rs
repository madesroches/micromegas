//! Manual parsing of dynamically sized events
use anyhow::{Context, Result};
use bumpalo::Bump;
use micromegas_transit::{
    CustomReaderMap, UserDefinedType, advance_window, parse_pod_instance, read_advance_string_in,
    read_consume_pod,
    value::{Object, Value},
};
use std::{collections::HashMap, sync::Arc};

use crate::property_set::PROPERTY_SET_DEP_TYPE_NAME;

// Member names: 'static string literals reborrow into the parse arena lifetime for free.
const DATA: &str = "data";
const DESC: &str = "desc";
const FORMAT: &str = "format";
const ID: &str = "id";
const LEVEL: &str = "level";
const MSG: &str = "msg";
const NAME: &str = "name";
const PROPERTIES: &str = "properties";
const TARGET: &str = "target";
const TIME: &str = "time";
const VALUE: &str = "value";

const PROPERTY_SET_TYPE_NAME: &str = "property_set";

fn parse_log_string_event<'a>(
    bump: &'a Bump,
    udt: &'a UserDefinedType,
    _udts: &'a [UserDefinedType],
    dependencies: &HashMap<u64, Value<'a>>,
    mut object_window: &'a [u8],
) -> Result<Value<'a>> {
    let desc_id: u64 = read_consume_pod(&mut object_window);
    let time: i64 = read_consume_pod(&mut object_window);
    // legacy format: the remaining bytes are the (utf8) message
    let msg = std::str::from_utf8(object_window).with_context(|| "parsing legacy string")?;
    let desc = *dependencies
        .get(&desc_id)
        .with_context(|| format!("desc member {desc_id} of LogStringEvent not found"))?;
    let members = bump.alloc_slice_copy(&[
        (TIME, Value::I64(time)),
        (MSG, Value::String(msg)),
        (DESC, desc),
    ]);
    Ok(Value::Object(bump.alloc(Object {
        type_name: udt.name.as_str(),
        members,
    })))
}

fn parse_log_string_event_v2<'a>(
    bump: &'a Bump,
    udt: &'a UserDefinedType,
    _udts: &'a [UserDefinedType],
    dependencies: &HashMap<u64, Value<'a>>,
    mut object_window: &'a [u8],
) -> Result<Value<'a>> {
    let desc_id: u64 = read_consume_pod(&mut object_window);
    let time: i64 = read_consume_pod(&mut object_window);
    let msg = read_advance_string_in(bump, &mut object_window).with_context(|| "parsing string")?;
    let desc = *dependencies
        .get(&desc_id)
        .with_context(|| format!("desc member {desc_id} of LogStringEvent not found"))?;
    let members = bump.alloc_slice_copy(&[
        (TIME, Value::I64(time)),
        (MSG, Value::String(msg)),
        (DESC, desc),
    ]);
    Ok(Value::Object(bump.alloc(Object {
        type_name: udt.name.as_str(),
        members,
    })))
}

fn parse_log_string_interop_event_v3<'a>(
    bump: &'a Bump,
    udt: &'a UserDefinedType,
    udts: &'a [UserDefinedType],
    dependencies: &HashMap<u64, Value<'a>>,
    mut object_window: &'a [u8],
) -> Result<Value<'a>> {
    let string_ref_metadata = udts
        .iter()
        .find(|t| *t.name == "StaticStringRef")
        .with_context(
            || "Can't parse log string interop event with no metadata for StaticStringRef",
        )?;
    let time: i64 = read_consume_pod(&mut object_window);
    let level: u8 = read_consume_pod(&mut object_window);
    let target = parse_pod_instance(
        bump,
        string_ref_metadata,
        udts,
        dependencies,
        &object_window[0..string_ref_metadata.size],
    )
    .with_context(|| "parse_pod_instance")?;
    object_window = advance_window(object_window, string_ref_metadata.size);
    let msg = read_advance_string_in(bump, &mut object_window)?;
    let members = bump.alloc_slice_copy(&[
        (TIME, Value::I64(time)),
        (LEVEL, Value::U8(level)),
        (TARGET, target),
        (MSG, Value::String(msg)),
    ]);
    Ok(Value::Object(bump.alloc(Object {
        type_name: udt.name.as_str(),
        members,
    })))
}

fn parse_tagged_log_interop_event<'a>(
    bump: &'a Bump,
    udt: &'a UserDefinedType,
    udts: &'a [UserDefinedType],
    dependencies: &HashMap<u64, Value<'a>>,
    mut object_window: &'a [u8],
) -> Result<Value<'a>> {
    let string_ref_metadata = udts
        .iter()
        .find(|t| *t.name == "StaticStringRef")
        .with_context(
            || "Can't parse log string interop event with no metadata for StaticStringRef",
        )?;
    let time: i64 = read_consume_pod(&mut object_window);
    let level: u8 = read_consume_pod(&mut object_window);
    let target = parse_pod_instance(
        bump,
        string_ref_metadata,
        udts,
        dependencies,
        &object_window[0..string_ref_metadata.size],
    )
    .with_context(|| "parse_pod_instance")?;
    object_window = advance_window(object_window, string_ref_metadata.size);
    let properties_id: u64 = read_consume_pod(&mut object_window);
    let properties = *dependencies
        .get(&properties_id)
        .with_context(|| "fetching properties in parse_tagged_log_interop_event")?;
    let msg = read_advance_string_in(bump, &mut object_window)?;
    let members = bump.alloc_slice_copy(&[
        (TIME, Value::I64(time)),
        (LEVEL, Value::U8(level)),
        (TARGET, target),
        (PROPERTIES, properties),
        (MSG, Value::String(msg)),
    ]);
    Ok(Value::Object(bump.alloc(Object {
        type_name: udt.name.as_str(),
        members,
    })))
}

fn parse_tagged_log_string<'a>(
    bump: &'a Bump,
    udt: &'a UserDefinedType,
    _udts: &'a [UserDefinedType],
    dependencies: &HashMap<u64, Value<'a>>,
    mut object_window: &'a [u8],
) -> Result<Value<'a>> {
    let desc_id: u64 = read_consume_pod(&mut object_window);
    let desc = *dependencies
        .get(&desc_id)
        .with_context(|| "fetching desc in parse_tagged_log_string")?;
    let properties_id: u64 = read_consume_pod(&mut object_window);
    let properties = *dependencies
        .get(&properties_id)
        .with_context(|| "fetching property set in parse_tagged_log_string")?;
    let time: i64 = read_consume_pod(&mut object_window);
    let msg = read_advance_string_in(bump, &mut object_window)?;

    let members = bump.alloc_slice_copy(&[
        (TIME, Value::I64(time)),
        (DESC, desc),
        (PROPERTIES, properties),
        (MSG, Value::String(msg)),
    ]);
    Ok(Value::Object(bump.alloc(Object {
        type_name: udt.name.as_str(),
        members,
    })))
}

fn parse_log_string_interop_event<'a>(
    bump: &'a Bump,
    udt: &'a UserDefinedType,
    udts: &'a [UserDefinedType],
    dependencies: &HashMap<u64, Value<'a>>,
    mut object_window: &'a [u8],
) -> Result<Value<'a>> {
    let stringid_metadata = udts
        .iter()
        .find(|t| *t.name == "StringId")
        .with_context(|| "Can't parse log string interop event with no metadata for StringId")?;
    let time: i64 = read_consume_pod(&mut object_window);
    let level: u32 = read_consume_pod(&mut object_window);
    let target = parse_pod_instance(
        bump,
        stringid_metadata,
        udts,
        dependencies,
        &object_window[0..stringid_metadata.size],
    )
    .with_context(|| "parse_pod_instance")?;
    object_window = advance_window(object_window, stringid_metadata.size);
    // legacy dyn string: the remaining bytes are the (utf8) message
    let msg =
        std::str::from_utf8(object_window).with_context(|| "parsing legacy interop string")?;
    let members = bump.alloc_slice_copy(&[
        (TIME, Value::I64(time)),
        (LEVEL, Value::U32(level)),
        (TARGET, target),
        (MSG, Value::String(msg)),
    ]);
    Ok(Value::Object(bump.alloc(Object {
        type_name: udt.name.as_str(),
        members,
    })))
}

fn parse_property_set<'a>(
    bump: &'a Bump,
    _udt: &'a UserDefinedType,
    udts: &'a [UserDefinedType],
    dependencies: &HashMap<u64, Value<'a>>,
    mut window: &'a [u8],
) -> Result<Value<'a>> {
    let property_layout = udts
        .iter()
        .find(|t| *t.name == "Property")
        .with_context(|| "could not find Property layout")?;

    let object_id: u64 = read_consume_pod(&mut window);
    let nb_properties = read_consume_pod::<u32>(&mut window) as usize;
    let property_size = property_layout.size;
    // Reject a corrupt count before reserving arena capacity: a real block holds
    // at most window.len()/property_size properties, so a bogus large count must
    // not trigger a huge (process-aborting) bump reservation.
    if property_size == 0 || nb_properties > window.len() / property_size {
        anyhow::bail!(
            "invalid property_set: nb_properties={nb_properties} exceeds {}-byte window",
            window.len()
        );
    }
    let mut members = bumpalo::collections::Vec::with_capacity_in(nb_properties, bump);
    for i in 0..nb_properties {
        let begin = i * property_size;
        let property_window = &window[begin..begin + property_size];
        if let Value::Object(obj) =
            parse_pod_instance(bump, property_layout, udts, dependencies, property_window)?
        {
            members.push((
                obj.get::<&str>("name")?,
                Value::String(obj.get::<&str>("value")?),
            ));
        } else {
            anyhow::bail!("invalid property in propertyset");
        }
    }

    let set: &Object = bump.alloc(Object {
        type_name: PROPERTY_SET_TYPE_NAME,
        members: members.into_bump_slice(),
    });
    let outer = bump.alloc_slice_copy(&[(ID, Value::U64(object_id)), (VALUE, Value::Object(set))]);
    Ok(Value::Object(bump.alloc(Object {
        type_name: PROPERTY_SET_DEP_TYPE_NAME.as_str(),
        members: outer,
    })))
}

fn parse_image_event<'a>(
    bump: &'a Bump,
    udt: &'a UserDefinedType,
    _udts: &'a [UserDefinedType],
    _dependencies: &HashMap<u64, Value<'a>>,
    mut object_window: &'a [u8],
) -> Result<Value<'a>> {
    let time: i64 = read_consume_pod(&mut object_window);
    let name =
        read_advance_string_in(bump, &mut object_window).with_context(|| "parsing image name")?;
    let format =
        read_advance_string_in(bump, &mut object_window).with_context(|| "parsing image format")?;
    let len: u32 = read_consume_pod(&mut object_window);
    // zero-copy: the blob borrows the (whole-block) source buffer; the consumer
    // copies it into Arrow inside the parse_block callback.
    let data: &[u8] = &object_window[..len as usize];
    let members = bump.alloc_slice_copy(&[
        (TIME, Value::I64(time)),
        (NAME, Value::String(name)),
        (FORMAT, Value::String(format)),
        (DATA, Value::Bytes(data)),
    ]);
    Ok(Value::Object(bump.alloc(Object {
        type_name: udt.name.as_str(),
        members,
    })))
}

/// Dictionnary of custom readers for dynamically sized events
pub fn make_custom_readers() -> CustomReaderMap {
    let mut custom_readers: CustomReaderMap = HashMap::new();
    custom_readers.insert("ImageEvent".into(), Arc::new(parse_image_event));
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
