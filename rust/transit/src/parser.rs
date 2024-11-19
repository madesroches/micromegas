use crate::{
    advance_window, read_advance_string, read_any, read_consume_pod,
    value::{Object, Value},
    UserDefinedType,
};
use anyhow::{bail, Context, Result};
use lazy_static::lazy_static;
use std::{collections::HashMap, sync::Arc};

fn parse_property_set(
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value>,
    mut window: &[u8],
) -> Result<(u64, Value)> {
    let property_layout = udts
        .iter()
        .find(|t| *t.name == "Property")
        .with_context(|| "could not find Property layout")?;

    let object_id: u64 = read_consume_pod(&mut window);
    let nb_properties: u32 = read_consume_pod(&mut window);
    let mut members = vec![];
    for i in 0..nb_properties {
        let property_size = property_layout.size as usize;
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
            bail!("invalid property in propertyset");
        }
    }

    lazy_static! {
        static ref PROPERTY_SET_TYPE_NAME: Arc<String> = Arc::new("property_set".into());
    }

    let set = Value::Object(Arc::new(Object {
        type_name: PROPERTY_SET_TYPE_NAME.clone(),
        members,
    }));
    Ok((object_id, set))
}

// todo: move the parsing dynamically-sized deps from tracing to that crate
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
            "PropertySetDependency" => {
                let window = advance_window(buffer, offset);
                let (id, value) = parse_property_set(udts, &hash, window)?;
                let insert_res = hash.insert(id, value);
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

pub type CustomReader =
    Arc<dyn Fn(&UserDefinedType, &[UserDefinedType], &HashMap<u64, Value>, &[u8]) -> Result<Value>>;
pub type CustomReaderMap = HashMap<String, CustomReader>;

fn parse_custom_instance(
    custom_readers: &CustomReaderMap,
    udt: &UserDefinedType,
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value>,
    object_window: &[u8],
) -> Result<Value> {
    if let Some(reader) = custom_readers.get(&*udt.name) {
        (*reader)(udt, udts, dependencies, object_window)
    } else {
        log::warn!("unknown custom object {}", &udt.name);
        Ok(Value::Object(Arc::new(Object {
            type_name: udt.name.clone(),
            members: vec![],
        })))
    }
}

pub fn parse_pod_instance(
    udt: &UserDefinedType,
    udts: &[UserDefinedType],
    dependencies: &HashMap<u64, Value>,
    object_window: &[u8],
) -> Result<Value> {
    let mut members: Vec<(Arc<String>, Value)> = vec![];
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
                bail!("dependency not found: member={member_meta:?} key={key}");
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
                        .position(|t| *t.name == non_intrinsic_member_type_name)
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
        if let Some(id_index) = members.iter().position(|m| *m.0 == "id") {
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
pub fn parse_object_buffer<F>(
    custom_readers: &CustomReaderMap,
    dependencies: &HashMap<u64, Value>,
    udts: &[UserDefinedType],
    buffer: &[u8],
    mut fun: F,
) -> Result<bool>
where
    F: FnMut(Value) -> Result<bool>,
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
                &custom_readers,
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
