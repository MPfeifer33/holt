//! Importance means something: it decays by kind, a deliberate recall reinforces it,
//! and it carries a small weight in ranking.
//!
//! Before: every kind decayed nightly towards the floor, facts and preferences
//! included, and ranking never read the number.

mod fixture;

use chrono::Utc;
use fixture::*;
use hillock_core::model::*;
use uuid::Uuid;

const ALL_KINDS: [MemoryKind; 6] = [
    MemoryKind::Observation,
    MemoryKind::Fact,
    MemoryKind::Decision,
    MemoryKind::Preference,
    MemoryKind::Constraint,
    MemoryKind::Procedure,
];

/// A memory old enough to be a decay candidate.
fn aged(content: &str, kind: MemoryKind, importance: Option<f32>) -> RememberInput {
    let mut i = input_with(content, kind, &[], importance);
    i.created_at = Some(Utc::now() - chrono::Duration::days(60));
    i
}

fn near(a: Option<f32>, b: f32) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-4)
}

// ── Decay by kind ───────────────────────────────────────────────────

#[tokio::test]
async fn only_observations_decay_by_default() {
    let (engine, _) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let mut ids = Vec::new();
    for kind in ALL_KINDS {
        let id = remember(&engine, aged(&format!("an old {kind:?}"), kind, Some(0.5))).await;
        ids.push((kind, id));
    }

    let out = engine
        .maintenance(AGENT, MaintenanceConfig::default())
        .await
        .unwrap();

    assert_eq!(out.memories_decayed, 1);
    for (kind, id) in ids {
        let expected = if kind == MemoryKind::Observation {
            0.49
        } else {
            0.5
        };
        assert!(
            near(importance_of(&engine, id).await, expected),
            "{kind:?} should be at {expected}"
        );
    }
}

#[tokio::test]
async fn the_kinds_that_decay_are_configurable() {
    let (engine, _) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let observation = remember(
        &engine,
        aged("an old observation", MemoryKind::Observation, Some(0.5)),
    )
    .await;
    let fact = remember(&engine, aged("an old fact", MemoryKind::Fact, Some(0.5))).await;

    let config = MaintenanceConfig {
        decay_kinds: vec![MemoryKind::Fact],
        ..Default::default()
    };
    engine.maintenance(AGENT, config).await.unwrap();
    assert!(near(importance_of(&engine, fact).await, 0.49));
    assert!(near(importance_of(&engine, observation).await, 0.5));

    // No kinds: nothing decays.
    let none = MaintenanceConfig {
        decay_kinds: vec![],
        decay_min_interval_hours: 0,
        ..Default::default()
    };
    assert_eq!(
        engine
            .maintenance(AGENT, none)
            .await
            .unwrap()
            .memories_decayed,
        0
    );
}

#[tokio::test]
async fn a_recently_recalled_memory_is_not_decayed() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("the one in use", 0.9);
    embedder.place("the one gathering dust", 0.1);
    let (engine, _) = engine_with(embedder, ScoringConfig::default()).await;
    let used = remember(
        &engine,
        aged("the one in use", MemoryKind::Observation, Some(0.5)),
    )
    .await;
    let dusty = remember(
        &engine,
        aged("the one gathering dust", MemoryKind::Observation, Some(0.5)),
    )
    .await;

    assert_eq!(recall_ids(&engine, query(1)).await, vec![used]);
    let after_recall = importance_of(&engine, used).await.unwrap();

    engine
        .maintenance(AGENT, MaintenanceConfig::default())
        .await
        .unwrap();
    assert!(
        near(importance_of(&engine, used).await, after_recall),
        "recalled within the grace window: holds"
    );
    assert!(
        near(importance_of(&engine, dusty).await, 0.49),
        "never recalled: decays"
    );

    // The exemption is the grace window, not an accident: lift it and the same memory decays.
    let no_grace = MaintenanceConfig {
        decay_recall_grace_days: 0,
        ..Default::default()
    };
    engine.maintenance(AGENT, no_grace).await.unwrap();
    assert!(near(
        importance_of(&engine, used).await,
        after_recall * 0.98
    ));
}

// ── Ranking ─────────────────────────────────────────────────────────

/// Two memories at chosen cosines and importances; returns both ids and the rank order.
async fn ranked_pair(
    scoring: ScoringConfig,
    a: (f32, Option<f32>),
    b: (f32, Option<f32>),
) -> (Uuid, Uuid, Vec<Uuid>) {
    let embedder = ScriptedEmbedder::new();
    embedder.place("memory alpha", a.0);
    embedder.place("memory beta", b.0);
    let (engine, _) = engine_with(embedder, scoring).await;
    let when = Utc::now() - chrono::Duration::days(3);
    let mut ia = input_with("memory alpha", MemoryKind::Fact, &[], a.1);
    ia.created_at = Some(when);
    let mut ib = input_with("memory beta", MemoryKind::Fact, &[], b.1);
    ib.created_at = Some(when);
    let alpha = remember(&engine, ia).await;
    let beta = remember(&engine, ib).await;
    (alpha, beta, recall_ids(&engine, query(10)).await)
}

#[tokio::test]
async fn with_equal_cosine_and_age_the_more_important_memory_ranks_first() {
    let (alpha, beta, order) =
        ranked_pair(ScoringConfig::default(), (0.8, Some(0.1)), (0.8, Some(0.9))).await;
    assert_eq!(order, vec![beta, alpha]);
}

/// Relevance still decides: the importance term tops out at its weight (0.03), and a
/// 0.05 cosine edge is worth 0.049 after the recency blend.
#[tokio::test]
async fn maximum_importance_does_not_overturn_a_clear_cosine_advantage() {
    let (alpha, beta, order) =
        ranked_pair(ScoringConfig::default(), (0.85, None), (0.80, Some(1.0))).await;
    assert_eq!(order, vec![alpha, beta]);
}

#[tokio::test]
async fn zero_importance_weight_reproduces_the_old_order() {
    // A 0.01 cosine edge: importance overturns it at the default weight...
    let (alpha, beta, order) = ranked_pair(
        ScoringConfig::default(),
        (0.81, Some(0.0)),
        (0.80, Some(1.0)),
    )
    .await;
    assert_eq!(order, vec![beta, alpha]);
    // ...and with the weight at zero the order is cosine's again.
    let off = ScoringConfig {
        importance_weight: 0.0,
        ..Default::default()
    };
    let (alpha, beta, order) = ranked_pair(off, (0.81, Some(0.0)), (0.80, Some(1.0))).await;
    assert_eq!(order, vec![alpha, beta]);
}

#[tokio::test]
async fn zero_importance_weight_leaves_scores_at_the_cosine_recency_blend() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("memory alpha", 0.8);
    let off = ScoringConfig {
        importance_weight: 0.0,
        ..Default::default()
    };
    let (engine, _) = engine_with(embedder, off).await;
    remember(
        &engine,
        input_with("memory alpha", MemoryKind::Fact, &[], Some(1.0)),
    )
    .await;
    let r = &engine.recall(AGENT, query(1)).await.unwrap().results[0];
    let expected = blend_score(
        r.cosine_score,
        recency_score(r.metadata.created_at, Utc::now()),
        0.02,
    );
    assert!(
        (r.score - expected).abs() < 1e-3,
        "score {} vs blend {expected}",
        r.score
    );
}

// ── Reinforcement on recall ─────────────────────────────────────────

#[tokio::test]
async fn recall_reinforces_a_result_once_per_interval() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("useful thing", 0.9);
    let (engine, _) = engine_with(embedder, ScoringConfig::default()).await;
    let id = remember(
        &engine,
        input_with("useful thing", MemoryKind::Fact, &[], Some(0.5)),
    )
    .await;

    recall_ids(&engine, query(5)).await;
    assert!(
        near(importance_of(&engine, id).await, 0.52),
        "first recall bumps by 0.02"
    );
    recall_ids(&engine, query(5)).await;
    recall_ids(&engine, query(5)).await;
    assert!(
        near(importance_of(&engine, id).await, 0.52),
        "no second bump inside the interval"
    );
}

/// Reinforcement is opt-in. Context packing, counting and evals read memory without
/// anyone having wanted the answer; they must not make what they touch look more
/// important. The default for a `RecallInput` is therefore off.
#[tokio::test]
async fn only_a_recall_that_asks_for_it_reinforces() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("useful thing", 0.9);
    let (engine, _) = engine_with(embedder, ScoringConfig::default()).await;
    let id = remember(
        &engine,
        input_with("useful thing", MemoryKind::Fact, &[], Some(0.5)),
    )
    .await;

    recall_ids(
        &engine,
        RecallInput {
            reinforce: false,
            ..query(5)
        },
    )
    .await;
    assert!(
        near(importance_of(&engine, id).await, 0.5),
        "a background read leaves importance alone"
    );

    // Context packing goes through the same recall and must stay read-only too.
    engine
        .pack_context(
            AGENT,
            PackContextInput {
                token_budget: 2000,
                query: Some(QUERY.to_string()),
                spaces: SpaceScope::Private,
                exclude_ids: vec![],
            },
        )
        .await
        .unwrap();
    assert!(
        near(importance_of(&engine, id).await, 0.5),
        "packing context leaves importance alone"
    );

    recall_ids(&engine, query(5)).await;
    assert!(
        near(importance_of(&engine, id).await, 0.52),
        "a deliberate recall counts"
    );
}

#[tokio::test]
async fn reinforcement_starts_unset_importance_at_the_floor_and_caps_at_one() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("unrated", 0.9);
    embedder.place("already vital", 0.8);
    let (engine, _) = engine_with(embedder, ScoringConfig::default()).await;
    let unrated = remember(&engine, input_with("unrated", MemoryKind::Fact, &[], None)).await;
    let vital = remember(
        &engine,
        input_with("already vital", MemoryKind::Fact, &[], Some(0.995)),
    )
    .await;

    recall_ids(&engine, query(5)).await;
    assert!(near(importance_of(&engine, unrated).await, 0.07));
    assert!(near(importance_of(&engine, vital).await, 1.0));
}

#[tokio::test]
async fn zero_reinforcement_makes_recall_read_only() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("useful thing", 0.9);
    let off = ScoringConfig {
        recall_reinforcement: 0.0,
        ..Default::default()
    };
    let (engine, _) = engine_with(embedder, off).await;
    let mut i = input_with("useful thing", MemoryKind::Observation, &[], Some(0.5));
    i.created_at = Some(Utc::now() - chrono::Duration::days(60));
    let id = remember(&engine, i).await;

    recall_ids(&engine, query(5)).await;
    assert!(near(importance_of(&engine, id).await, 0.5));
    // Nothing was stamped either: the memory is still an ordinary decay candidate.
    engine
        .maintenance(AGENT, MaintenanceConfig::default())
        .await
        .unwrap();
    assert!(near(importance_of(&engine, id).await, 0.49));
}

#[tokio::test]
async fn a_fragment_hit_reinforces_its_parent() {
    let embedder = ScriptedEmbedder::new();
    embedder.place("LONGDOC", 0.9);
    let (engine, store) = engine_with(embedder, ScoringConfig::default()).await;
    let parent = remember(
        &engine,
        input_with(&long_content("LONGDOC"), MemoryKind::Fact, &[], Some(0.5)),
    )
    .await;

    let hit = engine.recall(AGENT, query(5)).await.unwrap();
    assert_eq!(
        hit.results[0].parent_id,
        Some(parent),
        "the hit is a fragment"
    );

    assert!(near(importance_of(&engine, parent).await, 0.52));
    use hillock_core::store::MemoryStore as _;
    for fragment in store.get_chunks(parent).await.unwrap() {
        assert!(
            near(fragment.importance, 0.5),
            "fragments keep their own importance"
        );
    }
}

#[tokio::test]
async fn direct_fetch_does_not_reinforce() {
    let (engine, _) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let id = remember(
        &engine,
        input_with("fetched by id", MemoryKind::Fact, &[], Some(0.5)),
    )
    .await;
    recall_ids(&engine, fetch(id)).await;
    assert!(near(importance_of(&engine, id).await, 0.5));
}
