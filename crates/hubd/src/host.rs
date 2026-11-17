//! The service supervisor (`hub-services`) as the endpoint's channel host
//! (`hub-core`): a device's `channel.open` becomes `Supervisor::open_channel`.
//!
//! - The supervisor's channel connection becomes hub-core's
//!   `ChannelStream`: its two halves as they are, and its `StopSignal` as the
//!   `stop` future. The signal also resolves when the service is gone without
//!   a stop; the relay then sees the connection end, so that case waits
//!   forever instead of sending `channel.close stop` for a stop no one asked.
//! - Dropping the future hub-core holds is the cancel: the supervisor's open
//!   drops its pending count and its ticket with it.
//! - `OpenError::NoService` → `ChannelOpenError::NoService`, `Busy` → `Busy`.

use std::sync::Arc;

use pocket_hub_core::{BoxFuture, ChannelHost, ChannelOffer, ChannelOpenError, ChannelStream};
use pocket_hub_services::{OpenError, Supervisor};

pub struct ServiceHost(pub Arc<Supervisor>);

impl ChannelHost for ServiceHost {
    fn open(&self, offer: ChannelOffer) -> BoxFuture<'static, Result<ChannelStream, ChannelOpenError>> {
        let supervisor = self.0.clone();
        Box::pin(async move {
            let offer = pocket_hub_services::ChannelOffer {
                device: offer.device,
                device_name: offer.device_name,
                model: offer.model,
                target: offer.target,
                app: offer.app,
                hash: offer.hash,
                generation: offer.generation,
                payload_limit: offer.payload_limit,
            };
            match supervisor.open_channel(offer).await {
                Ok(stream) => {
                    let mut stop = stream.stop;
                    let stop: BoxFuture<'static, ()> = Box::pin(async move {
                        stop.requested().await;
                        if !stop.is_requested() {
                            std::future::pending::<()>().await;
                        }
                    });
                    Ok(ChannelStream { reader: Box::new(stream.reader), writer: Box::new(stream.writer), stop: Some(stop) })
                }
                Err(OpenError::NoService) => Err(ChannelOpenError::NoService),
                Err(OpenError::Busy(why)) => Err(ChannelOpenError::Busy(why)),
            }
        })
    }
}
