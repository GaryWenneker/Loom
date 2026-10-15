# Cloud storage provider roadmap

## Supported target environments

- **AWS:** Amazon S3 for immutable objects; DynamoDB conditional writes, RDS PostgreSQL or another proven CAS-capable authority for mutable refs. IAM roles / workload identity, SSE-KMS and bucket policies.
- **Azure:** Azure Blob Storage for immutable objects; Azure Database for PostgreSQL or another CAS-capable ref authority. Managed identity, RBAC and encryption with Key Vault integration.
- **Google Cloud:** Google Cloud Storage for immutable objects; Cloud SQL PostgreSQL or Spanner for refs. Workload identity and IAM.
- **S3-compatible:** MinIO, Ceph RGW and compatible endpoints, subject to a verified conditional-create and consistency profile.
- **Hybrid:** local filesystem/SQLite for offline work and cloud object backends for remote durable replication. Offline synchronization requires explicit divergence handling, not silent last-writer-wins.

## What is implemented now

`cloud_storage::CloudObjectLocation` validates provider, namespace, key prefix and optional HTTPS S3-compatible origin. It produces the same canonical relative object key across providers. **No AWS, Azure, Google Cloud or S3 network adapter is implemented yet.** No credentials are accepted or transmitted by this module. The current repository engine still persists locally.

## Adapter acceptance requirements

1. Auth via workload identity/managed identity or short-lived credentials, not plaintext access keys in repository metadata.
2. Verified canonical BLAKE3 object envelope and idempotent immutable writes; reject different content at an existing object ID.
3. Conditional object publication and explicit consistency profile; do not rely on bucket listing for correctness.
4. Streaming/range reads, retries with exponential backoff, bounded memory, quotas and cancellation.
5. CAS-backed ref authority independent from object storage, with complete closure verification before ref publication.
6. Provider conformance tests against real AWS/Azure/GCP services or controlled emulators, including partitions, duplicate requests, timeouts, credential expiry and lost acknowledgements.
7. Cross-provider backup/restore and migration with object ID preservation and raw benchmark results.

Cloud configuration alone is not a working cloud datastore. Implement network drivers and wire them through the storage interfaces before declaring a provider supported.
