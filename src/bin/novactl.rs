//! `novactl` - control a running NovaWM over its IPC socket.
//!

use std::{
    env,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::ExitCode,
};

const SOCKET_NAME: &str = "novawm-ipc.sock";

fn socket_path() -> Result<PathBuf, String> {
    if let Some(overridden) = env::var_os("NOVAWM_IPC_SOCKET") {
        if !overridden.is_empty() {
            return Ok(PathBuf::from(overridden));
        }
    }

    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "XDG_RUNTIME_DIR is not set".to_string())?;

    Ok(runtime.join(SOCKET_NAME))
}

fn usage() -> String {
    "usage: novactl <list-windows|monitors|layers|layout get|layout set <scroll|dwindle|canvas>|close [index]|screenshot <area|monitor> <copy|save> [path]|screenshot cancel|record [output] [path]|record stop|status|cancel|toggle|stop>"
        .to_string()
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);

    let Some(command) = args.next() else {
        eprintln!("{}", usage());
        return ExitCode::from(2);
    };

    let request = match command.as_str() {
        "list-windows" | "monitors" | "layers" | "stop" => command,
        "layout" => match args.next().as_deref() {
            Some("get") => "layout get".to_string(),
            Some("set") => match args.next() {
                Some(layout) => format!("layout set {layout}"),
                None => {
                    eprintln!("usage: novactl layout set <scroll|dwindle|canvas>");
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!("usage: novactl layout get | layout set <scroll|dwindle|canvas>");
                return ExitCode::from(2);
            }
        },
        "close" => match args.next() {
            Some(index) => format!("close {index}"),
            None => "close".to_string(),
        },
        "screenshot" => match args.next().as_deref() {
            Some("cancel") => "screenshot cancel".to_string(),
            Some(kind @ ("area" | "monitor")) => match args.next().as_deref() {
                Some(dest @ ("copy" | "save")) => match args.next() {
                    Some(path) => format!("screenshot {kind} {dest} {path}"),
                    None => format!("screenshot {kind} {dest}"),
                },
                _ => {
                    eprintln!(
                        "usage: novactl screenshot <area|monitor> <copy|save> [path] | screenshot cancel"
                    );
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!(
                    "usage: novactl screenshot <area|monitor> <copy|save> [path] | screenshot cancel"
                );
                return ExitCode::from(2);
            }
        },
        "record" => match args.next().as_deref() {
            None => "record".to_string(),
            Some("stop") => "record stop".to_string(),
            Some("status") => "record status".to_string(),
            Some("cancel") => "record cancel".to_string(),
            Some("toggle") => "record toggle".to_string(),
            Some(output) => match args.next() {
                Some(path) => format!("record {output} {path}"),
                None => format!("record {output}"),
            },
        },
        "help" | "-h" | "--help" => {
            println!("{}", usage());
            return ExitCode::SUCCESS;
        }
        other => {
            eprintln!("novactl: unknown command: {other}");
            eprintln!("{}", usage());
            return ExitCode::from(2);
        }
    };

    let path = match socket_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("novactl: {error} (is novawm running with a runtime dir?)");
            return ExitCode::FAILURE;
        }
    };

    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!(
                "novactl: cannot connect to {}: {error} (novawm must be running; \
                 set NOVAWM_IPC_SOCKET to override the path)",
                path.display(),
            );
            return ExitCode::FAILURE;
        }
    };

    stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(std::time::Duration::from_secs(2))).ok();

    if let Err(error) = writeln!(stream, "{request}") {
        eprintln!("novactl: write failed: {error}");
        return ExitCode::FAILURE;
    }
    if let Err(error) = stream.flush() {
        eprintln!("novactl: flush failed: {error}");
        return ExitCode::FAILURE;
    }

    let mut response = String::new();
    if let Err(error) = stream.read_to_string(&mut response) {
        if error.kind() != io::ErrorKind::TimedOut {
            eprintln!("novactl: read failed: {error}");
            return ExitCode::FAILURE;
        }
    }

    if response.is_empty() {
        eprintln!("novactl: no response from compositor");
        return ExitCode::FAILURE;
    }

    if let Some(rest) = response.strip_prefix("E ") {
        eprint!("{rest}");
        return ExitCode::FAILURE;
    }

    print!("{response}");
    ExitCode::SUCCESS
}