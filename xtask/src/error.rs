use std::fmt;
use std::io;

/// A command failure with the process exit code `cargo xtask` should report.
///
/// Usage errors exit with 2. A silent error carries only an exit code because
/// the failing child process already printed its diagnostics.
#[derive(Debug)]
pub(crate) struct XtaskError {
    pub(crate) message: String,
    pub(crate) code: i32,
}

impl XtaskError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 1,
        }
    }

    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 2,
        }
    }

    pub(crate) fn with_code(message: impl Into<String>, code: i32) -> Self {
        Self {
            message: message.into(),
            code: if code == 0 { 1 } else { code },
        }
    }

    pub(crate) fn silent(code: i32) -> Self {
        Self::with_code(String::new(), code)
    }
}

impl fmt::Display for XtaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for XtaskError {}

impl From<io::Error> for XtaskError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for XtaskError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<zip::result::ZipError> for XtaskError {
    fn from(error: zip::result::ZipError) -> Self {
        Self::new(error.to_string())
    }
}

pub(crate) type Result<T> = std::result::Result<T, XtaskError>;
