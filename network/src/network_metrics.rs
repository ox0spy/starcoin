use prometheus::Error as PrometheusError;
use starcoin_metrics::HistogramVec;

#[derive(Clone)]
pub struct NetworkMetrics {
    pub broadcast_duration: HistogramVec,
}

impl NetworkMetrics {
    pub fn register() -> Result<Self, PrometheusError> {
        let broadcast_duration = register_histogram_vec!(
            "broadcast_duration",
            "network broadcast message duration by protocol",
            &["notification_protocol"]
        )?;
        Ok(Self { broadcast_duration })
    }
}
