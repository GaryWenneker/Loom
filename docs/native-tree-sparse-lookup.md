# Sparse native tree path lookup

`tree::lookup_file(store, root, path)` resolves a file object ID by walking only the immutable directory nodes along a validated path. Each directory uses binary search over canonical sorted entries; unrelated sibling trees and blob payloads are not loaded. Missing files or paths that traverse a regular file return `None`. Invalid paths, malformed directory nodes, or missing nodes on the selected path return an error.

This is a **core building block** for on-demand checkout and sparse workspaces, not yet a user-facing sparse checkout or network cache. Complexity is proportional to the number and size of the directories on the selected path rather than the number of tracked files in the entire repository. A corrupt *unrelated* subtree is intentionally not traversed; full integrity checking remains a separate operation. The helper is currently crate-private and requires integration into a client-facing operation before users can request individual paths.

Follow-up work for issues #6, #13 and #26 includes bounded node caches, remote fetch, path-scoped authorization and native workspace APIs.
