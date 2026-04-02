use brain_knowledge_rs::memory_store::MemoryStore;
use tempfile::tempdir;

#[tokio::test]
#[ignore] // Requires downloading fastembed models (~680MB)
async fn test_qmd_search_pipeline_real() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("memory.db");

    let mem_store = MemoryStore::open(&db_path).expect("memory store open");

    // Add sample markdown document
    let md_content = r#"
# Ilhae Agent Architecture
The architecture is based on Rust backend and React frontend.
We use Tauri to glue them together. 

## Memory Store
Memory store is a SQLite database using sqlite-vec for vector search.
It supports QMD's deep search pipeline with cross-encoder reranking.
    "#;

    let md_path = dir.path().join("architecture.md");
    std::fs::write(&md_path, md_content).expect("Failed to write md file");

    // MemoryStore::index_file implements Smart Chunking internally and saves to DB.
    mem_store
        .index_file(&md_path)
        .expect("Failed to index file");

    // Ensure chunks are embedded using multilingual-e5-small
    mem_store
        .embed_pending()
        .expect("Failed to embed pending chunks");

    // 1. Test Vector Search (vsearch)
    let v_results = mem_store
        .vsearch("rust backend react", 5)
        .expect("vsearch failed");
    assert!(
        !v_results.is_empty(),
        "vsearch should find semantic matches"
    );
    println!("vsearch top rank: {}", v_results[0].rank);

    // 2. Test Hybrid Search (RRF)
    let h_results = mem_store
        .hybrid_search("vector search sqlite", 5)
        .expect("hybrid_search failed");
    assert!(
        !h_results.is_empty(),
        "hybrid_search should find combined matches"
    );
    println!("hybrid top rank: {}", h_results[0].rank);

    // 3. Test Deep Search (Cross-Encoder Reranking + Blending)
    let d_results = mem_store
        .deep_search("how does memory store work?", 5)
        .expect("deep_search failed");
    assert!(
        !d_results.is_empty(),
        "deep_search should return highly relevant chunk"
    );
    println!("deep top rank (blended): {}", d_results[0].rank);

    // Since we queried 'memory store', the chunk regarding Memory Store should be ranked highest.
    assert!(
        d_results[0]
            .text
            .contains("Memory store is a SQLite database")
    );
}
