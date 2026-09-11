use std::io::{ErrorKind, Read, Write};
use std::thread;
use std::time::Duration;

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream, ToNsName};
use tokio::sync::mpsc::UnboundedSender;

const SOCKET: &str = match cfg!(debug_assertions) {
    true => "sonora-dev.sock",
    false => "sonora.sock",
};

pub enum Instance {
    First,
    Running,
    Failed,
}

/// `args` is every non-flag command-line argument of this launch: a lone `spotify:`/web link, or
/// one or more file paths from an "Open With" selection.
pub fn claim(args: &[String], sender: UnboundedSender<Vec<String>>) -> Instance {
    let name = match SOCKET.to_ns_name::<GenericNamespaced>() {
        Ok(name) => name,
        Err(error) => {
            log::warn!("single: cannot name the instance socket: {error:#}");
            return Instance::Failed;
        }
    };

    let listener = match ListenerOptions::new().name(name.clone()).create_sync() {
        Ok(listener) => listener,

        Err(error) => {
            // Windows reports an occupied pipe with an error other than
            // AddrInUse, so any failure may mean another Sonora owns the socket.
            if hand_over(name.clone(), args) {
                return Instance::Running;
            }

            // Only a filesystem socket can leave a stale AddrInUse behind.
            if error.kind() != ErrorKind::AddrInUse {
                log::error!("single: cannot own the instance socket: {error:#}");
                return Instance::Failed;
            }

            log::warn!("single: socket is occupied but unreachable; attempting recovery");

            match ListenerOptions::new()
                .name(name.clone())
                .try_overwrite(true)
                .max_spin_time(Duration::ZERO)
                .create_sync()
            {
                Ok(listener) => listener,
                Err(error) => {
                    log::error!("single: cannot recover instance socket: {error:#}");
                    return Instance::Failed;
                }
            }
        }
    };

    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else {
                continue;
            };
            let mut payload = String::new();
            if connection.read_to_string(&mut payload).is_err() {
                continue;
            }
            let items: Vec<String> = payload
                .split('\n')
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            if sender.send(items).is_err() {
                break;
            }
        }
    });

    Instance::First
}

fn hand_over(name: interprocess::local_socket::Name<'_>, args: &[String]) -> bool {
    let Ok(mut stream) = Stream::connect(name) else {
        return false;
    };
    stream.write_all(args.join("\n").as_bytes()).is_ok()
}
