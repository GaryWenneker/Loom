# Security policy

## Current support status

Loom 0.1 is an **experimental alpha**. There is no production security-support or response-time guarantee. Do not make Loom the sole storage location for business-critical or confidential source code.

## Vulnerability disclosure

Prefer GitHub's private vulnerability reporting feature for this repository if available. Otherwise contact the maintainer using a privately verified channel. Do not publish proof-of-concept exploits, private code, access tokens, or sensitive paths in a public issue.

A useful report provides the affected revision, minimal reproduction, impact, affected platform/filesystem and suggested mitigation. Only test systems and data you are authorized to access.

## Threat model priorities

Important risk areas include immutable object publication, compromised object paths, symlink/hardlink traversal, recovery replay, corrupted changeset DAGs, memory exhaustion, and future repository authorization and remote object access.

Successful automated tests or an audit of dependencies do not establish production security. See the [engineering findings](docs/code-review.md) and [release gates](docs/release-gates.md).
