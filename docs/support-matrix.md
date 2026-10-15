# Platform and capability support

**Loom v0.1:** experimental native local alpha. Supported here means available in merged source, not production-certified.

| Capability | Current support | Limitations |
|---|---|---|
| Windows, Linux and macOS | Tested by CI | Durability across filesystems not certified |
| Local init, status, commits, log | Basic | Worktree scans and first-parent history |
| Immutable staged file snapshots | Basic | Individual paths and persistent opt-in mode |
| `review`, staged/unstaged diff | Available | Text patches have size limits |
| Indexed current-branch file and prefix listing | Experimental | Branch-tip data uses a derived path index with history fallback |
| Historical native single-file retrieval | Experimental | Exact changeset IDs with result cache |
| Historical tree path listing | Experimental | Lazy rebuildable SQLite snapshots with ancestor checkpoint reuse |
| Canonical immutable directory-tree objects | Experimental | Versioned v2 tree nodes can be built on demand from v1 history; no full v2 migration, historical modes or sparse remote APIs |
| Branch create, list, switch | Basic | Untracked/dirty worktrees block switching |
| Three-way text merge | Limited | No interactive conflict continuation |
| Object, branch, staging and pending-recovery integrity checks | Basic | Detects missing/corrupt references; not a backup or comprehensive repair |
| Process-crash recovery | Partial | Selected branch switch and merge operations |
| Regular files under 64 MiB | Basic | No manifests or streaming file storage |
| Symlinks, modes, special filenames | Not supported | No metadata preservation guarantee |
| Tags, revert, reset, rebase, stash | Not supported | Planned |
| Native sync server and remote CLI | Not supported | Planned |
| Git import/export | Not supported | Planned adapter |
| Sparse checkout, node caches | Not supported | Planned |
| Production-grade disaster recovery | Not supported | Do not use as sole repository copy |

For architecture targets, see the [roadmap](roadmap.md) and [Scion assessment](scion-architecture-review.md).
