use failure::Fail;

#[derive(Fail, Debug)]
pub enum Error {
    #[fail(display = "i/o error: {}", _0)]
    IOError(std::io::Error),
    #[fail(display = "url parse error: {}", _0)]
    UrlParseError(url::ParseError),
    #[fail(display = "toml deserialization error: {}", _0)]
    TomlDeError(toml::de::Error),
    #[fail(display = "toml serialization error: {}", _0)]
    TomlSerError(toml::ser::Error),
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

impl From<url::ParseError> for Error {
    fn from(error: url::ParseError) -> Self {
        Self::UrlParseError(error)
    }
}

impl From<toml::de::Error> for Error {
    fn from(error: toml::de::Error) -> Self {
        Self::TomlDeError(error)
    }
}

impl From<toml::ser::Error> for Error {
    fn from(error: toml::ser::Error) -> Self {
        Self::TomlSerError(error)
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
