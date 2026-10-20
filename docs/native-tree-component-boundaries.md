# Native tree component boundary checks

The `twenty_native_tree_component_conformance_cases` test runs twenty distinct named input cases through Loom's native tree encoder. Ten accepted UTF-8 component names (`a`, `README`, `file.txt`, `A-b_1`, `space name`, `café`, `日本語`, `Δοκιμή`, `é`, `data.json`) must encode and decode byte-for-byte identically. Ten rejected inputs (``, `.`, `..`, `../escape`, `dir/file`, `dir\\file`, `.git`, `.loom`, `/absolute`, `a/../b`) must fail encoding.

This guards cross-platform path-component identity without silently normalizing Unicode or interpreting separators. The test runs in the standard Linux, macOS and Windows CI matrix. It is not a complete portability policy for Windows reserved device names, Unicode normalization, symlinks, executable bits or v1-to-v2 migration. Those remain open under issue #13.
