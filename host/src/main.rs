mod capture;
mod debug;
mod serial;

fn usage() {
    eprintln!("usage: esp32-uc-host <command> <serial-port>");
    eprintln!();
    eprintln!("commands:");
    eprintln!("  debug   Interactive debug CLI (t/k/l/s/q)");
    eprintln!("  run     Real capture mode (keyboard + trackpad)");
}

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str());
    let port = args.get(2).map(|s| s.as_str());

    let result = match (cmd, port) {
        (Some("debug"), Some(port)) => debug::run(port),
        (Some("run"), Some(port)) => capture::run(port),
        _ => {
            usage();
            std::process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
