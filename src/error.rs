pub type BoxedError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
pub enum Error {
    RateLimited,
    Other(BoxedError),
}

impl From<&str> for Error {
    fn from(error: &str) -> Self {
        Self::Other(error.into())
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Other(error.into())
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Other(error.into())
    }
}
impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Other(error.into())
    }
}
impl From<BoxedError> for Error {
    fn from(error: BoxedError) -> Self {
        Self::Other(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited => f.write_str("hit GitHub rate limiting"),
            Self::Other(o) => f.write_fmt(format_args!("{}", o)),
        }
    }
}

impl std::error::Error for Error {}
