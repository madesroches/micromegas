use micromegas_transit::{Utf8StaticStringDependency, prelude::*};

#[derive(Debug, TransitReflect)]
pub struct LogMsgEvent {
    pub level: i32,
    pub msg: &'static str,
}

impl InProcSerialize for LogMsgEvent {}

#[derive(Debug, TransitReflect)]
pub struct NullEvent {}

impl InProcSerialize for NullEvent {}

declare_queue_struct!(
    struct LogMsgQueue<LogMsgEvent, NullEvent> {}
);

declare_queue_struct!(
    struct DepQueue<Utf8StaticStringDependency> {}
);

#[test]
fn test_deps() {
    let mut q = LogMsgQueue::new(1024);
    q.push(LogMsgEvent {
        level: 0,
        msg: "test_msg",
    });
    q.push(NullEvent {});
    q.push(LogMsgEvent {
        level: 0,
        msg: "__test",
    });

    let reflection = LogMsgQueue::reflect_contained();
    println!("{:?}", reflection);

    let mut deps = DepQueue::new(1024);

    for x in q.iter() {
        match x {
            LogMsgQueueAny::LogMsgEvent(evt) => {
                deps.push(Utf8StaticStringDependency {
                    len: evt.msg.len() as u32,
                    ptr: evt.msg.as_ptr(),
                });
            }
            LogMsgQueueAny::NullEvent(_evt) => {}
        }
    }

    assert_eq!(40, deps.len_bytes());

    for x in deps.iter() {
        println!("{:?}", x);
    }
}
