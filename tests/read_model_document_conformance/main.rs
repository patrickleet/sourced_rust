use serde::{Deserialize, Serialize};
use sourced_rust::{
    HashMapRepository, InMemoryReadModelStore, Lock, LockManager, QueuedReadModelStore, ReadModel,
    ReadModelError, ReadModelSession, ReadModelSessionCommitExt, ReadModelStore, ReadOpts,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(collection = "document_views")]
struct DocumentView {
    #[readmodel(id)]
    id: String,
    value: i32,
    category: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(collection = "other_document_views")]
struct OtherDocumentView {
    #[readmodel(id)]
    id: String,
    value: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "relational_document_views")]
struct RelationalDocumentView {
    #[readmodel(id)]
    id: String,
    value: i32,
}

fn document_view(id: &str, value: i32, category: &str) -> DocumentView {
    DocumentView {
        id: id.into(),
        value,
        category: category.into(),
    }
}

fn document_session(view: &DocumentView) -> ReadModelSession {
    let mut session = ReadModelSession::new();
    session.document(view).unwrap();
    session
}

#[test]
fn document_session_plan_uses_key_value_store_and_shared_clone_storage() {
    let repo = HashMapRepository::new();
    let clone = repo.clone();
    let view = document_view("document-1", 10, "a");

    repo.read_models(document_session(&view))
        .commit_all()
        .unwrap();

    let loaded = clone
        .get_model::<DocumentView>("document-1")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.data, view);
}

#[test]
fn unsupported_row_plan_rejection_does_not_apply_prior_document_write() {
    let repo = HashMapRepository::new();
    let document = document_view("mixed", 10, "a");
    let relational = RelationalDocumentView {
        id: "relational".into(),
        value: 20,
    };
    let mut session = document_session(&document);
    session.save(&relational).unwrap();

    let err = repo.read_models(session).commit_all().unwrap_err();

    assert!(
        matches!(err, sourced_rust::RepositoryError::Model(message) if message.contains("relational row writes"))
    );
    assert!(repo.get_model::<DocumentView>("mixed").unwrap().is_none());
}

#[test]
fn optimistic_conflicts_and_delete_behavior_remain_document_row_storage() {
    let store = InMemoryReadModelStore::new();
    let view = document_view("conflict", 1, "a");
    store.insert(&view).unwrap();

    let err = store
        .update(&document_view("conflict", 2, "a"), 99)
        .unwrap_err();

    assert!(matches!(
        err,
        ReadModelError::ConcurrencyConflict {
            collection,
            id,
            expected: 99,
            actual: 1,
        } if collection == "document_views" && id == "conflict"
    ));
    assert!(store.delete::<DocumentView>("conflict").unwrap());
    assert!(!store.delete::<DocumentView>("conflict").unwrap());
}

#[test]
fn predicate_helpers_are_in_memory_only_and_still_work_in_memory() {
    let store = InMemoryReadModelStore::new();
    store.upsert(&document_view("a-1", 1, "a")).unwrap();
    store.upsert(&document_view("a-2", 2, "a")).unwrap();
    store.upsert(&document_view("b-1", 3, "b")).unwrap();

    let a_views = store
        .find_models::<DocumentView>(&|view| view.category == "a")
        .unwrap();
    let first_high = store
        .find_one_model::<DocumentView>(&|view| view.value > 2)
        .unwrap()
        .unwrap();

    assert_eq!(a_views.len(), 2);
    assert_eq!(first_high.data.id, "b-1");
}

#[test]
fn queued_load_for_update_no_lock_read_and_session_commit_release_lock() {
    let store = QueuedReadModelStore::new(HashMapRepository::new());
    store.upsert(&document_view("locked", 1, "a")).unwrap();
    let loaded = store
        .load_for_update::<DocumentView>("locked")
        .unwrap()
        .unwrap();

    let peeked = store
        .load_no_lock::<DocumentView>("locked")
        .unwrap()
        .unwrap();
    assert_eq!(peeked.data, loaded.data);

    store
        .read_models(document_session(&document_view("locked", 2, "a")))
        .commit_all()
        .unwrap();

    let lock = store
        .lock_manager()
        .get_lock("document_views:locked")
        .unwrap();
    assert!(lock.try_lock().unwrap());
    lock.unlock().unwrap();
}

#[test]
fn queued_abort_unlocks_and_same_id_different_models_are_independent() {
    let store = QueuedReadModelStore::new(HashMapRepository::new());
    store.upsert(&document_view("same", 1, "a")).unwrap();
    store
        .upsert(&OtherDocumentView {
            id: "same".into(),
            value: 2,
        })
        .unwrap();

    store
        .load_for_update::<DocumentView>("same")
        .unwrap()
        .unwrap();
    store
        .load_for_update::<OtherDocumentView>("same")
        .unwrap()
        .unwrap();
    store.abort::<DocumentView>("same").unwrap();
    store.abort::<OtherDocumentView>("same").unwrap();

    let document_lock = store
        .lock_manager()
        .get_lock("document_views:same")
        .unwrap();
    let other_lock = store
        .lock_manager()
        .get_lock("other_document_views:same")
        .unwrap();
    assert!(document_lock.try_lock().unwrap());
    assert!(other_lock.try_lock().unwrap());
    document_lock.unlock().unwrap();
    other_lock.unlock().unwrap();
}

#[test]
fn queued_session_commit_failure_keeps_lock_until_explicit_abort() {
    let store = QueuedReadModelStore::new(HashMapRepository::new());
    store.upsert(&document_view("failed", 1, "a")).unwrap();
    store
        .load_for_update::<DocumentView>("failed")
        .unwrap()
        .unwrap();
    let relational = RelationalDocumentView {
        id: "relational".into(),
        value: 20,
    };
    let mut session = ReadModelSession::new();
    session.save(&relational).unwrap();

    let err = store.read_models(session).commit_all().unwrap_err();

    assert!(
        matches!(err, sourced_rust::RepositoryError::Model(message) if message.contains("relational row writes"))
    );
    let lock = store
        .lock_manager()
        .get_lock("document_views:failed")
        .unwrap();
    assert!(!lock.try_lock().unwrap());
    store.abort::<DocumentView>("failed").unwrap();
    assert!(lock.try_lock().unwrap());
    lock.unlock().unwrap();
}

#[test]
fn read_opts_no_lock_matches_explicit_no_lock_helper() {
    let store = QueuedReadModelStore::new(HashMapRepository::new());
    store.upsert(&document_view("opts", 1, "a")).unwrap();

    let via_opts = store
        .get_model_with::<DocumentView>("opts", ReadOpts::no_lock())
        .unwrap()
        .unwrap();
    let via_helper = store.load_no_lock::<DocumentView>("opts").unwrap().unwrap();

    assert_eq!(via_opts.data, via_helper.data);
}
