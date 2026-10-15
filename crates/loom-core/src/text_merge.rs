//! Conservative, deterministic three-way line merge.
//! Returns None rather than guessing about overlapping changes or large inputs.

#[derive(Debug, PartialEq, Eq)]
struct Hunk<'a> {
    start: usize,
    end: usize,
    inserted: Vec<&'a str>,
}

fn hunks<'a>(base: &[&str], changed: &[&'a str]) -> Option<Vec<Hunk<'a>>> {
    let n = base.len();
    let m = changed.len();
    let width = m.checked_add(1)?;
    let cells = n.checked_add(1)?.checked_mul(width)?;
    // Bound both worst-case memory and CPU (O(n*m)).
    if cells > 1_000_000 { return None; }
    let mut table = vec![0_u32; cells];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * width + j] = if base[i] == changed[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }

    let mut result = Vec::new();
    let mut current: Option<Hunk<'a>> = None;
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && base[i] == changed[j] {
            if let Some(hunk) = current.take() { result.push(hunk); }
            i += 1;
            j += 1;
        } else {
            let hunk = current.get_or_insert_with(|| Hunk { start: i, end: i, inserted: Vec::new() });
            if j < m && (i == n || table[i * width + j + 1] >= table[(i + 1) * width + j]) {
                hunk.inserted.push(changed[j]);
                j += 1;
            } else {
                i += 1;
                hunk.end = i;
            }
        }
    }
    if let Some(hunk) = current { result.push(hunk); }
    Some(result)
}

fn overlap(a: &Hunk<'_>, b: &Hunk<'_>) -> bool {
    // Treat insertions exactly at a modified range boundary as ambiguous.
    a.start <= b.end && b.start <= a.end
}

/// Merge independently modified UTF-8 line sequences, refusing ambiguous hunks.
/// Binary or oversized inputs are never auto-merged.
pub(crate) fn merge_text(base: &[u8], ours: &[u8], theirs: &[u8]) -> Option<Vec<u8>> {
    if base.len().saturating_add(ours.len()).saturating_add(theirs.len()) > 1_500_000 {
        return None;
    }
    let b = std::str::from_utf8(base).ok()?;
    let o = std::str::from_utf8(ours).ok()?;
    let t = std::str::from_utf8(theirs).ok()?;
    if b.contains('\0') || o.contains('\0') || t.contains('\0') { return None; }
    let base_lines: Vec<_> = b.split_inclusive('\n').collect();
    let our_lines: Vec<_> = o.split_inclusive('\n').collect();
    let their_lines: Vec<_> = t.split_inclusive('\n').collect();
    let mut edits = hunks(&base_lines, &our_lines)?;
    edits.extend(hunks(&base_lines, &their_lines)?);
    edits.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.end.cmp(&b.end)));
    let mut unique: Vec<Hunk<'_>> = Vec::new();
    for edit in edits {
        if let Some(last) = unique.last() {
            if overlap(last, &edit) {
                if last == &edit { continue; }
                return None;
            }
        }
        unique.push(edit);
    }

    let mut merged = String::new();
    let mut pos = 0;
    for edit in unique {
        if edit.start < pos { return None; }
        for line in &base_lines[pos..edit.start] { merged.push_str(line); }
        for line in edit.inserted { merged.push_str(line); }
        pos = edit.end;
    }
    for line in &base_lines[pos..] { merged.push_str(line); }
    Some(merged.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::merge_text;

    #[test]
    fn merges_different_lines() {
        let base = b"one\ntwo\nthree\nfour\n";
        let ours = b"ONE\ntwo\nthree\nfour\n";
        let theirs = b"one\ntwo\nthree\nFOUR\n";
        assert_eq!(merge_text(base, ours, theirs).unwrap(), b"ONE\ntwo\nthree\nFOUR\n");
    }

    #[test]
    fn refuses_overlapping_changes_and_binary() {
        assert!(merge_text(b"x\n", b"a\n", b"b\n").is_none());
        assert!(merge_text(b"\0\n", b"a\n", b"b\n").is_none());
        assert!(merge_text(b"x\n", b"\xff", b"b\n").is_none());
    }

    #[test]
    fn identical_edits_are_not_duplicated() {
        assert_eq!(merge_text(b"x\ny\n", b"z\ny\n", b"z\ny\n").unwrap(), b"z\ny\n");
    }

    #[test]
    fn append_and_edit_nonoverlapping_ranges() {
        assert_eq!(merge_text(b"a\nb\nc\n", b"A\nb\nc\n", b"a\nb\nc\nd\n").unwrap(), b"A\nb\nc\nd\n");
    }

    #[test]
    fn empty_and_newline_only_changes() {
        assert_eq!(merge_text(b"", b"hello\n", b""), Some(b"hello\n".to_vec()));
        assert!(merge_text(b"x", b"x\n", b"z").is_none());
    }
}
