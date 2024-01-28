# how to generate new test data

 - Open a console
 - Point environment variable LEGION_TELEMETRY_INGESTION_SRC_DATA_DIRECTORY to a non-existent or empty directory
```
> SET LEGION_TELEMETRY_INGESTION_SRC_DATA_DIRECTORY=d:\temp\telemetry
```
 - start the telemetry ingestion server directory
```
> cd server\telemetry-ingestion-srv
> cargo run --release
```
 - make sure that telemetry.db3 has been created in the data directory
 
 - open a new console
 
 - enable telemetry by specifying the url in the environment variable LGN_TELEMETRY_URL
 
```
> SET LGN_TELEMETRY_URL=http://localhost:8080
```

 - generate data by running instrumented unit tests in this shell
 
```
> cargo test -p lsc --release
```

 - go back to the first console terminal and kill the ingestion server using `CTRL-C`

 - cleanup sqlite telemetry database
 
Because of the impolite way in which we terminate the server the database probably still has temporary files. Look in the data directory, there could be `telemetry.db3-shm` and `telemetry.db3-wal` in addition to the expected `telemetry.db3`.

```
> cd %LEGION_TELEMETRY_INGESTION_SRC_DATA_DIRECTORY%
> sqlite3 telemetry.db3
sqlite> select count(*) from blocks;
sqlite> <CTRL-Z> <ENTER>
```
 - `telemetry.db3-shm` and `telemetry.db3-wal` should be gone
 - copy `telemetry.db3` to `legion\crates\lgn-telemetry-admin-cli\tests\data`
