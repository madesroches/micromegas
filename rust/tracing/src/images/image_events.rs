use micromegas_transit::{
    DynBlob, DynString, UserDefinedType, prelude::*, read_advance_string, read_consume_pod,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct ImageEvent {
    pub time: i64,
    pub name: DynString,
    pub format: DynString,
    pub data: DynBlob,
}

impl InProcSerialize for ImageEvent {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        Some(
            std::mem::size_of::<i64>() as u32
                + self.name.get_value_size().expect("name size")
                + self.format.get_value_size().expect("format size")
                + self.data.get_value_size().expect("data size"),
        )
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        write_any(buffer, &self.time);
        self.name.write_value(buffer);
        self.format.write_value(buffer);
        self.data.write_value(buffer);
    }

    #[allow(unsafe_code)]
    unsafe fn read_value(mut window: &[u8]) -> Self {
        let time: i64 = read_consume_pod(&mut window);
        let name = DynString(read_advance_string(&mut window).expect("reading image event name"));
        let format =
            DynString(read_advance_string(&mut window).expect("reading image event format"));
        let len: u32 = read_consume_pod(&mut window);
        let data = DynBlob(window[..len as usize].to_vec());
        Self {
            time,
            name,
            format,
            data,
        }
    }
}

impl Reflect for ImageEvent {
    fn reflect() -> UserDefinedType {
        lazy_static::lazy_static! {
            static ref TYPE_NAME: Arc<String> = Arc::new("ImageEvent".into());
        }
        UserDefinedType {
            name: TYPE_NAME.clone(),
            size: 0,
            members: vec![],
            is_reference: false,
            secondary_udts: vec![],
        }
    }
}
