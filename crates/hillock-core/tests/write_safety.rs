//! Writes that cannot leave the store half-changed.
//!
//! - Re-remembering text whose row was superseded used to return the hidden row's id and
//!   leave it hidden, dropping the new tags, importance and `supersedes` on the floor.
//! - A long memory's parent row was written before its fragments were embedded, so an
//!   embedding failure stranded a parent that owned the content hash forever.

mod fixture;

use chrono::Utc;
use fixture::*;
use hillock_core::model::*;
use hillock_core::store::MemoryStore as _;

async fn belief_of(engine: &hillock_core::Engine, id: uuid::Uuid) -> Option<BeliefState> {
    engine.get_memory(id).await.unwrap().belief_state
}

// ── Exact-match remember ────────────────────────────────────────────

#[tokio::test]
async fn a_first_write_is_not_reported_as_a_duplicate() {
    let (engine, _) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let first = engine
        .remember(AGENT, input("the server listens on 8080"))
        .await
        .unwrap();
    assert!(!first.deduplicated);

    // Same text, different case and surrounding whitespace: the hash is over the
    // trimmed, lowercased content, so this is the same memory.
    let again = engine
        .remember(AGENT, input("  The server listens on 8080 "))
        .await
        .unwrap();
    assert!(again.deduplicated, "the caller is told nothing was written");
    assert_eq!(again.memory_id, first.memory_id);
    assert_eq!(store_count(&engine).await, 1, "and nothing was");
}

async fn store_count(engine: &hillock_core::Engine) -> u64 {
    engine.inspect(AGENT).await.unwrap().private_count
}

#[tokio::test]
async fn remembering_superseded_text_again_revives_the_row() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("the deploy runs at dusk", 0.9);
    let (engine, _) = engine_with(embedder, ScoringConfig::default()).await;
    let original = remember(&engine, input("the deploy runs at dusk")).await;
    let mut replacement = input("it moved: now it runs at dawn");
    replacement.supersedes = vec![original];
    remember(&engine, replacement).await;
    assert!(
        !recall_ids(&engine, query(10)).await.contains(&original),
        "superseded: hidden"
    );

    let again = engine
        .remember(AGENT, input("the deploy runs at dusk"))
        .await
        .unwrap();

    assert_eq!(again.memory_id, original, "same row, not a new one");
    assert!(
        again.deduplicated,
        "no row was written, and the caller is told so"
    );
    assert_eq!(
        belief_of(&engine, original).await,
        Some(BeliefState::Active)
    );
    assert!(
        recall_ids(&engine, query(10)).await.contains(&original),
        "revived: recallable again"
    );
}

#[tokio::test]
async fn an_exact_match_merges_tags_and_keeps_the_higher_importance() {
    let (engine, _) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let id = remember(
        &engine,
        input_with(
            "same words",
            MemoryKind::Fact,
            &["first", "shared"],
            Some(0.4),
        ),
    )
    .await;

    let again = engine
        .remember(
            AGENT,
            input_with(
                "same words",
                MemoryKind::Fact,
                &["second", "shared"],
                Some(0.8),
            ),
        )
        .await
        .unwrap();
    assert_eq!(again.memory_id, id);
    assert!(again.deduplicated);
    let merged = engine.get_memory(id).await.unwrap();
    assert_eq!(merged.tags, vec!["first", "second", "shared"]);
    assert_eq!(merged.importance, Some(0.8), "raised to the new value");

    engine
        .remember(
            AGENT,
            input_with("same words", MemoryKind::Fact, &[], Some(0.1)),
        )
        .await
        .unwrap();
    let kept = engine.get_memory(id).await.unwrap();
    assert_eq!(kept.importance, Some(0.8), "never lowered");
    assert_eq!(
        kept.tags,
        vec!["first", "second", "shared"],
        "nothing removed"
    );
}

#[tokio::test]
async fn an_exact_match_carries_its_tags_and_importance_to_fragments() {
    let (engine, store) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let text = long_content("LONGDOC");
    let parent = remember(
        &engine,
        input_with(&text, MemoryKind::Fact, &["first"], Some(0.4)),
    )
    .await;

    engine
        .remember(
            AGENT,
            input_with(&text, MemoryKind::Fact, &["second"], Some(0.8)),
        )
        .await
        .unwrap();

    for fragment in store.get_chunks(parent).await.unwrap() {
        assert_eq!(fragment.tags, vec!["first", "second"]);
        assert_eq!(fragment.importance, Some(0.8));
    }
}

#[tokio::test]
async fn an_exact_match_still_applies_supersedes_but_never_to_itself() {
    let (engine, _) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let kept = remember(&engine, input("the current belief")).await;
    let stale = remember(&engine, input("the stale belief")).await;

    let mut again = input("the current belief");
    again.supersedes = vec![stale, kept];
    let out = engine.remember(AGENT, again).await.unwrap();

    assert_eq!(out.memory_id, kept);
    assert_eq!(
        belief_of(&engine, stale).await,
        Some(BeliefState::Superseded)
    );
    assert_eq!(
        belief_of(&engine, kept).await,
        Some(BeliefState::Active),
        "a memory cannot supersede itself"
    );
}

// ── Atomic long write ───────────────────────────────────────────────

#[tokio::test]
async fn a_failed_fragment_embedding_leaves_no_rows() {
    let embedder = ScriptedEmbedder::new();
    let (engine, store) = engine_with(embedder.clone(), ScoringConfig::default()).await;
    let text = long_content("LONGDOC");

    embedder.fail_batches(true);
    assert!(engine.remember(AGENT, input(&text)).await.is_err());
    assert_eq!(
        store.count_memories(SPACE).await.unwrap(),
        0,
        "no stranded parent"
    );
    assert_eq!(
        store
            .find_by_content_hash(SPACE, &content_hash(&text))
            .await
            .unwrap(),
        None
    );

    // And the retry is a real write, not a dedup onto a parent with no fragments.
    embedder.fail_batches(false);
    let retry = engine.remember(AGENT, input(&text)).await.unwrap();
    assert!(!retry.deduplicated);
    let fragments = retry.chunk_count.expect("long content is chunked") as u64;
    assert_eq!(store.count_memories(SPACE).await.unwrap(), fragments + 1);
}

/// One fragment that cannot be written takes the parent and its siblings with it.
#[tokio::test]
async fn a_batch_insert_is_all_or_nothing() {
    let (engine, store) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let seed = remember(&engine, input("a memory that already exists")).await;
    let existing = engine.get_memory(seed).await.unwrap();

    let row = |content: &str| Memory {
        id: uuid::Uuid::new_v4(),
        content: content.to_string(),
        content_hash: content_hash(content),
        embedding: None,
        ..existing.clone()
    };
    let good_one = row("first new row");
    let good_two = row("second new row");
    // Same primary key as a row already in the table: this insert must fail.
    let clash = Memory {
        id: seed,
        ..row("third new row")
    };

    let err = store
        .insert_memories(&[good_one.clone(), good_two.clone(), clash])
        .await;
    assert!(err.is_err(), "the clashing row must fail the batch");
    assert_eq!(
        store.count_memories(SPACE).await.unwrap(),
        1,
        "nothing from the failed batch was kept"
    );
    assert!(store.get_memory(good_one.id).await.is_err());

    store
        .insert_memories(&[good_one.clone(), good_two])
        .await
        .unwrap();
    assert_eq!(store.count_memories(SPACE).await.unwrap(), 3);
    // The space's own counter moved with the rows, not ahead of them.
    assert_eq!(store.get_space(SPACE).await.unwrap().memory_count, 3);
}

// ── Conflict candidates ─────────────────────────────────────────────

#[tokio::test]
async fn conflict_candidates_carry_a_preview_and_the_real_creation_time() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("twin", 0.9);
    let (engine, _) = engine_with(embedder, ScoringConfig::default()).await;
    let written = Utc::now() - chrono::Duration::days(30);
    // Multi-byte characters across the cut: a byte-indexed preview would panic or split one.
    let content = format!("twin {}", "é".repeat(400));
    let mut first = input(&content);
    first.created_at = Some(written);
    let first = remember(&engine, first).await;

    let out = engine
        .remember(AGENT, input("twin, the short one"))
        .await
        .unwrap();

    let candidate = out
        .near_duplicates
        .iter()
        .find(|c| c.memory_id == first)
        .expect("the twin is a near-duplicate");
    assert!(
        candidate.content_preview.starts_with("twin ééé"),
        "{}",
        candidate.content_preview
    );
    let shown = candidate.content_preview.chars().count();
    assert!(
        (100..=161).contains(&shown),
        "about 160 characters, got {shown}"
    );
    assert_eq!(candidate.created_at.timestamp(), written.timestamp());
}
