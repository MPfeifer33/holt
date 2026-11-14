//! Deleting a memory takes its pins with it.
//!
//! `pinned_memories` names a memory by id and was not cleaned on delete, so a forgotten
//! or pruned memory went on holding one of its agent's few pin slots.

mod fixture;

use chrono::Utc;
use fixture::*;
use hillock_core::model::*;
use hillock_core::ops::pin::MAX_PINS;
use hillock_core::store::MemoryStore as _;
use uuid::Uuid;

#[tokio::test]
async fn forgetting_a_memory_removes_its_pin() {
    let (engine, store) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let doomed = remember(&engine, input("to be forgotten")).await;
    let kept = remember(&engine, input("to be kept")).await;
    assert!(engine.pin(AGENT, doomed).await.unwrap().pinned);
    assert!(engine.pin(AGENT, kept).await.unwrap().pinned);
    // A second agent's pin on the same memory goes too: the memory is gone for everyone.
    store.pin_memory("someone-else", doomed).await.unwrap();

    engine
        .forget(AGENT, ForgetInput { memory_id: doomed })
        .await
        .unwrap();

    assert_eq!(
        store.get_pinned(AGENT).await.unwrap(),
        vec![kept],
        "the pin went with the memory, and only that pin"
    );
    assert!(store.get_pinned("someone-else").await.unwrap().is_empty());
}

#[tokio::test]
async fn forgetting_a_long_memory_removes_its_pin_too() {
    let (engine, store) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let parent = remember(&engine, input(&long_content("LONGDOC"))).await;
    assert!(
        !store.get_chunks(parent).await.unwrap().is_empty(),
        "the fixture must take the chunked path"
    );
    engine.pin(AGENT, parent).await.unwrap();

    engine
        .forget(AGENT, ForgetInput { memory_id: parent })
        .await
        .unwrap();

    assert!(store.get_pinned(AGENT).await.unwrap().is_empty());
}

/// Deleting something that is not there changes nothing, including the keyword index
/// of the rows that are.
#[tokio::test]
async fn a_delete_that_finds_nothing_leaves_the_store_untouched() {
    let (engine, store) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let kept = remember(&engine, input("walrus tambourine")).await;

    assert!(store.delete_memory(Uuid::new_v4()).await.is_err());

    let hits = store
        .fts_search("walrus", &[SPACE.to_string()], 10)
        .await
        .unwrap();
    assert_eq!(
        hits.iter().map(|h| h.memory_id).collect::<Vec<_>>(),
        vec![kept]
    );
}

/// Pruning deletes through the same store calls, so a pruned memory cannot leave a pin
/// behind either. (Pinned memories are never pruned for their own agent; this is the
/// other agent's pin.)
#[tokio::test]
async fn a_pruned_memory_does_not_leave_another_agents_pin_behind() {
    let (engine, store) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let mut old = input_with("faded long ago", MemoryKind::Observation, &[], Some(0.01));
    old.created_at = Some(Utc::now() - chrono::Duration::days(90));
    let faded = remember(&engine, old).await;
    store.pin_memory("someone-else", faded).await.unwrap();

    let out = engine
        .maintenance(AGENT, MaintenanceConfig::default())
        .await
        .unwrap();

    assert_eq!(out.memories_pruned, 1);
    assert!(store.get_pinned("someone-else").await.unwrap().is_empty());
}

#[tokio::test]
async fn maintenance_removes_dangling_pins_and_reports_them() {
    let (engine, store) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let real = remember(&engine, input("a pinned memory that exists")).await;
    engine.pin(AGENT, real).await.unwrap();
    // The store pins whatever id it is given; this is the row a delete used to leave behind.
    let ghost = Uuid::new_v4();
    store.pin_memory(AGENT, ghost).await.unwrap();
    // Another agent's dangling pin is that agent's maintenance to clean.
    store
        .pin_memory("someone-else", Uuid::new_v4())
        .await
        .unwrap();

    let rehearsal = engine
        .maintenance(
            AGENT,
            MaintenanceConfig {
                dry_run: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        rehearsal.dangling_pins_removed, 1,
        "a dry run projects the count"
    );
    assert_eq!(
        store.get_pinned(AGENT).await.unwrap().len(),
        2,
        "and removes nothing"
    );

    let out = engine
        .maintenance(AGENT, MaintenanceConfig::default())
        .await
        .unwrap();
    assert_eq!(out.dangling_pins_removed, 1);
    assert_eq!(store.get_pinned(AGENT).await.unwrap(), vec![real]);
    assert_eq!(store.get_pinned("someone-else").await.unwrap().len(), 1);

    let nightly = engine
        .nightly(AGENT, NightlyConfig::default())
        .await
        .unwrap();
    assert_eq!(nightly.dangling_pins_removed, 0, "nothing left to remove");
}

#[tokio::test]
async fn nightly_reports_dangling_pins_and_old_output_still_parses() {
    let (engine, store) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    store.pin_memory(AGENT, Uuid::new_v4()).await.unwrap();
    let nightly = engine
        .nightly(AGENT, NightlyConfig::default())
        .await
        .unwrap();
    assert_eq!(nightly.dangling_pins_removed, 1);

    // Output written before the field existed must still deserialize.
    let old: MaintenanceOutput = serde_json::from_str(
        r#"{"memories_decayed":1,"memories_pruned":0,"tombstones_cleaned":0,"duration_ms":3}"#,
    )
    .unwrap();
    assert_eq!(old.dangling_pins_removed, 0);
    let mut json = serde_json::to_value(&nightly).unwrap();
    json.as_object_mut()
        .unwrap()
        .remove("dangling_pins_removed");
    let old: NightlyOutput = serde_json::from_value(json).unwrap();
    assert_eq!(old.dangling_pins_removed, 0);
}

/// A refused pin comes back as `Ok(PinOutput { pinned: false, .. })`, not an `Err`, so
/// the only thing standing between a caller and a silent no-op is the `error` text.
#[tokio::test]
async fn a_refused_pin_always_says_why() {
    let (engine, store) = engine_with(ScriptedEmbedder::new(), ScoringConfig::default()).await;
    let refused = |out: PinOutput, why: &str| {
        assert!(!out.pinned, "{why}: must be refused");
        assert!(
            out.error.as_deref().is_some_and(|e| !e.trim().is_empty()),
            "{why}: refusal without a reason"
        );
    };

    // A fragment.
    let parent = remember(&engine, input(&long_content("LONGDOC"))).await;
    let fragment = store.get_chunks(parent).await.unwrap()[0].id;
    refused(engine.pin(AGENT, fragment).await.unwrap(), "fragment");

    // Already pinned.
    let granted = engine.pin(AGENT, parent).await.unwrap();
    assert!(
        granted.pinned && granted.error.is_none(),
        "a granted pin carries no error"
    );
    refused(engine.pin(AGENT, parent).await.unwrap(), "already pinned");

    // At the cap.
    for i in 1..MAX_PINS {
        let id = remember(&engine, input(&format!("pin filler {i}"))).await;
        assert!(engine.pin(AGENT, id).await.unwrap().pinned);
    }
    let one_too_many = remember(&engine, input("one pin too many")).await;
    let out = engine.pin(AGENT, one_too_many).await.unwrap();
    assert_eq!(out.pin_count, MAX_PINS);
    refused(out, "cap reached");

    // A memory that does not exist is an error, not a quiet `pinned: false`.
    assert!(engine.pin(AGENT, Uuid::new_v4()).await.is_err());
}
