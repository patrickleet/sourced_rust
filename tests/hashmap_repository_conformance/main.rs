#[path = "../persistent_repository_conformance/mod.rs"]
mod conformance;

use distributed::HashMapRepository;

async fn repository() -> Option<HashMapRepository> {
    Some(HashMapRepository::new())
}

conformance::repository_conformance_tests!();
