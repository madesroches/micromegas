# Object Storage

Micromegas keeps raw telemetry payloads and materialized Parquet views in an object store. Every
service that touches the lake selects it with `MICROMEGAS_OBJECT_STORE_URI`; the other URI
settings below use the same URI forms and the same credential rules.

| Variable | Used by |
|---|---|
| `MICROMEGAS_OBJECT_STORE_URI` | Ingestion, FlightSQL, maintenance daemon, monolith: the data lake |
| `MICROMEGAS_OBJECT_CACHE_ORIGIN_URI` | [Object cache](object-cache.md): a bucket-only origin (no path after the bucket) |
| `MICROMEGAS_MAPS_OBJECT_STORE_URI` | [Web app](web-app.md#maps): the Map cell catalog (read and write) |
| `MICROMEGAS_STATIC_TABLES_URL` | [FlightSQL](flight-sql.md) and [maintenance](maintenance.md): static lookup tables (read) |

## Backends

| Backend | URI form | Example |
|---|---|---|
| Local filesystem | `file://` | `file:///var/lib/micromegas/lake` |
| AWS S3 | `s3://bucket/prefix` | `s3://my-bucket/lake` |
| Google Cloud Storage | `gs://bucket/prefix` | `gs://my-bucket/lake` |
| Azure Blob Storage | `az://container/prefix` (account from `AZURE_STORAGE_ACCOUNT_NAME`) | `az://my-container/lake` |
| Azure Blob Storage | `abfss://container@account.dfs.core.windows.net/prefix` | `abfss://my-container@myaccount.dfs.core.windows.net/lake` |
| Azure Blob Storage | `https://account.blob.core.windows.net/container/prefix` | `https://myaccount.blob.core.windows.net/my-container/lake` |
| S3-compatible (MinIO, Cloudflare R2, Ceph, ...) | `s3://` plus `AWS_ENDPOINT` | see [below](#s3-compatible-stores) |

The `file://` backend is for local development. Everything after the bucket or container name
is the lake root: all objects are stored under it.

## Credentials

Options are read from the process environment: each variable name is lowercased and handed to the
[`object_store`](https://docs.rs/object_store) builder for the backend as an option key. The
standard variables for each cloud work, including instance and managed identity, so a service
running on EC2, ECS, GCE, GKE or Azure needs no explicit key.

| Backend | Variables |
|---|---|
| AWS S3 | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_REGION`, container and instance role credentials |
| GCS | `GOOGLE_SERVICE_ACCOUNT`, `GOOGLE_SERVICE_ACCOUNT_KEY`, `GOOGLE_APPLICATION_CREDENTIALS`; otherwise the GCE/GKE metadata server |
| Azure | `AZURE_STORAGE_ACCOUNT_NAME`, `AZURE_STORAGE_ACCOUNT_KEY`, `AZURE_STORAGE_SAS_KEY`, `AZURE_CLIENT_ID` / `AZURE_CLIENT_SECRET` / `AZURE_TENANT_ID`, workload identity, or managed identity |

See the builder documentation for the full list of keys:
[`AmazonS3Builder`](https://docs.rs/object_store/latest/object_store/aws/struct.AmazonS3Builder.html),
[`GoogleCloudStorageBuilder`](https://docs.rs/object_store/latest/object_store/gcp/struct.GoogleCloudStorageBuilder.html),
[`MicrosoftAzureBuilder`](https://docs.rs/object_store/latest/object_store/azure/struct.MicrosoftAzureBuilder.html).

!!! warning "Unprefixed variable names also match"
    `object_store` also accepts unprefixed alias names from the environment (Azure: `token`,
    `endpoint`, `client_id`, `tenant_id`, `access_key`, `account_name`; GCS: `bucket`, `base_url`,
    `service_account`). A generic `TOKEN` or `ENDPOINT` variable in the process environment can
    therefore reconfigure the store. Keep the environment of Micromegas services free of such names.

## Required permissions

The process credentials need read, write, delete and list on the configured prefix. For S3 that is
the equivalent of `s3:GetObject`, `s3:PutObject`, `s3:DeleteObject` and `s3:ListBucket`; GCS and
Azure have equivalent roles. Services that only read the store (for example the object cache origin
or static tables) need read and list only.

## Create-only writes

The lake requires an object store that supports conditional put (`PutMode::Create`): block payload
objects are stored at deterministic paths with a **create-only** write (first write wins; a
colliding write is rejected, not applied). This is why plain `http` or WebDAV stores are not
supported.

AWS S3, GCS and Azure enforce create-only writes natively, so nothing needs to be verified for
them. An S3-compatible store explicitly configured with `aws_conditional_put=disabled` will fail
every block write rather than silently falling back to overwrite.

## S3-compatible stores

For MinIO, Cloudflare R2, Ceph and similar, use an `s3://` URI and point the client at the
endpoint with `AWS_ENDPOINT`. Add `AWS_ALLOW_HTTP=true` for a plain HTTP endpoint.

Before depending on a new S3-compatible endpoint, verify it actually enforces
conditional put: write a key, write different bytes to the same key, read it
back, and confirm either an `AlreadyExists` error on the second write, or (if
it succeeded) that the read still returns the *first* write's bytes. If
neither holds, the store does not honor conditional put and the write-once
guarantee does not hold against it.

**Caveat**: a store that *accepts* `If-None-Match: *` but doesn't enforce it
(returns 200 and overwrites regardless) will make `put_if_absent` return
`Created` on every call: no error, no log line, so the write-once invariant
silently degrades to a plain overwrite. There is no code-level way to detect
this; it must be verified operationally with the procedure above before
depending on the store.
