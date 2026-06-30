use crate::{
    UserDefinedType, advance_window, read_advance_string_in, read_any, read_consume_pod,
    value::{Object, Value},
};
use anyhow::{Context, Result, bail};
use bumpalo::Bump;
use std::{collections::HashMap, sync::Arc};

/// A reader for a custom (dynamically-sized) transit type.
///
/// Two higher-ranked lifetimes keep the returned `Value<'a>` (which borrows the
/// arena and source buffer) independent of the `&'dep` borrow of the dependency
/// map, so the caller can insert the result back into the map afterwards.
pub type CustomReader = Arc<
    dyn for<'a, 'dep> Fn(
        &'a Bump,
        &'a UserDefinedType,
        &'a [UserDefinedType],
        &'dep HashMap<u64, Value<'a>>,
        &'a [u8],
    ) -> Result<Value<'a>>,
>;
pub type CustomReaderMap = HashMap<String, CustomReader>;

pub fn read_dependencies<'a>(
    bump: &'a Bump,
    custom_readers: &CustomReaderMap,
    udts: &'a [UserDefinedType],
    buffer: &'a [u8],
) -> Result<HashMap<u64, Value<'a>>> {
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
                let obj_size = unsafe { read_any::<u32>(buffer.as_ptr().add(offset)) };
                offset += std::mem::size_of::<u32>();
                obj_size as usize
            }
            static_size => static_size,
        };

        match udt.name.as_str() {
            "StaticString" => {
                // zero-copy: the string borrows the (whole-block) source buffer.
                let string_id = unsafe { read_any::<u64>(buffer.as_ptr().add(offset)) };
                let nb_utf8_bytes = object_size - std::mem::size_of::<usize>();
                let str_start = offset + std::mem::size_of::<usize>();
                let s = std::str::from_utf8(&buffer[str_start..str_start + nb_utf8_bytes])
                    .with_context(|| "str::from_utf8")?;
                let insert_res = hash.insert(string_id, Value::String(s));
                assert!(insert_res.is_none());
            }
            "StaticStringDependency" => {
                let mut window = advance_window(buffer, offset);
                let string_id: u64 = read_consume_pod(&mut window);
                let s =
                    read_advance_string_in(bump, &mut window).with_context(|| "parsing string")?;
                let insert_res = hash.insert(string_id, Value::String(s));
                assert!(insert_res.is_none());
            }
            type_name => {
                if let Some(reader) = custom_readers.get(type_name) {
                    let window = advance_window(buffer, offset);
                    if let Value::Object(obj) = (*reader)(bump, udt, udts, &hash, window)
                        .with_context(|| "parsing custom dependency")?
                    {
                        let id: u64 = obj
                            .get("id")
                            .with_context(|| "reading id of custom dependency")?;
                        let value = *obj
                            .get_ref("value")
                            .with_context(|| "reading value of custom dependency")?;
                        let insert_res = hash.insert(id, value);
                        assert!(insert_res.is_none());
                    } else {
                        anyhow::bail!("custom dependency is not an object");
                    }
                } else {
                    if udt.size == 0 {
                        anyhow::bail!("invalid user-defined type {:?}", udt);
                    }
                    let instance = parse_pod_instance(
                        bump,
                        udt,
                        udts,
                        &hash,
                        &buffer[offset..offset + udt.size],
                    )
                    .with_context(|| "parse_pod_instance")?;
                    if let Value::Object(obj) = instance {
                        let insert_res = hash.insert(obj.get::<u64>("id")?, Value::Object(obj));
                        assert!(insert_res.is_none());
                    }
                }
            }
        }
        offset += object_size;
    }

    Ok(hash)
}

fn parse_custom_instance<'a>(
    bump: &'a Bump,
    custom_readers: &CustomReaderMap,
    udt: &'a UserDefinedType,
    udts: &'a [UserDefinedType],
    dependencies: &HashMap<u64, Value<'a>>,
    object_window: &'a [u8],
) -> Result<Value<'a>> {
    if let Some(reader) = custom_readers.get(&*udt.name) {
        (*reader)(bump, udt, udts, dependencies, object_window)
    } else {
        log::warn!("unknown custom object {}", &udt.name);
        Ok(Value::Object(bump.alloc(Object {
            type_name: udt.name.as_str(),
            members: &[],
        })))
    }
}

pub fn parse_pod_instance<'a>(
    bump: &'a Bump,
    udt: &'a UserDefinedType,
    udts: &'a [UserDefinedType],
    dependencies: &HashMap<u64, Value<'a>>,
    object_window: &'a [u8],
) -> Result<Value<'a>> {
    let mut members = bumpalo::collections::Vec::with_capacity_in(udt.members.len(), bump);
    for member_meta in &udt.members {
        let name: &'a str = member_meta.name.as_str();
        let value = if member_meta.is_reference {
            if member_meta.size < std::mem::size_of::<u64>() {
                bail!(
                    "member references have to be at least 8 bytes {:?}",
                    member_meta
                );
            }
            let key = unsafe { read_any::<u64>(object_window.as_ptr().add(member_meta.offset)) };
            if let Some(v) = dependencies.get(&key) {
                *v
            } else {
                bail!("dependency not found: member={member_meta:?} key={key}");
            }
        } else {
            match member_meta.type_name.as_str() {
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
                            bump,
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
            return Ok(members[id_index].1);
        }
        bail!("reference object has no 'id' member");
    }

    Ok(Value::Object(bump.alloc(Object {
        type_name: udt.name.as_str(),
        members: members.into_bump_slice(),
    })))
}

// parse_object_buffer calls fun for each object in the buffer until fun returns
// `false`
pub fn parse_object_buffer<'a, F>(
    bump: &'a Bump,
    custom_readers: &CustomReaderMap,
    dependencies: &HashMap<u64, Value<'a>>,
    udts: &'a [UserDefinedType],
    buffer: &'a [u8],
    mut fun: F,
) -> Result<bool>
where
    F: FnMut(Value<'a>) -> Result<bool>,
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
                let obj_size = unsafe { read_any::<u32>(buffer.as_ptr().add(offset)) };
                offset += std::mem::size_of::<u32>();
                (obj_size as usize, true)
            }
            static_size => (static_size, false),
        };
        let instance = if is_size_dynamic {
            parse_custom_instance(
                bump,
                custom_readers,
                udt,
                udts,
                dependencies,
                &buffer[offset..offset + object_size],
            )
            .with_context(|| "parse_custom_instance")?
        } else {
            parse_pod_instance(
                bump,
                udt,
                udts,
                dependencies,
                &buffer[offset..offset + udt.size],
            )
            .with_context(|| "parse_pod_instance")?
        };
        if !fun(instance)? {
            return Ok(false);
        }
        offset += object_size;
    }
    Ok(true)
}
