use serde::{Deserialize, Serialize};

/// Deployment/service endpoint metadata kept outside the logical application
/// contract. It is consumed by service tooling and never serialized into an
/// [`crate::application::ApplicationManifest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceDescriptor {
    pub name: String,
    pub commands: Vec<MessageEndpointDescriptor>,
    pub events: Vec<MessageEndpointDescriptor>,
    pub transports: Vec<TransportDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<ServiceObservabilityDescriptor>,
}

impl ServiceDescriptor {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            commands: Vec::new(),
            events: Vec::new(),
            transports: Vec::new(),
            observability: None,
        }
    }

    pub fn command(mut self, name: impl Into<String>) -> Self {
        self.commands.push(MessageEndpointDescriptor::new(name));
        self
    }

    pub fn event(mut self, name: impl Into<String>) -> Self {
        self.events.push(MessageEndpointDescriptor::new(name));
        self
    }

    pub fn transport(mut self, kind: impl Into<String>) -> Self {
        self.transports.push(TransportDescriptor::new(kind));
        self
    }

    pub fn observability(mut self, observability: ServiceObservabilityDescriptor) -> Self {
        self.observability = Some(observability);
        self
    }

    pub fn metrics(mut self, metrics: MetricsEndpointDescriptor) -> Self {
        let mut observability = self.observability.unwrap_or_default();
        observability.metrics = Some(metrics);
        self.observability = Some(observability);
        self
    }

    pub fn tracing(mut self, tracing: TracingDescriptor) -> Self {
        let mut observability = self.observability.unwrap_or_default();
        observability.tracing = Some(tracing);
        self.observability = Some(observability);
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceObservabilityDescriptor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<MetricsEndpointDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracing: Option<TracingDescriptor>,
}

impl ServiceObservabilityDescriptor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn metrics(mut self, metrics: MetricsEndpointDescriptor) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn tracing(mut self, tracing: TracingDescriptor) -> Self {
        self.tracing = Some(tracing);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsEndpointDescriptor {
    pub path: String,
    pub port_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
}

impl MetricsEndpointDescriptor {
    pub fn new(path: impl Into<String>, port_name: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            port_name: port_name.into(),
            interval: None,
        }
    }

    pub fn prometheus_default() -> Self {
        Self::new("/metrics", "http").interval("30s")
    }

    pub fn interval(mut self, interval: impl Into<String>) -> Self {
        self.interval = Some(interval.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracingDescriptor {
    pub propagation: TracePropagationMode,
    pub export: TraceExportMode,
}

impl TracingDescriptor {
    pub fn otlp() -> Self {
        Self {
            propagation: TracePropagationMode::W3cTraceContext,
            export: TraceExportMode::Otlp,
        }
    }

    pub fn disabled() -> Self {
        Self {
            propagation: TracePropagationMode::Disabled,
            export: TraceExportMode::Disabled,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TracePropagationMode {
    #[default]
    W3cTraceContext,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceExportMode {
    #[default]
    Otlp,
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEndpointDescriptor {
    pub name: String,
}

impl MessageEndpointDescriptor {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportDescriptor {
    pub kind: String,
}

impl TransportDescriptor {
    pub fn new(kind: impl Into<String>) -> Self {
        Self { kind: kind.into() }
    }

    pub fn http() -> Self {
        Self::new("http")
    }
}
