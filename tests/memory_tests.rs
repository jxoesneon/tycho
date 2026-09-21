//! Memory layer tests: conversation history and MemPalace client stub.

use rust_voice_assistant::memory::{ConversationHistory, MemPalaceClient};

#[test]
fn test_conversation_history_push_and_format() {
    let mut history = ConversationHistory::new(3);
    assert!(history.is_empty());
    assert_eq!(history.len(), 0);

    history.push("user", "hello");
    history.push("tycho", "hi there");
    assert_eq!(history.len(), 2);
    assert!(!history.is_empty());
    assert_eq!(history.formatted_dialogue(), "user: hello\ntycho: hi there");
}

#[test]
fn test_conversation_history_eviction() {
    let mut history = ConversationHistory::new(2);
    history.push("user", "one");
    history.push("tycho", "two");
    history.push("user", "three");

    assert_eq!(history.len(), 2);
    let dialogue = history.formatted_dialogue();
    assert!(!dialogue.contains("one"));
    assert!(dialogue.contains("two"));
    assert!(dialogue.contains("three"));
}

#[tokio::test]
async fn test_mempalace_persistence() {
    let dir = std::env::temp_dir().join(format!("tycho-mem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("nested").join("store.jsonl");

    let client = MemPalaceClient::new(&db);
    assert_eq!(client.db_path, db);

    // Missing store reads as empty.
    assert!(client.query_recent("", 5).await.unwrap().is_empty());

    // store() creates parents and appends JSONL records.
    for text in ["user: first", "tycho: second", "user: third"] {
        client.store(text).await.unwrap();
    }
    let all = client.query_recent("", 10).await.unwrap();
    assert_eq!(all, vec!["user: first", "tycho: second", "user: third"]);

    // Substring filtering and recency limiting (newest kept, chrono order).
    assert_eq!(client.query_recent("user", 10).await.unwrap().len(), 2);
    let last_two = client.query_recent("", 2).await.unwrap();
    assert_eq!(last_two, vec!["tycho: second", "user: third"]);

    // A second client over the same file sees prior state (persistence).
    let reopen = MemPalaceClient::new(&db);
    assert_eq!(
        reopen.query_recent("", 1).await.unwrap(),
        vec!["user: third"]
    );

    // Corrupt lines are skipped, not fatal.
    let mut f = std::fs::OpenOptions::new().append(true).open(&db).unwrap();
    use std::io::Write;
    writeln!(f, "{{not valid json").unwrap();
    drop(f);
    assert_eq!(
        reopen.query_recent("", 1).await.unwrap(),
        vec!["user: third"]
    );

    // A db_path that is itself a directory fails on open, honestly.
    let dir_db = MemPalaceClient::new(&dir);
    assert!(dir_db.store("x").await.is_err());

    // Unwritable location (a path component is a file) fails honestly.
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"file").unwrap();
    let bad = MemPalaceClient::new(blocker.join("inner").join("s.jsonl"));
    assert!(bad.store("x").await.is_err());
    assert!(bad.query_recent("", 1).await.is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mempalace_query_relevant() {
    let dir = std::env::temp_dir().join(format!("tycho-memrel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("store.jsonl");
    let client = MemPalaceClient::new(&db);

    // Missing store reads as empty.
    assert!(client.query_relevant("rust", 5).await.unwrap().is_empty());

    for text in [
        "user: tell me about rust lifetimes",
        "tycho: lifetimes track borrow validity",
        "user: what is the weather",
        "tycho: I cannot check the weather",
        "user: explain rust ownership",
        "tycho: ownership is rust's memory model",
    ] {
        client.store(text).await.unwrap();
    }

    // Relevance-ranked: only records actually containing "rust" hit.
    let hits = client.query_relevant("rust", 10).await.unwrap();
    assert_eq!(hits.len(), 3);
    assert!(hits.iter().all(|t| t.to_lowercase().contains("rust")));

    // Limit applies; the tighter- matched record survives.
    let hits = client
        .query_relevant("rust ownership model", 2)
        .await
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|t| t.contains("ownership")));

    // No overlap -> no hits.
    assert!(client
        .query_relevant("kubernetes", 5)
        .await
        .unwrap()
        .is_empty());

    // Chronological order in output despite relevance ranking.
    let hits = client.query_relevant("rust", 10).await.unwrap();
    let first = hits[0].clone();
    assert!(first.contains("lifetimes"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_mempalace_rotation_cap() {
    let dir = std::env::temp_dir().join(format!("tycho-memrot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("store.jsonl");
    let client = MemPalaceClient::new(&db);

    // Fill past the 1000-record cap; rotation keeps the newest half.
    for i in 0..1002 {
        client.store(&format!("user: turn {i}")).await.unwrap();
    }
    let all = client.query_recent("", 5000).await.unwrap();
    assert_eq!(all.len(), 501, "rotated to newest half + the new record");
    assert_eq!(all[0], "user: turn 501");
    assert_eq!(all.last().unwrap(), "user: turn 1001");

    let _ = std::fs::remove_dir_all(&dir);
}
