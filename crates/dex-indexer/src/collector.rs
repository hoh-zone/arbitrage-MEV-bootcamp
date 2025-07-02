use burberry::{async_trait, Collector, CollectorStream};
use eyre::Result;
use tokio::time::{interval_at, Duration, Instant};

use crate::types::Event;

pub struct QueryEventCollector {
    tick_interval: Duration,
}

impl QueryEventCollector {
    pub fn new() -> Self {
        Self {
            tick_interval: Duration::from_secs(10),
        }
    }
}

#[async_trait]
impl Collector<Event> for QueryEventCollector {
    fn name(&self) -> &str {
        "QueryEventCollector"
    }

    async fn get_event_stream(&self) -> Result<CollectorStream<'_, Event>> {
        let mut interval = interval_at(Instant::now() + self.tick_interval, self.tick_interval);

        let stream = async_stream::stream! {
            loop {
                interval.tick().await;
                yield Event::QueryEventTrigger;
            }
        };

        Ok(Box::pin(stream))
    }
}
