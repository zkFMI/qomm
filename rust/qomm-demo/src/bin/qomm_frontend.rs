use qomm_demo::web::serve_static_frontend;

fn value(arguments: &[String], name: &str) -> Result<String, String> {
    let position = arguments
        .iter()
        .position(|argument| argument == name)
        .ok_or_else(|| format!("missing {name}"))?;
    arguments
        .get(position + 1)
        .cloned()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn run() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let gateway_host = arguments
        .iter()
        .position(|argument| argument == "--gateway-host")
        .and_then(|position| arguments.get(position + 1))
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    serve_static_frontend(
        &value(&arguments, "--host")?,
        value(&arguments, "--port")?
            .parse()
            .map_err(|_| "--port must be an unsigned 16-bit integer".to_string())?,
        &gateway_host,
        value(&arguments, "--gateway-port")?
            .parse()
            .map_err(|_| "--gateway-port must be an unsigned 16-bit integer".to_string())?,
    )
}

fn main() {
    if let Err(error) = run() {
        eprintln!("qomm-frontend failed: {error}");
        std::process::exit(1);
    }
}
