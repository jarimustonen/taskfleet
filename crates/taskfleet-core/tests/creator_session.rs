//! Attribution survives event recovery and old manifests without becoming ownership.
use serde_json::{json, Value};
use taskfleet_core::{
    append_and_apply_event, read_manifest, replay_unapplied_unlocked, RunLock, RunPaths,
};

const SESSION: &str = "12345678-1234-4234-8234-123456789abc";

fn fixture() -> (tempfile::TempDir, RunPaths) {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = RunPaths::new(tmp.path().join("run"), "01jxsnap000000000000000000").unwrap();
    (tmp, paths)
}

fn data(session: Value) -> Value {
    json!({"kind":"spinoff", "lifecycle":"autonomous", "title":"session", "created_by_session":session})
}

#[test]
fn creation_replay_and_lagging_watermark_keep_first_session() {
    let (_tmp, paths) = fixture();
    let first = json!({"harness":"pi", "session_id":SESSION});
    append_and_apply_event(&paths, "run.created", None, None, data(first.clone())).unwrap();
    // A repeated creation fact cannot rewrite immutable attribution.
    append_and_apply_event(
        &paths,
        "run.created",
        None,
        None,
        data(json!({"harness":"pi", "session_id":"abcdefab-1234-4234-8234-123456789abc"})),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(read_manifest(&paths).unwrap().created_by_session).unwrap(),
        first
    );
    // Deterministically model a crash after projection fsync but before the
    // watermark advances. Recovery re-folds both creation facts idempotently.
    RunLock::with_lock(&paths, |locked| {
        let mut disk = serde_json::to_value(read_manifest(&paths)?).unwrap();
        disk["applied_seq"] = json!(0);
        std::fs::write(paths.manifest(), serde_json::to_vec(&disk).unwrap()).unwrap();
        replay_unapplied_unlocked(locked, &paths)?;
        Ok(())
    })
    .unwrap();
    let manifest = read_manifest(&paths).unwrap();
    assert_eq!(manifest.applied_seq, 2);
    assert_eq!(
        serde_json::to_value(manifest.created_by_session).unwrap(),
        first
    );
    assert!(manifest.created_by_run_id.is_none());
    assert!(manifest.parent_run_id.is_none());
}

#[test]
fn old_events_and_manifests_decode_absent_attribution_as_none() {
    let (_tmp, paths) = fixture();
    let mut old = data(Value::Null);
    old.as_object_mut().unwrap().remove("created_by_session");
    append_and_apply_event(&paths, "run.created", None, None, old).unwrap();
    let manifest = read_manifest(&paths).unwrap();
    assert!(manifest.created_by_session.is_none());
    let mut disk = serde_json::to_value(manifest).unwrap();
    disk.as_object_mut().unwrap().remove("created_by_session");
    let decoded: taskfleet_core::Manifest = serde_json::from_value(disk).unwrap();
    assert!(decoded.created_by_session.is_none());
}

#[test]
fn invalid_session_event_data_is_rejected_before_append() {
    for session in [
        json!("pi:bad"),
        json!({}),
        json!({"harness":"claude", "session_id":SESSION}),
        json!({"harness":"pi", "session_id":"bad"}),
        json!({"harness":"pi", "session_id":SESSION, "authority":true}),
    ] {
        let (_tmp, paths) = fixture();
        let err =
            append_and_apply_event(&paths, "run.created", None, None, data(session)).unwrap_err();
        assert!(err.to_string().contains("created_by_session"), "{err}");
        assert!(!paths.manifest().exists());
        assert!(taskfleet_core::read_all_events(&paths.events())
            .unwrap()
            .is_empty());
    }
}
