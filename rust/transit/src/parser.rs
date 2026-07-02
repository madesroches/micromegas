use crate::{
    UserDefinedType, advance_window, read_advance_string_in, try_read_consume_pod, try_read_pod_at,
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
                let obj_size = try_read_pod_at::<u32>(buffer, offset)?;
                offset += std::mem::size_of::<u32>();
                obj_size as usize
            }
            static_size => static_size,
        };
        // Single guard: bounds every slice/advance derived from this object below
        // (offset..object_end) against the actual buffer length.
        let object_end = match offset.checked_add(object_size) {
            Some(end) if end <= buffer.len() => end,
            _ => bail!(
                "corrupt block: object at offset {offset} with size {object_size} exceeds {}-byte buffer",
                buffer.len()
            ),
        };

        match udt.name.as_str() {
            "StaticString" => {
                // zero-copy: the string borrows the (whole-block) source buffer.
                let string_id = try_read_pod_at::<u64>(buffer, offset)?;
                let nb_utf8_bytes = object_size
                    .checked_sub(std::mem::size_of::<usize>())
                    .with_context(|| {
                        format!("StaticString object_size {object_size} smaller than header")
                    })?;
                let str_start = offset + std::mem::size_of::<usize>();
                let s = std::str::from_utf8(&buffer[str_start..str_start + nb_utf8_bytes])
                    .with_context(|| "str::from_utf8")?;
                let insert_res = hash.insert(string_id, Value::String(s));
                if insert_res.is_some() {
                    bail!("duplicate dependency id {string_id}");
                }
            }
            "StaticStringDependency" => {
                // This window covers the entire remaining buffer, not just
                // object_size bytes, so the guard above doesn't bound what's
                // read here — the checked reads below do that.
                let mut window = advance_window(buffer, offset);
                let string_id: u64 = try_read_consume_pod(&mut window)?;
                let s =
                    read_advance_string_in(bump, &mut window).with_context(|| "parsing string")?;
                let insert_res = hash.insert(string_id, Value::String(s));
                if insert_res.is_some() {
                    bail!("duplicate dependency id {string_id}");
                }
            }
            type_name => {
                if let Some(reader) = custom_readers.get(type_name) {
                    // Same unbounded-window property as StaticStringDependency
                    // above: the reader validates every field it consumes.
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
                        if insert_res.is_some() {
                            bail!("duplicate dependency id {id}");
                        }
                    } else {
                        anyhow::bail!("custom dependency is not an object");
                    }
                } else {
                    if udt.size == 0 {
                        anyhow::bail!("invalid user-defined type {:?}", udt);
                    }
                    let instance =
                        parse_pod_instance(bump, udt, udts, &hash, &buffer[offset..object_end])
                            .with_context(|| "parse_pod_instance")?;
                    if let Value::Object(obj) = instance {
                        let id = obj.get::<u64>("id")?;
                        let insert_res = hash.insert(id, Value::Object(obj));
                        if insert_res.is_some() {
                            bail!("duplicate dependency id {id}");
                        }
                    }
                }
            }
        }
        offset = object_end;
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
        // Bounds the reference-key read and every intrinsic read below against
        // the object window (member metadata is untrusted, same as the payload).
        match member_meta.offset.checked_add(member_meta.size) {
            Some(end) if end <= object_window.len() => {}
            _ => bail!(
                "corrupt block: member {member_meta:?} exceeds {}-byte object window",
                object_window.len()
            ),
        }
        let value = if member_meta.is_reference {
            if member_meta.size < std::mem::size_of::<u64>() {
                bail!(
                    "member references have to be at least 8 bytes {:?}",
                    member_meta
                );
            }
            let key = try_read_pod_at::<u64>(object_window, member_meta.offset)?;
            if let Some(v) = dependencies.get(&key) {
                *v
            } else {
                bail!("dependency not found: member={member_meta:?} key={key}");
            }
        } else {
            match member_meta.type_name.as_str() {
                "u8" | "uint8" => {
                    if std::mem::size_of::<u8>() != member_meta.size {
                        bail!("type size mismatch for member {member_meta:?}");
                    }
                    Value::U8(try_read_pod_at::<u8>(object_window, member_meta.offset)?)
                }
                "u32" | "uint32" => {
                    if std::mem::size_of::<u32>() != member_meta.size {
                        bail!("type size mismatch for member {member_meta:?}");
                    }
                    Value::U32(try_read_pod_at::<u32>(object_window, member_meta.offset)?)
                }
                "u64" | "uint64" => {
                    if std::mem::size_of::<u64>() != member_meta.size {
                        bail!("type size mismatch for member {member_meta:?}");
                    }
                    Value::U64(try_read_pod_at::<u64>(object_window, member_meta.offset)?)
                }
                "i64" | "int64" => {
                    if std::mem::size_of::<i64>() != member_meta.size {
                        bail!("type size mismatch for member {member_meta:?}");
                    }
                    Value::I64(try_read_pod_at::<i64>(object_window, member_meta.offset)?)
                }
                "f64" => {
                    if std::mem::size_of::<f64>() != member_meta.size {
                        bail!("type size mismatch for member {member_meta:?}");
                    }
                    Value::F64(try_read_pod_at::<f64>(object_window, member_meta.offset)?)
                }
                non_intrinsic_member_type_name => {
                    if let Some(index) = udts
                        .iter()
                        .position(|t| *t.name == non_intrinsic_member_type_name)
                    {
                        let member_udt = &udts[index];
                        // member_udt.size may differ from member_meta.size, so it
                        // needs its own guard before slicing.
                        let udt_end = match member_meta.offset.checked_add(member_udt.size) {
                            Some(end) if end <= object_window.len() => end,
                            _ => bail!(
                                "corrupt block: nested member {member_meta:?} exceeds {}-byte object window",
                                object_window.len()
                            ),
                        };
                        parse_pod_instance(
                            bump,
                            member_udt,
                            udts,
                            dependencies,
                            &object_window[member_meta.offset..udt_end],
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
                let obj_size = try_read_pod_at::<u32>(buffer, offset)?;
                offset += std::mem::size_of::<u32>();
                (obj_size as usize, true)
            }
            static_size => (static_size, false),
        };
        let object_end = match offset.checked_add(object_size) {
            Some(end) if end <= buffer.len() => end,
            _ => bail!(
                "corrupt block: object at offset {offset} with size {object_size} exceeds {}-byte buffer",
                buffer.len()
            ),
        };
        let instance = if is_size_dynamic {
            parse_custom_instance(
                bump,
                custom_readers,
                udt,
                udts,
                dependencies,
                &buffer[offset..object_end],
            )
            .with_context(|| "parse_custom_instance")?
        } else {
            parse_pod_instance(bump, udt, udts, dependencies, &buffer[offset..object_end])
                .with_context(|| "parse_pod_instance")?
        };
        if !fun(instance)? {
            return Ok(false);
        }
        offset = object_end;
    }
    Ok(true)
}
