pushd ..\rust
set PUBLISH_GRACE_SLEEP=2

cargo release -p micromegas-derive-transit
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-transit
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-tracing-proc-macros
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-tracing
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-telemetry
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-ingestion
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-analytics
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-telemetry-sink
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

popd
