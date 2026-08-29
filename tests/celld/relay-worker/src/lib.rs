//! Queue consumer boundary: celld provides durable push/retry, while the
//! native Distributed bus provides command routing and event fanout.

use distributed::bus::{CelldQueueEnvelope, CelldQueueHttpPublisher, CelldQueueRelay};
use distributed::cell_host::CELL_INTERNAL_SECRET_HEADER;
use worker::{event, Context, Env, Error, MessageBatch, MessageExt, Result};

#[event(queue)]
pub async fn main(batch: MessageBatch<CelldQueueEnvelope>, env: Env, _ctx: Context) -> Result<()> {
    console_error_panic_hook::set_once();
    let publisher = CelldQueueHttpPublisher::new(required_var(&env, "CELLD_QUEUE_RELAY_URL")?)
        .with_header(
            CELL_INTERNAL_SECRET_HEADER,
            required_var(&env, "DISTRIBUTED_INTERNAL_SECRET")?,
        );
    let relay = CelldQueueRelay::new(publisher);

    for delivery in batch.messages()? {
        match relay.relay(delivery.body().clone()).await {
            Ok(()) => delivery.ack(),
            Err(error) => {
                worker::console_error!("celld Queue relay failed for {}: {}", delivery.id(), error);
                delivery.retry();
            }
        }
    }
    Ok(())
}

fn required_var(env: &Env, name: &str) -> Result<String> {
    env.var(name)
        .map(|value| value.to_string())
        .map_err(|_| Error::RustError(format!("{name} is required")))
}
