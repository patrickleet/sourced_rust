//! High-level celld Queue drain lifecycle for aggregate-cell outboxes.

use std::fmt;
use std::time::Duration;

use worker::{Env, Storage};

use super::cell::AggregateCell;
use super::store::DurableAggregateCellState;
use crate::bus::MessagePublisher;
use crate::bus::{CelldQueuePublisher, TransportError};
use crate::outbox_worker::OutboxDispatchOutcome;
use crate::Aggregate;

/// Conventional celld Queue producer binding for aggregate outboxes.
pub const CELLD_OUTBOX_DEFAULT_BINDING: &str = "OUTBOX";
/// Optional Worker variable overriding the watchdog interval.
pub const CELLD_OUTBOX_DRAIN_INTERVAL_ENV: &str = "OUTBOX_DRAIN_INTERVAL_MS";
/// Default watchdog interval when the Worker variable is absent.
pub const CELLD_OUTBOX_DEFAULT_DRAIN_INTERVAL_MS: u64 = 5_000;
/// Default claim batch sent to celld Queue per drain pass.
pub const CELLD_OUTBOX_DEFAULT_BATCH_SIZE: usize = 100;
/// Default claim lease while a Queue send is in flight.
pub const CELLD_OUTBOX_DEFAULT_LEASE: Duration = Duration::from_secs(30);
/// Default publish-failure ceiling before an outbox row is terminal.
pub const CELLD_OUTBOX_DEFAULT_MAX_ATTEMPTS: u32 = 10_000;

/// celld Queue binding and drain policy attached to an [`AggregateCell`].
///
/// Domain aggregates remain transport-independent. The infrastructure-facing
/// cell host selects this binding once, then [`AggregateCell::persist_and_drain_outbox`]
/// owns the persist → watchdog → publish → settle → persist lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CelldOutbox {
    binding: String,
    drain_interval: Duration,
    lease: Duration,
    batch_size: usize,
    max_attempts: u32,
}

impl CelldOutbox {
    /// Configure an outbox binding with framework defaults.
    pub fn new(binding: impl Into<String>) -> Result<Self, TransportError> {
        let binding = binding.into();
        if binding.trim().is_empty() {
            return Err(TransportError::permanent(
                "celld outbox Queue binding must not be empty",
            ));
        }
        Ok(Self {
            binding,
            drain_interval: Duration::from_millis(CELLD_OUTBOX_DEFAULT_DRAIN_INTERVAL_MS),
            lease: CELLD_OUTBOX_DEFAULT_LEASE,
            batch_size: CELLD_OUTBOX_DEFAULT_BATCH_SIZE,
            max_attempts: CELLD_OUTBOX_DEFAULT_MAX_ATTEMPTS,
        })
    }

    /// Resolve and validate a Worker Queue binding, applying the optional
    /// `OUTBOX_DRAIN_INTERVAL_MS` watchdog override.
    pub fn from_env(env: &Env, binding: impl Into<String>) -> Result<Self, TransportError> {
        let mut outbox = Self::new(binding)?;
        // Fail during Durable Object assembly rather than on the first command.
        CelldQueuePublisher::from_env(env, &outbox.binding)?;

        if let Ok(value) = env.var(CELLD_OUTBOX_DRAIN_INTERVAL_ENV) {
            let raw = value.to_string();
            let milliseconds = raw.parse::<u64>().map_err(|_| {
                TransportError::permanent(format!(
                    "{CELLD_OUTBOX_DRAIN_INTERVAL_ENV} must be an integer number of milliseconds"
                ))
            })?;
            outbox = outbox.with_drain_interval(Duration::from_millis(milliseconds))?;
        }
        Ok(outbox)
    }

    pub fn with_drain_interval(mut self, interval: Duration) -> Result<Self, TransportError> {
        if interval < Duration::from_secs(1) {
            return Err(TransportError::permanent(
                "celld outbox drain interval must be at least 1000ms",
            ));
        }
        self.drain_interval = interval;
        Ok(self)
    }

    pub fn with_lease(mut self, lease: Duration) -> Result<Self, TransportError> {
        if lease.is_zero() {
            return Err(TransportError::permanent(
                "celld outbox claim lease must be greater than zero",
            ));
        }
        self.lease = lease;
        Ok(self)
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Result<Self, TransportError> {
        if batch_size == 0 {
            return Err(TransportError::permanent(
                "celld outbox batch size must be greater than zero",
            ));
        }
        self.batch_size = batch_size;
        Ok(self)
    }

    pub fn with_max_attempts(mut self, max_attempts: u32) -> Result<Self, TransportError> {
        if max_attempts == 0 {
            return Err(TransportError::permanent(
                "celld outbox max attempts must be greater than zero",
            ));
        }
        self.max_attempts = max_attempts;
        Ok(self)
    }

    pub fn binding(&self) -> &str {
        &self.binding
    }

    pub fn drain_interval(&self) -> Duration {
        self.drain_interval
    }

    pub fn lease(&self) -> Duration {
        self.lease
    }

    pub fn batch_size(&self) -> usize {
        self.batch_size
    }

    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    pub(crate) async fn persist_and_drain<A, F, E>(
        &self,
        cell: &AggregateCell<A>,
        env: &Env,
        storage: &Storage,
        persist: F,
    ) -> Result<OutboxDispatchOutcome, TransportError>
    where
        A: Aggregate + Send + Sync + 'static,
        F: Fn(&DurableAggregateCellState) -> Result<(), E>,
        E: fmt::Display,
    {
        let alarm = WorkerAlarm(storage);
        self.persist_and_drain_with(
            cell,
            || CelldQueuePublisher::from_env(env, &self.binding),
            &alarm,
            persist,
        )
        .await
    }

    async fn persist_and_drain_with<A, P, PF, F, E, W>(
        &self,
        cell: &AggregateCell<A>,
        publisher: PF,
        alarm: &W,
        persist: F,
    ) -> Result<OutboxDispatchOutcome, TransportError>
    where
        A: Aggregate + Send + Sync + 'static,
        P: MessagePublisher,
        PF: FnOnce() -> Result<P, TransportError>,
        F: Fn(&DurableAggregateCellState) -> Result<(), E>,
        E: fmt::Display,
        W: CelldOutboxAlarm,
    {
        persist_current_state(cell, &persist)?;

        if !has_pending(cell)? {
            alarm.clear().await?;
            return Ok(OutboxDispatchOutcome::default());
        }

        // The watchdog is durable before Queue egress. If Queue accepts but
        // settlement persistence is interrupted, the stable id is retried.
        alarm.arm(self.drain_interval).await?;

        // Binding resolution is deliberately after the state commit and armed
        // watchdog. A missing/misconfigured Queue cannot make the aggregate
        // mutation disappear; the alarm retries after the deployment is fixed.
        let publisher = publisher()?;
        let dispatcher = cell.outbox_dispatcher(
            publisher,
            format!("celld-queue:{}", cell.instance_name()),
            self.lease,
            self.max_attempts,
        );
        let dispatch = dispatcher.dispatch_batch(self.batch_size).await;

        // Persist even when the dispatcher reports a store error: earlier rows
        // in the pass may already have settled. The armed alarm remains the
        // recovery path if exporting or persisting this state fails.
        persist_current_state(cell, &persist)?;

        if has_pending(cell)? {
            alarm.arm(self.drain_interval).await?;
        } else {
            alarm.clear().await?;
        }

        dispatch
    }
}

#[async_trait::async_trait(?Send)]
trait CelldOutboxAlarm {
    async fn arm(&self, interval: Duration) -> Result<(), TransportError>;
    async fn clear(&self) -> Result<(), TransportError>;
}

struct WorkerAlarm<'a>(&'a Storage);

#[async_trait::async_trait(?Send)]
impl CelldOutboxAlarm for WorkerAlarm<'_> {
    async fn arm(&self, interval: Duration) -> Result<(), TransportError> {
        self.0.set_alarm(interval).await.map_err(|error| {
            TransportError::retryable(format!("cannot arm celld outbox watchdog: {error}"))
        })
    }

    async fn clear(&self) -> Result<(), TransportError> {
        self.0.delete_alarm().await.map_err(|error| {
            TransportError::retryable(format!("cannot clear celld outbox watchdog: {error}"))
        })
    }
}

fn persist_current_state<A, F, E>(
    cell: &AggregateCell<A>,
    persist: &F,
) -> Result<(), TransportError>
where
    A: Aggregate + Send + Sync + 'static,
    F: Fn(&DurableAggregateCellState) -> Result<(), E>,
    E: fmt::Display,
{
    let state = cell.durable_state().map_err(|error| {
        TransportError::permanent(format!("cannot export aggregate cell state: {error}"))
    })?;
    persist(&state).map_err(|error| {
        TransportError::retryable(format!("cannot persist aggregate cell state: {error}"))
    })
}

fn has_pending<A>(cell: &AggregateCell<A>) -> Result<bool, TransportError>
where
    A: Aggregate + Send + Sync + 'static,
{
    cell.durable_outbox()
        .map(|rows| {
            rows.iter()
                .any(|row| !row.is_published() && !row.is_failed())
        })
        .map_err(|error| {
            TransportError::permanent(format!("cannot inspect aggregate cell outbox: {error}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{Entity, EventRecord};
    use crate::outbox_worker::testing::block_on;
    use crate::{OutboxMessage, OutboxMessageStatus};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct TestAggregate {
        entity: Entity,
    }

    impl Aggregate for TestAggregate {
        type ReplayError = String;

        fn aggregate_type() -> &'static str {
            "celld_outbox_test"
        }

        fn entity(&self) -> &Entity {
            &self.entity
        }

        fn entity_mut(&mut self) -> &mut Entity {
            &mut self.entity
        }

        fn replay_event(&mut self, _event: &EventRecord) -> Result<(), Self::ReplayError> {
            Err("test aggregate has no replay events".into())
        }
    }

    #[derive(Clone)]
    struct RecordingPublisher {
        log: Arc<Mutex<Vec<&'static str>>>,
        fail: bool,
    }

    impl MessagePublisher for RecordingPublisher {
        fn publish(
            &self,
            _message: crate::bus::Message,
        ) -> impl std::future::Future<Output = Result<(), TransportError>> + Send + '_ {
            let log = Arc::clone(&self.log);
            let fail = self.fail;
            async move {
                log.lock().unwrap().push("publish");
                if fail {
                    Err(TransportError::retryable("Queue unavailable"))
                } else {
                    Ok(())
                }
            }
        }
    }

    struct RecordingAlarm {
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait(?Send)]
    impl CelldOutboxAlarm for RecordingAlarm {
        async fn arm(&self, _interval: Duration) -> Result<(), TransportError> {
            self.log.lock().unwrap().push("arm");
            Ok(())
        }

        async fn clear(&self) -> Result<(), TransportError> {
            self.log.lock().unwrap().push("clear");
            Ok(())
        }
    }

    fn pending_cell() -> AggregateCell<TestAggregate> {
        let cell = AggregateCell::<TestAggregate>::new("aggregate-1").unwrap();
        let message = OutboxMessage::create("evt-1", "test.created", vec![1]).unwrap();
        cell.restore_durable_state(DurableAggregateCellState {
            version: super::super::store::DURABLE_AGGREGATE_CELL_STATE_VERSION,
            events: Vec::new(),
            snapshots: Vec::new(),
            commands: Vec::new(),
            outbox: vec![message],
            sealed_row: None,
        })
        .unwrap();
        cell
    }

    #[test]
    fn defaults_are_safe_and_conventional() {
        let outbox = CelldOutbox::new(CELLD_OUTBOX_DEFAULT_BINDING).unwrap();
        assert_eq!(outbox.binding(), "OUTBOX");
        assert_eq!(
            outbox.drain_interval(),
            Duration::from_millis(CELLD_OUTBOX_DEFAULT_DRAIN_INTERVAL_MS)
        );
        assert_eq!(outbox.lease(), CELLD_OUTBOX_DEFAULT_LEASE);
        assert_eq!(outbox.batch_size(), CELLD_OUTBOX_DEFAULT_BATCH_SIZE);
        assert_eq!(outbox.max_attempts(), CELLD_OUTBOX_DEFAULT_MAX_ATTEMPTS);
    }

    #[test]
    fn invalid_policy_values_are_rejected() {
        assert!(CelldOutbox::new(" ").is_err());
        let base = CelldOutbox::new("OUTBOX").unwrap();
        assert!(base
            .clone()
            .with_drain_interval(Duration::from_millis(999))
            .is_err());
        assert!(base.clone().with_lease(Duration::ZERO).is_err());
        assert!(base.clone().with_batch_size(0).is_err());
        assert!(base.with_max_attempts(0).is_err());
    }

    #[test]
    fn persists_and_arms_before_publish_then_persists_and_clears() {
        block_on(async {
            let cell = pending_cell();
            let outbox = CelldOutbox::new("OUTBOX").unwrap();
            let log = Arc::new(Mutex::new(Vec::new()));
            let states = Arc::new(Mutex::new(Vec::new()));
            let publisher = RecordingPublisher {
                log: Arc::clone(&log),
                fail: false,
            };
            let alarm = RecordingAlarm {
                log: Arc::clone(&log),
            };
            let persist_log = Arc::clone(&log);
            let persisted_states = Arc::clone(&states);

            let outcome = outbox
                .persist_and_drain_with(
                    &cell,
                    || Ok(publisher),
                    &alarm,
                    move |state| {
                        persist_log.lock().unwrap().push("persist");
                        persisted_states.lock().unwrap().push(state.clone());
                        Ok::<_, String>(())
                    },
                )
                .await
                .unwrap();

            assert_eq!(outcome.published, 1);
            assert_eq!(
                *log.lock().unwrap(),
                ["persist", "arm", "publish", "persist", "clear"]
            );
            let states = states.lock().unwrap();
            assert_eq!(states[0].outbox[0].status, OutboxMessageStatus::Pending);
            assert_eq!(states[1].outbox[0].status, OutboxMessageStatus::Published);
        });
    }

    #[test]
    fn retryable_publish_stays_pending_and_rearms_without_error() {
        block_on(async {
            let cell = pending_cell();
            let outbox = CelldOutbox::new("OUTBOX").unwrap();
            let log = Arc::new(Mutex::new(Vec::new()));
            let states = Arc::new(Mutex::new(Vec::new()));
            let publisher = RecordingPublisher {
                log: Arc::clone(&log),
                fail: true,
            };
            let alarm = RecordingAlarm {
                log: Arc::clone(&log),
            };
            let persist_log = Arc::clone(&log);
            let persisted_states = Arc::clone(&states);

            let outcome = outbox
                .persist_and_drain_with(
                    &cell,
                    || Ok(publisher),
                    &alarm,
                    move |state| {
                        persist_log.lock().unwrap().push("persist");
                        persisted_states.lock().unwrap().push(state.clone());
                        Ok::<_, String>(())
                    },
                )
                .await
                .unwrap();

            assert_eq!(outcome.released, 1);
            assert_eq!(
                *log.lock().unwrap(),
                ["persist", "arm", "publish", "persist", "arm"]
            );
            let states = states.lock().unwrap();
            assert_eq!(states[1].outbox[0].status, OutboxMessageStatus::Pending);
            assert_eq!(states[1].outbox[0].attempts, 1);
        });
    }

    #[test]
    fn persistence_failure_prevents_alarm_and_publish() {
        block_on(async {
            let cell = pending_cell();
            let outbox = CelldOutbox::new("OUTBOX").unwrap();
            let log = Arc::new(Mutex::new(Vec::new()));
            let alarm = RecordingAlarm {
                log: Arc::clone(&log),
            };
            let resolve_log = Arc::clone(&log);
            let persist_log = Arc::clone(&log);

            let error = outbox
                .persist_and_drain_with(
                    &cell,
                    move || {
                        resolve_log.lock().unwrap().push("resolve");
                        Ok(RecordingPublisher {
                            log: Arc::clone(&resolve_log),
                            fail: false,
                        })
                    },
                    &alarm,
                    move |_state| {
                        persist_log.lock().unwrap().push("persist");
                        Err::<(), _>("storage unavailable")
                    },
                )
                .await
                .unwrap_err();

            assert!(error.is_retryable());
            assert_eq!(*log.lock().unwrap(), ["persist"]);
        });
    }

    #[test]
    fn binding_failure_happens_after_persistence_and_watchdog() {
        block_on(async {
            let cell = pending_cell();
            let outbox = CelldOutbox::new("OUTBOX").unwrap();
            let log = Arc::new(Mutex::new(Vec::new()));
            let alarm = RecordingAlarm {
                log: Arc::clone(&log),
            };
            let resolve_log = Arc::clone(&log);
            let persist_log = Arc::clone(&log);

            let error = outbox
                .persist_and_drain_with(
                    &cell,
                    move || {
                        resolve_log.lock().unwrap().push("resolve");
                        Err::<RecordingPublisher, _>(TransportError::permanent(
                            "binding unavailable",
                        ))
                    },
                    &alarm,
                    move |_state| {
                        persist_log.lock().unwrap().push("persist");
                        Ok::<_, String>(())
                    },
                )
                .await
                .unwrap_err();

            assert!(error.is_permanent());
            assert_eq!(*log.lock().unwrap(), ["persist", "arm", "resolve"]);
        });
    }
}
