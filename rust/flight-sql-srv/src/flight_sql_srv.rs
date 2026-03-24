use clap::Parser;
use micromegas::micromegas_main;
use micromegas::servers::flight_sql_server::FlightSqlServer;

#[derive(Parser, Debug)]
#[clap(name = "Micromegas FlightSQL server")]
#[clap(about = "Micromegas FlightSQL server", version, author)]
struct Cli {
    #[clap(long)]
    disable_auth: bool,
}

#[micromegas_main(interop_max_level = "info", max_level_override = "debug")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    let mut builder = FlightSqlServer::builder();

    if !args.disable_auth {
        builder = builder.with_default_auth();
    }

    builder.build_and_serve().await?;
    Ok(())
}
