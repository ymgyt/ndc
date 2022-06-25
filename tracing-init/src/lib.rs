/// A Parameter to configure tracing subscriber.
#[derive(Debug)]
pub struct Parameter<'a> {
    /// Default RUST_LOG.
    directive_env_key: &'a str,
    display_source: bool,
    display_target: bool,
    color: bool,
}

impl Default for Parameter<'_> {
    fn default() -> Self {
       Self {
           directive_env_key: "RUST_LOG",
           display_source: true,
           display_target: false,
           color: true,
       }
    }
}

/// Initialize global tracing subscriber with default parameter.
pub fn init() {
    init_with(Parameter::default())
}

/// Initialize global tracing subscriber.
pub fn init_with(param: Parameter) {
    use tracing_subscriber::{filter, fmt::time};


    let Parameter {
        directive_env_key,
        display_source,
        display_target,
        color,
    } = param;

    if std::env::var(directive_env_key).is_err(){
        std::env::set_var(directive_env_key, "info");
    }

    tracing_subscriber::fmt()
        .with_timer(time::UtcTime::rfc_3339())
        .with_ansi(color)
        .with_file(display_source)
        .with_line_number(display_source)
        .with_target(display_target)
        .with_env_filter(filter::EnvFilter::from_env(directive_env_key))
        .init()
}
