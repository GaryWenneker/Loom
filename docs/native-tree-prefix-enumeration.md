# Native tree prefix-scoped enumeration

`tree::list_prefix(store, root, prefix)` walks only the immutable tree nodes required to locate a directory prefix, then recursively enumerates files within that selected subtree. It returns canonical full paths and blob IDs sorted by path. A prefix naming a file returns that one file. An absent prefix returns an empty map. The empty prefix enumerates the entire tree.

Unrelated directory subtrees are not opened. This permits path-local queries even if unrelated tree objects have not yet been fetched, an important building block for sparse workspaces. Invalid prefixes, malformed selected nodes and missing selected objects fail closed. Full enumeration still traverses all files and is not bounded or paginated; it must not be exposed to untrusted clients without a quota/pagination layer.

This is an internal primitive, not yet a public CLI feature or a complete sparse workspace. See issues #6 and #26.
