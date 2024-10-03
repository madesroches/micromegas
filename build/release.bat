pushd %~dp0..\rust

set PUBLISH_GRACE_SLEEP=60

cargo release -p micromegas-derive-transit -x --no-confirm
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-transit -x --no-confirm
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-tracing-proc-macros -x --no-confirm
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-tracing -x --no-confirm
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-telemetry -x --no-confirm
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-ingestion -x --no-confirm
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-telemetry-sink -x --no-confirm
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas-analytics -x --no-confirm
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

cargo release -p micromegas -x --no-confirm
@if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%

popd
