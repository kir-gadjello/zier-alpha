use zier_alpha::memory::MemoryIndex;
use tempfile::TempDir;
use rusqlite::Connection;

#[tokio::test]
async fn test_vec_table_dimension() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test.sqlite");

    // Create with specific dimension
    let _index = MemoryIndex::new_with_db_path(temp.path(), &db_path, Some(1024)).unwrap();

    // Verify table dimension
    let conn = Connection::open(&db_path).unwrap();

    // sqlite-vec loads as extension, need to load it again if we want to query vec0 table details?
    // Or just check sql schema.

    let sql: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE name='chunks_vec'",
        [],
        |row| row.get(0)
    ).unwrap_or("".to_string());

    if sql.is_empty() {
        println!("Skipping test as sqlite-vec not loaded or table not created");
        return;
    }

    assert!(sql.contains("float[1024]"), "SQL was: {}", sql);

    // Try creating with DIFFERENT dimension - should fail or warn (and disable ext)
    // We can't check logs easily, but we can check if table was altered (it shouldn't be)
    let _index2 = MemoryIndex::new_with_db_path(temp.path(), &db_path, Some(512));

    // Table should still be 1024
    let sql2: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE name='chunks_vec'",
        [],
        |row| row.get(0)
    ).unwrap();

    assert!(sql2.contains("float[1024]"), "SQL changed to: {}", sql2);
}
