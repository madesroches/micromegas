#[cfg(windows)]
mod installation;

use anyhow::{Context, Result};
use micromegas::chrono::DateTime;
use micromegas::client::SpanTypes;
use micromegas::client::flightsql_client::Client;
use micromegas::client::perfetto_trace_client::write_perfetto_trace;
use micromegas::tonic::transport::{Channel, Uri};
use std::collections::HashMap;

fn help() {
    println!(
        r#"micromegas-uri-handler [command]
commands: 
  --help : prints this message
  --install : registers this executable as the micromegas:// uri handler
  write-perfetto-trace <process_id> <begin> <end> <out_filename>
"#
    );
}

fn pause() {
    println!("Press <Enter> to continue...");
    std::io::stdin().read_line(&mut String::new()).unwrap();
}

async fn execute_uri_command(str_uri: &str) -> Result<()> {
    let url = url::Url::parse(str_uri)?;
    let command = url.path().to_owned();
    let mut arguments: HashMap<String, String> = HashMap::new();
    for (k, v) in url.query_pairs() {
        arguments.insert(k.into(), v.into());
    }
    if command == "write-perfetto-trace" {
        return Box::pin(execute_command(&[
            command,
            arguments
                .get("process_id")
                .with_context(|| "missing process_id")?
                .to_owned(),
            arguments
                .get("begin")
                .with_context(|| "missing begin")?
                .to_owned(),
            arguments
                .get("end")
                .with_context(|| "missing end")?
                .to_owned(),
            arguments
                .get("out_filename")
                .with_context(|| "missing out_filename")?
                .to_owned(),
        ]))
        .await;
    }
    anyhow::bail!("unknown command {command}")
}

async fn execute_command(args: &[String]) -> Result<()> {
    if args[0] == "write-perfetto-trace" && args.len() == 5 {
        let process_id = &args[1];
        let begin = DateTime::parse_from_rfc3339(&args[2])?.into();
        let end = DateTime::parse_from_rfc3339(&args[3])?.into();
        let out_filename = &args[4];
        let flight_url = std::env::var("MICROMEGAS_FLIGHT_URL")
            .with_context(|| "error reading MICROMEGAS_FLIGHT_URL environment variable")?
            .parse::<Uri>()?;
        let channel = Channel::builder(flight_url).connect().await?;
        let mut client = Client::new(channel);
        return write_perfetto_trace(
            &mut client,
            process_id,
            begin,
            end,
            out_filename,
            SpanTypes::Both,
        )
        .await;
    }
    let uri_start = "micromegas:";
    if args[0].starts_with(uri_start)
        && let Err(e) = execute_uri_command(&args[0]).await
    {
        println!("{e:?}");
        pause();
    }

    println!("unrecognized command {}", args[0]);
    help();
    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        help();
        return;
    }

    #[cfg(any(windows, doc))]
    if args[1] == "--install" {
        unsafe {
            return installation::install().unwrap();
        }
    }

    if let Err(e) = execute_command(&args[1..args.len()]).await {
        println!("{e:?}");
    }
}
