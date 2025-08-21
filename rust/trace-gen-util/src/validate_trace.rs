use anyhow::Result;
use micromegas_perfetto::protos::Trace;
use prost::Message;
use std::env;
use std::fs;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <trace-file>", args[0]);
        std::process::exit(1);
    }
    
    let trace_file = &args[1];
    println!("Validating Perfetto trace: {}", trace_file);
    
    let data = fs::read(trace_file)?;
    println!("Trace file size: {} bytes", data.len());
    
    let trace = Trace::decode(&data[..])?;
    println!("Trace packets: {}", trace.packet.len());
    
    let mut process_descriptors = 0;
    let mut thread_descriptors = 0;
    let mut async_track_descriptors = 0;
    let mut track_events = 0;
    
    for packet in &trace.packet {
        if let Some(data) = &packet.data {
            match data {
                micromegas_perfetto::protos::trace_packet::Data::TrackDescriptor(track) => {
                    if track.process.is_some() {
                        process_descriptors += 1;
                    } else if track.thread.is_some() {
                        thread_descriptors += 1;
                    } else if track.static_or_dynamic_name.is_some() {
                        async_track_descriptors += 1;
                    }
                }
                micromegas_perfetto::protos::trace_packet::Data::TrackEvent(_) => {
                    track_events += 1;
                }
                _ => {}
            }
        }
    }
    
    println!("Process descriptors: {}", process_descriptors);
    println!("Thread descriptors: {}", thread_descriptors);
    println!("Async track descriptors: {}", async_track_descriptors);
    println!("Track events: {}", track_events);
    
    println!("✅ Trace validation successful!");
    
    Ok(())
}