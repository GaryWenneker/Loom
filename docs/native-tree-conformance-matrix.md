# Native tree serialization conformance cases

The Rust unit tests in `crates/loom-core/src/tree.rs` include ten explicit cases, `conformance_01` through `conformance_10`:

| Case | Invariant |
| --- | --- |
| 01 | Empty root roundtrip |
| 02 | Single regular file roundtrip |
| 03 | UTF-8 component roundtrip |
| 04 | Case-distinct entries preserve identity |
| 05 | Duplicate names rejected |
| 06 | Noncanonical ordering rejected |
| 07 | Backslash path components rejected |
| 08 | Executable bit on directories rejected |
| 09 | Trailing serialized bytes rejected |
| 10 | Truncated serialized bytes rejected |

These checks run through the standard Linux, macOS and Windows Rust CI matrix. They are regression cases for the current native tree codec, **not** an independent conformance implementation or golden vectors across separate runtimes. They do not establish full portable v2 metadata or migration support. See issue #13 for the remaining format and migration requirements.
