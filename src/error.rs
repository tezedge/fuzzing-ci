use failure::Fail;

#[derive(Fail, Debug)]
pub enum Error {
    #[fail(display = "i/o error: {}", _0)]
    IOError(std::io::Error),
    #[fail(display = "deserialization error: {}", _0)]
    TomlDeError(toml::de::Error),
    #[fail(display = "JSON serialization error: {}", _0)]
    JsonError(serde_json::Error),
    #[fail(display = "Template substitution error: {}", _0)]
    HandlebarsRenderError(handlebars::RenderError),
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::IOError(error)
    }
}

impl From<toml::de::Error> for Error {
    fn from(error: toml::de::Error) -> Self {
        Self::TomlDeError(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::JsonError(error)
    }
}

impl From<handlebars::RenderError> for Error {
    fn from(error: handlebars::RenderError) -> Self {
        Self::HandlebarsRenderError(error)
    }
}
