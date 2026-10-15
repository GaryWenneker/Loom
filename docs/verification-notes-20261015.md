# Loom verification notes

Native trees require verified closure before publication.

This note documents a test-only verification milestone.

A canonical tree has a stable binary envelope.

## Format validation

The native v2 tree envelope has a 5-byte magic, a one-byte version, a one-byte type, an eight-byte little-endian length and a bounded payload. Empty root serialization is 19 bytes. Future conformance fixtures must compare literal bytes across platforms, not only round-trip a single codec.
