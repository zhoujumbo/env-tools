use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ProgressEvent {
    pub runtime: String,
    pub op: String,
    pub phase: String,
    pub version: String,
    pub percent: Option<f32>,
    pub bytes: Option<u64>,
    pub total: Option<u64>,
    pub message: String,
}

impl ProgressEvent {
    pub fn new(runtime: &str, op: &str, phase: &str, version: &str, message: impl Into<String>) -> Self {
        Self {
            runtime: runtime.into(),
            op: op.into(),
            phase: phase.into(),
            version: version.into(),
            percent: None,
            bytes: None,
            total: None,
            message: message.into(),
        }
    }

    pub fn node(op: &str, phase: &str, version: &str, message: impl Into<String>) -> Self {
        Self::new("node", op, phase, version, message)
    }

    pub fn python(op: &str, phase: &str, version: &str, message: impl Into<String>) -> Self {
        Self::new("python", op, phase, version, message)
    }

    pub fn java(op: &str, phase: &str, version: &str, message: impl Into<String>) -> Self {
        Self::new("java", op, phase, version, message)
    }

    pub fn golang(op: &str, phase: &str, version: &str, message: impl Into<String>) -> Self {
        Self::new("golang", op, phase, version, message)
    }

    pub fn with_bytes(mut self, bytes: u64, total: Option<u64>) -> Self {
        self.bytes = Some(bytes);
        self.total = total;
        if let Some(t) = total {
            if t > 0 {
                self.percent = Some(((bytes as f64 / t as f64) * 100.0) as f32);
            }
        }
        self
    }

    pub fn with_percent(mut self, percent: f32) -> Self {
        self.percent = Some(percent.clamp(0.0, 100.0));
        self
    }
}
