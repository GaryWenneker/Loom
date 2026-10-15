// Tree vector fixtures.

#[test]
fn native_tree_v2_empty_vector() {
    let bytes = crate::tree::NativeTree { entries: vec![] }.encode().unwrap();
    assert_eq!(bytes.len(), 19);
}
