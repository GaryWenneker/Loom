# Pinned SQLite runtime

Loom pins `rusqlite` 0.39.0 with its `bundled` feature. The resulting
`libsqlite3-sys` 0.37.0 amalgamation is SQLite 3.51.3 with source ID:

```text
2026-03-13 10:38:09 737ae4a34738ffa0c3ff7f9bb18df914dd1cad163f28fd6b6e114a344fe6d618
```

SQLite 3.51.3 fixes the WAL-reset corruption race documented by the SQLite
project for concurrent writers and checkpoints. The exact dependency pin and
lockfile make the linked C source reproducible. The cross-platform test
`bundled_sqlite_runtime_contains_wal_reset_fix` checks both the runtime version
and `sqlite_source_id()`; a successful dependency audit by itself is not proof
that the fixed C runtime was linked.

This is a runtime hardening change, not a repository-format migration. Existing
SQLite object databases remain compatible. Loom still uses WAL,
`synchronous=FULL`, bounded atomic conditional object batches, fresh live
readback, all-replica verification and ref compare-and-swap. No additional
checkpoint concurrency or weaker durability setting is introduced.

The SQLite batch evidence workflow builds the exact PR base and head in
separate Cargo targets, runs the runtime proof, and compares identical complete
publication/read workflows. Its artifacts retain commit, tree, harness and
binary hashes. A performance change is reported only from those paired results;
this dependency upgrade makes no speedup claim by itself.

Primary references:

- [SQLite 3.51.3 release notes](https://sqlite.org/releaselog/3_51_3.html)
- [SQLite WAL documentation](https://sqlite.org/wal.html)
