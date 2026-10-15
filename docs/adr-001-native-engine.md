# ADR-001: Native version control engine

**Status:** Accepted for experimental Loom format v1

## Decision

Implement Loom in Rust with immutable, BLAKE3-addressed native objects and a first-parent delta model. Do not store Git objects or call Git commands from the core. Use SQLite for a local compare-and-swap reference store.

## Rationale

The core can evolve independently of Git, supporting lightweight clients and incremental changesets. Content-addressed immutable data can be cached while mutable branch pointers remain transactional.

## Consequences

Native data format compatibility needs its own migration strategy. File modes, renames, sparse workspaces, chunking, typed trees and multiwriter server coordination are follow-on work. First-parent delta replay has linear-history costs until index checkpointing ships. The initial objectstore uses loose objects and is not suitable for very large monorepos.

## Key risks

The filesystem write boundary and the SQLite transaction cannot form a single true distributed transaction. The implementation persists objects before refs and tolerates orphan objects; GC must preserve new uploads and avoid race-induced deletion.

The initial local workspace checkout is not crash-atomic. Branch commit and history safety do not imply workspace recovery safety.
