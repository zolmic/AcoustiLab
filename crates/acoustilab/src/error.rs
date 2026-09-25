use thiserror::Error;

/// Every failure the engine reports. Messages name the offending element,
/// node or key so a malformed netlist fails loudly rather than silently.
#[derive(Debug, Error)]
pub enum Error {
    #[error("netlist JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("netlist: {0}")]
    Netlist(String),

    #[error("element '{id}': {msg}")]
    Element { id: String, msg: String },

    #[error("unknown element type '{ty}' (element '{id}')")]
    UnknownElementType { id: String, ty: String },

    #[error("singular system at {f_hz} Hz: unknown '{unknown}' is undetermined (floating node or loop?)")]
    Singular { f_hz: f64, unknown: String },

    #[error("probe '{id}': {msg}")]
    Probe { id: String, msg: String },

    #[error("parameter '{name}': {msg}")]
    Parameter { name: String, msg: String },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn element(id: impl Into<String>, msg: impl Into<String>) -> Self {
        Error::Element {
            id: id.into(),
            msg: msg.into(),
        }
    }
}
