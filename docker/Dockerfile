FROM ubuntu:latest

RUN apt-get update
RUN apt-get upgrade -y
RUN apt-get install ca-certificates -y

COPY telemetry-ingestion-srv .
RUN chmod +x /telemetry-ingestion-srv

COPY flight-sql-srv .
RUN chmod +x /flight-sql-srv

COPY telemetry-admin .
RUN chmod +x /telemetry-admin

COPY pg-gateway-srv .
RUN chmod +x /pg-gateway-srv
