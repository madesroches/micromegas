use crate::{
    advance_window, read_advance_string, read_any, read_consume_pod, InProcSerialize,
    LegacyDynString, UserDefinedType,
};
use anyhow::{bail, Context, Result};
use std::{collections::HashMap, hash::BuildHasher, sync::Arc};

#[derive(Debug, Clone)]
pub struct Object {
    pub type_name: String,
    pub members: Vec<(String, Value)>,
}

impl Object {
    pub fn get<T>(&self, member_name: &str) -> Result<T>
    where
        T: TransitValue,
    {
        for m in &self.members {
            if m.0 == member_name {
                return T::get(&m.1);
            }
        }
        bail!("member {} not found in {:?}", member_name, self);
    }

    pub fn get_ref(&self, member_name: &str) -> Result<&Value> {
        for m in &self.members {
            if m.0 == member_name {
                return Ok(&m.1);
            }
        }
        bail!("member {} not found", member_name);
    }
}

pub trait TransitValue {
    fn get(value: &Value) -> Result<Self>
    where
        Self: Sized;
}

impl TransitValue for u8 {
    fn get(value: &Value) -> Result<Self> {
        if let Value::U8(val) = value {
            Ok(*val)
        } else {
            bail!("bad type cast u8 for value {:?}", value);
        }
    }
}

impl TransitValue for u32 {
    fn get(value: &Value) -> Result<Self> {
        match value {
            Value::U32(val) => Ok(*val),
            Value::U8(val) => Ok(Self::from(*val)),
            _ => {
                bail!("bad type cast u32 for value {:?}", value);
            }
        }
    }
}

impl TransitValue for u64 {
    fn get(value: &Value) -> Result<Self> {
        match value {
            Value::I64(val) => Ok(*val as Self),
            Value::U64(val) => Ok(*val),
            _ => {
                bail!("bad type cast u64 for value {:?}", value)
            }
        }
    }
}

impl TransitValue for i64 {
    #[allow(clippy::cast_possible_wrap)]
    fn get(value: &Value) -> Result<Self> {
        match value {
            Value::I64(val) => Ok(*val),
            Value::U64(val) => Ok(*val as Self),
            _ => {
                bail!("bad type cast i64 for value {:?}", value)
            }
        }
    }
}

impl TransitValue for f64 {
    fn get(value: &Value) -> Result<Self> {
        if let Value::F64(val) = value {
            Ok(*val)
        } else {
            bail!("bad type cast f64 for value {:?}", value);
        }
    }
}

impl TransitValue for Arc<String> {
    fn get(value: &Value) -> Result<Self> {
        if let Value::String(val) = value {
            Ok(val.clone())
        } else {
            bail!("bad type cast String for value {:?}", value);
        }
    }
}

impl TransitValue for Arc<Object> {
    fn get(value: &Value) -> Result<Self> {
        if let Value::Object(val) = value {
            Ok(val.clone())
        } else {
            bail!("bad type cast String for value {:?}", value);
        }
    }
}

#[derive(Debug, Clone)]
pub enum Value {
    String(Arc<String>),
    Object(Arc<Object>),
    U8(u8),
    U32(u32),
    U64(u64),
    I64(i64),
    F64(f64),
    None,
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        if let Value::String(s) = &self {
            Some(s.as_str())
        } else {
            None
        }
    }
}

pub fn read_dependencies(udts: &[UserDefinedType], buffer: &[u8]) -> Result<HashMap<u64, Value>> {
    let mut hash = HashMap::new();
    let mut offset = 0;
    while offset < buffer.len() {
        let type_index = buffer[offset] as usize;
        if type_index >= udts.len() {
            bail!(
                "Invalid type index parsing transit dependencies: {}",
                type_index
            );
        }
        offset += 1;
        let udt = &udts[type_index];
        let object_size = match udt.size {
            0 => {
                //dynamic size
                unsafe {
                    let size_ptr = buffer.as_ptr().add(offset);
                    let obj_size = read_any::<u32>(size_ptr);
                    offset += std::mem::size_of::<u32>();
                    obj_size as usize
                }
            }
            static_size => static_size,
        };

        match udt.name.as_str() {
            "StaticString" => unsafe {
                let id_ptr = buffer.as_ptr().add(offset);
                let string_id = read_any::<u64>(id_ptr);
                let nb_utf8_bytes = object_size - std::mem::size_of::<usize>();
                let utf8_ptr = buffer.as_ptr().add(offset + std::mem::size_of::<usize>());
                let slice = std::ptr::slice_from_raw_parts(utf8_ptr, nb_utf8_bytes);
                let string =
                    String::from(std::str::from_utf8(&*slice).with_context(|| "str::from_utf8")?);
                let insert_res = hash.insert(string_id, Value::String(Arc::new(string)));
                assert!(insert_res.is_none());
            },
            "StaticStringDependency" => {
                let mut window = advance_window(buffer, offset);
                let string_id: u64 = read_consume_pod(&mut window);
                let string = read_advance_string(&mut window).with_context(|| "parsing string")?;

                let insert_res = hash.insert(string_id, Value::String(Arc::new(string)));
                assert!(insert_res.is_none());
            }

            _ => {
                if udt.size == 0 {
                    anyhow::bail!("invalid user-defined type {:?}", udt);
                }
                let instance =
                    parse_pod_instance(udt, udts, &hash, &buffer[offset..offset + udt.size])
                        .with_context(|| "parse_pod_instance")?;
                if let Value::Object(obj) = instance {
                    let insert_res = hash.insert(obj.get::<u64>("id")?, Value::Object(obj));
                    assert!(insert_res.is_none());
                }
            }
        }
        offset += object_size;
    }

    Ok(hash)
}

fn parse_log_string_event<S>(
    dependencies: &HashMap<u64, Value, S>,
    mut object_window: &[u8],
) -> Result<Vec<(String, Value)>>
where
    S: BuildHasher,
{
    let desc_id: u64 = read_consume_pod(&mut object_window);
    let time: i64 = read_consume_pod(&mut object_window);
    let msg = String::from_utf8(object_window.to_vec()).with_context(|| "parsing legacy string")?;
    let desc = dependencies
        .get(&desc_id)
        .with_context(|| format!("desc member {} of LogStringEvent not found", desc_id))?;
    Ok(vec![
        (String::from("time"), Value::I64(time)),
        (String::from("msg"), Value::String(Arc::new(msg))),
        (String::from("desc"), desc.clone()),
    ])
}

fn parse_log_string_event_v2<S>(
    dependencies: &HashMap<u64, Value, S>,
    mut object_window: &[u8],
) -> Result<Vec<(String, Value)>>
where
    S: BuildHasher,
{
    let desc_id: u64 = read_consume_pod(&mut object_window);
    let time: i64 = read_consume_pod(&mut object_window);
    let msg = read_advance_string(&mut object_window).with_context(|| "parsing string")?;
    let mut desc: Value = Value::None;
    if let Some(found_desc) = dependencies.get(&desc_id) {
        desc = found_desc.clone();
    } else {
        log::warn!("desc member {} of LogStringEvent not found", desc_id);
    }
    Ok(vec![
        (String::from("time"), Value::I64(time)),
        (String::from("msg"), Value::String(Arc::new(msg))),
        (String::from("desc"), desc),
    ])
}

fn parse_log_string_interop_event_v3<S>(
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value, S>,
    mut object_window: &[u8],
) -> Result<Vec<(String, Value)>>
where
    S: BuildHasher,
{
    if let Some(index) = udts.iter().position(|t| t.name == "StaticStringRef") {
        let string_ref_metadata = &udts[index];
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

        Ok(vec![
            (String::from("time"), Value::I64(time)),
            (String::from("level"), Value::U8(level)),
            (String::from("target"), target),
            (String::from("msg"), Value::String(Arc::new(msg))),
        ])
    } else {
        bail!("Can't parse log string interop event with no metadata for StaticStringRef");
    }
}

fn parse_log_string_interop_event<S>(
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value, S>,
    mut object_window: &[u8],
) -> Result<Vec<(String, Value)>>
where
    S: BuildHasher,
{
    if let Some(index) = udts.iter().position(|t| t.name == "StringId") {
        let stringid_metadata = &udts[index];
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

            Ok(vec![
                (String::from("time"), Value::I64(time)),
                (String::from("level"), Value::U32(level)),
                (String::from("target"), target),
                (String::from("msg"), Value::String(Arc::new(msg.0))),
            ])
        }
    } else {
        bail!("Can't parse log string interop event with no metadata for StringId");
    }
}

fn parse_custom_instance<S>(
    udt: &UserDefinedType,
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value, S>,
    object_window: &[u8],
) -> Result<Value>
where
    S: BuildHasher,
{
    let members = match udt.name.as_str() {
        // todo: move out of transit lib.
        // LogStringEvent belongs to the tracing lib
        // we need to inject the serialization logic of custom objects
        "LogStringEvent" => parse_log_string_event(dependencies, object_window)
            .with_context(|| "parse_log_string_event")?,
        "LogStringEventV2" => parse_log_string_event_v2(dependencies, object_window)
            .with_context(|| "parse_log_string_event_v2")?,
        "LogStringInteropEventV2" => {
            parse_log_string_interop_event(udts, dependencies, object_window)
                .with_context(|| "parse_log_string_interop_event")?
        }
        "LogStringInteropEventV3" => {
            parse_log_string_interop_event_v3(udts, dependencies, object_window)
                .with_context(|| "parse_log_string_interop_event_v3")?
        }
        other => {
            log::warn!("unknown custom object {}", other);
            Vec::new()
        }
    };
    Ok(Value::Object(Arc::new(Object {
        type_name: udt.name.clone(),
        members,
    })))
}

fn parse_pod_instance<S>(
    udt: &UserDefinedType,
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value, S>,
    object_window: &[u8],
) -> Result<Value>
where
    S: BuildHasher,
{
    let mut members: Vec<(String, Value)> = vec![];
    for member_meta in &udt.members {
        let name = member_meta.name.clone();
        let type_name = member_meta.type_name.clone();
        let value = if member_meta.is_reference {
            if member_meta.size < std::mem::size_of::<u64>() {
                bail!(
                    "member references have to be at least 8 bytes {:?}",
                    member_meta
                );
            }
            let key = unsafe { read_any::<u64>(object_window.as_ptr().add(member_meta.offset)) };
            if let Some(v) = dependencies.get(&key) {
                v.clone()
            } else {
                log::warn!("dependency not found: {}", key);
                Value::None
            }
        } else {
            match type_name.as_str() {
                "u8" | "uint8" => {
                    assert_eq!(std::mem::size_of::<u8>(), member_meta.size);
                    unsafe {
                        Value::U8(read_any::<u8>(
                            object_window.as_ptr().add(member_meta.offset),
                        ))
                    }
                }
                "u32" | "uint32" => {
                    assert_eq!(std::mem::size_of::<u32>(), member_meta.size);
                    unsafe {
                        Value::U32(read_any::<u32>(
                            object_window.as_ptr().add(member_meta.offset),
                        ))
                    }
                }
                "u64" | "uint64" => {
                    assert_eq!(std::mem::size_of::<u64>(), member_meta.size);
                    unsafe {
                        Value::U64(read_any::<u64>(
                            object_window.as_ptr().add(member_meta.offset),
                        ))
                    }
                }
                "i64" | "int64" => {
                    assert_eq!(std::mem::size_of::<i64>(), member_meta.size);
                    unsafe {
                        Value::I64(read_any::<i64>(
                            object_window.as_ptr().add(member_meta.offset),
                        ))
                    }
                }
                "f64" => {
                    assert_eq!(std::mem::size_of::<f64>(), member_meta.size);
                    unsafe {
                        Value::F64(read_any::<f64>(
                            object_window.as_ptr().add(member_meta.offset),
                        ))
                    }
                }
                non_intrinsic_member_type_name => {
                    if let Some(index) = udts
                        .iter()
                        .position(|t| t.name == non_intrinsic_member_type_name)
                    {
                        let member_udt = &udts[index];
                        parse_pod_instance(
                            member_udt,
                            udts,
                            dependencies,
                            &object_window
                                [member_meta.offset..member_meta.offset + member_udt.size],
                        )
                        .with_context(|| "parse_pod_instance")?
                    } else {
                        bail!("unknown member type {}", non_intrinsic_member_type_name);
                    }
                }
            }
        };
        members.push((name, value));
    }

    if udt.is_reference {
        // reference objects need a member called 'id' which is the key to the dependency
        if let Some(id_index) = members.iter().position(|m| m.0 == "id") {
            return Ok(members[id_index].1.clone());
        }
        bail!("reference object has no 'id' member");
    }

    Ok(Value::Object(Arc::new(Object {
        type_name: udt.name.clone(),
        members,
    })))
}

// parse_object_buffer calls fun for each object in the buffer until fun returns
// `false`
pub fn parse_object_buffer<F, S>(
    dependencies: &HashMap<u64, Value, S>,
    udts: &[UserDefinedType],
    buffer: &[u8],
    mut fun: F,
) -> Result<bool>
where
    F: FnMut(Value) -> Result<bool>,
    S: BuildHasher,
{
    let mut offset = 0;
    while offset < buffer.len() {
        let type_index = buffer[offset] as usize;
        if type_index >= udts.len() {
            bail!("Invalid type index parsing transit objects: {}", type_index);
        }
        offset += 1;
        let udt = &udts[type_index];
        let (object_size, is_size_dynamic) = match udt.size {
            0 => {
                //dynamic size
                unsafe {
                    let size_ptr = buffer.as_ptr().add(offset);
                    let obj_size = read_any::<u32>(size_ptr);
                    offset += std::mem::size_of::<u32>();
                    (obj_size as usize, true)
                }
            }
            static_size => (static_size, false),
        };
        let instance = if is_size_dynamic {
            parse_custom_instance(
                udt,
                udts,
                dependencies,
                &buffer[offset..offset + object_size],
            )
            .with_context(|| "parse_custom_instance")?
        } else {
            parse_pod_instance(udt, udts, dependencies, &buffer[offset..offset + udt.size])
                .with_context(|| "parse_pod_instance")?
        };
        if !fun(instance)? {
            return Ok(false);
        }
        offset += object_size;
    }
    Ok(true)
}
