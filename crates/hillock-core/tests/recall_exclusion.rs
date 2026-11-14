//! `RecallInput::exclude_tags` — rows a host captured by itself leave a recall without
//! being deleted.
//!
//! Captured conversation turns are near-verbatim echoes of whatever is being asked
//! about, so they win on cosine and crowd out the memories someone chose to write down.

mod fixture;

use fixture::*;
use hillock_core::model::*;

const CAPTURED: &str = "cognitive_intake";

fn excluding(mut i: RecallInput) -> RecallInput {
    i.exclude_tags = vec![CAPTURED.to_string()];
    i
}

#[tokio::test]
async fn an_excluded_tag_drops_even_the_best_match_and_an_empty_list_drops_nothing() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("raw turn", 0.95);
    embedder.place("deliberate note", 0.60);
    let (engine, _) = engine_with(embedder, ScoringConfig::default()).await;
    let raw = remember(
        &engine,
        input_with(
            "raw turn about the build server",
            MemoryKind::Observation,
            &[CAPTURED],
            None,
        ),
    )
    .await;
    let note = remember(
        &engine,
        input_with(
            "deliberate note about the build server",
            MemoryKind::Fact,
            &["build"],
            None,
        ),
    )
    .await;

    assert_eq!(
        recall_ids(&engine, query(10)).await,
        vec![raw, note],
        "no exclusion: best cosine first"
    );
    assert_eq!(
        recall_ids(&engine, excluding(query(10))).await,
        vec![note],
        "excluded tag is dropped"
    );
}

/// The exclusion list is matched as written and as `remember` normalises tags, so a
/// caller does not have to know the stored spelling.
#[tokio::test]
async fn exclusion_matches_the_normalised_spelling_of_a_tag() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("raw turn", 0.95);
    let (engine, _) = engine_with(embedder, ScoringConfig::default()).await;
    remember(
        &engine,
        input_with("raw turn", MemoryKind::Observation, &[CAPTURED], None),
    )
    .await;

    let mut ask = query(10);
    ask.exclude_tags = vec!["Cognitive Intake".to_string()];
    assert!(recall_ids(&engine, ask).await.is_empty());
}

#[tokio::test]
async fn a_fragment_of_an_excluded_parent_is_not_returned() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("LONGRAW", 0.95);
    embedder.place("deliberate note", 0.60);
    let (engine, _) = engine_with(embedder, ScoringConfig::default()).await;
    let parent = engine
        .remember(
            AGENT,
            input_with(
                &long_content("LONGRAW"),
                MemoryKind::Observation,
                &[CAPTURED],
                None,
            ),
        )
        .await
        .unwrap();
    assert!(
        parent.chunk_count.unwrap() > 1,
        "the fixture must take the chunked path"
    );
    let note = remember(
        &engine,
        input_with("deliberate note", MemoryKind::Fact, &[], None),
    )
    .await;

    let open = engine.recall(AGENT, query(10)).await.unwrap();
    assert_eq!(
        open.results[0].parent_id,
        Some(parent.memory_id),
        "without exclusion a fragment wins"
    );
    assert_eq!(recall_ids(&engine, excluding(query(10))).await, vec![note]);
}

/// Exclusion is applied after the candidate pool is cut, exactly like a filter, so it
/// needs the wide pool for the same reason: with captured rows filling the narrow top-N
/// there would be nothing left to return.
#[tokio::test]
async fn exclusion_searches_the_wide_pool() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("raw turn", 0.95);
    embedder.place("deliberate note", 0.40);
    let scoring = ScoringConfig {
        oversample_factor: 1,
        min_candidates: 3,
        filtered_min_candidates: 50,
        ..Default::default()
    };
    let (engine, _) = engine_with(embedder, scoring).await;
    for i in 0..6 {
        remember(
            &engine,
            input_with(
                &format!("raw turn {i}"),
                MemoryKind::Observation,
                &[CAPTURED],
                None,
            ),
        )
        .await;
    }
    let note = remember(
        &engine,
        input_with("deliberate note", MemoryKind::Fact, &[], None),
    )
    .await;

    assert!(
        !recall_ids(&engine, query(2)).await.contains(&note),
        "outside the narrow pool without exclusion"
    );
    assert_eq!(recall_ids(&engine, excluding(query(2))).await, vec![note]);
}

#[tokio::test]
async fn exclusion_applies_to_keyword_candidates_under_every_strategy() {
    let (engine, _) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let raw = remember(
        &engine,
        input_with(
            "zebra quartz came up in chat",
            MemoryKind::Observation,
            &[CAPTURED],
            None,
        ),
    )
    .await;
    let note = remember(
        &engine,
        input_with("zebra quartz is the codename", MemoryKind::Fact, &[], None),
    )
    .await;

    for strategy in [
        SearchStrategy::CosineOnly,
        SearchStrategy::HybridBm25Inject,
        SearchStrategy::HybridRrf,
    ] {
        let ask = RecallInput {
            query: Some("zebra quartz".into()),
            search_strategy: strategy,
            ..query(10)
        };
        let open = recall_ids(&engine, ask.clone()).await;
        assert!(
            open.contains(&raw) && open.contains(&note),
            "{strategy:?} without exclusion: {open:?}"
        );
        assert_eq!(
            recall_ids(&engine, excluding(ask)).await,
            vec![note],
            "{strategy:?}"
        );
    }
}

#[tokio::test]
async fn direct_fetch_ignores_exclusion() {
    let (engine, _) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let raw = remember(
        &engine,
        input_with(
            "raw turn fetched by id",
            MemoryKind::Observation,
            &[CAPTURED],
            None,
        ),
    )
    .await;
    assert_eq!(recall_ids(&engine, excluding(fetch(raw))).await, vec![raw]);
}

/// A recall input written before the fields existed must still deserialize, with
/// nothing excluded and nothing reinforced.
#[test]
fn an_older_recall_input_still_parses_with_the_new_fields_off() {
    let old: RecallInput = serde_json::from_str(r#"{"query":"q","memory_id":null,"filter":null}"#)
        .expect("older input must parse");
    assert!(old.exclude_tags.is_empty());
    assert!(!old.reinforce);
}
