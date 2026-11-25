// Protocol ported from Druide's antidote-api-python 1.1.0 (MIT, © 2026 Druide informatique inc.).
// From: recupere_port_websocket.py and windows.py, for Windows only. The checks on the folder and
// the program, the cap on its answer, the time limits and the kill are MailALM's.

//! Finding the port of Antidote's local WebSocket server.
//!
//! On Windows, `HKLM\SOFTWARE\Druide informatique inc.\Connectix`, value `DossierConnectix`, names
//! the folder of `AgentConnectixConsole.exe`. MailALM checks that the folder is a local absolute
//! path and that the program is a file right in it, then runs it with the single argument `--api`:
//! no shell, no window, no input. It answers `{"port": 59004}` on its output. As in the Python,
//! there are two attempts: the first waits 2 s; the second, which the Python lets wait forever,
//! waits 20 s. The program is killed if it is still running after its answer, or after the wait.
//!
//! The Python also knows fixed paths on macOS and Linux; MailALM targets Windows, and says
//! [`Error::Unavailable`] elsewhere.

use crate::Error;

/// The port of AgentConnectix's WebSocket server.
pub(crate) fn find_port() -> Result<u16, Error> {
    #[cfg(windows)]
    {
        windows::find_port()
    }
    #[cfg(not(windows))]
    {
        Err(Error::Unavailable)
    }
}

/// Whether Antidote's connector is installed, without starting anything: the registry names its
/// folder, and `AgentConnectixConsole.exe` is a file right in it (the checks [`find_port`] makes
/// before it runs the program). The compose window shows Antidote's buttons only then.
pub fn installed() -> Result<(), Error> {
    #[cfg(windows)]
    {
        windows::console_path().map(drop)
    }
    #[cfg(not(windows))]
    {
        Err(Error::Unavailable)
    }
}

/// The port in the console's answer `{"port": N}`, if N is in 1..=65535.
#[cfg(any(windows, test))]
fn port_in(answer: &serde_json::Value) -> Option<u16> {
    answer.get("port")?.as_u64().and_then(|port| u16::try_from(port).ok()).filter(|port| *port != 0)
}

#[cfg(windows)]
mod windows {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::path::{Component, Path, PathBuf, Prefix};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use serde_json::Value;

    use crate::{Error, clip};

    const KEY: &str = r"SOFTWARE\Druide informatique inc.\Connectix";
    const VALUE: &str = "DossierConnectix";
    const CONSOLE: &str = "AgentConnectixConsole.exe";
    /// `CREATE_NO_WINDOW`: the console program gets no console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// The Python waits 2 s, then without limit.
    const ATTEMPTS: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(20)];
    /// The answer is a few bytes of JSON.
    const MAX_ANSWER: usize = 4096;
    /// How long the console may take to exit after its answer.
    const EXIT_WAIT: Duration = Duration::from_secs(2);

    pub(super) fn find_port() -> Result<u16, Error> {
        let console = console_path()?;
        for timeout in ATTEMPTS {
            match ask(&console, timeout)? {
                Some(answer) => match super::port_in(&answer) {
                    Some(port) => return Ok(port),
                    None => log::warn!("Antidote: AgentConnectixConsole answered without a valid port"),
                },
                None => log::warn!("Antidote: AgentConnectixConsole did not answer within {timeout:?}"),
            }
        }
        Err(Error::Console("AgentConnectixConsole gave no port".into()))
    }

    pub(super) fn console_path() -> Result<PathBuf, Error> {
        let key = windows_registry::LOCAL_MACHINE
            .open(KEY)
            .map_err(|_| Error::NotInstalled(format!("no registry key HKLM\\{KEY}")))?;
        let folder = key.get_string(VALUE).map_err(|_| Error::NotInstalled(format!("no {VALUE} in HKLM\\{KEY}")))?;
        console_in(Path::new(&folder))
    }

    /// `AgentConnectixConsole.exe` in `folder`, if `folder` is a local absolute path (a drive
    /// letter, not a network share) and the program is a file right in it, links resolved.
    pub(super) fn console_in(folder: &Path) -> Result<PathBuf, Error> {
        let local = matches!(folder.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)));
        if !local || !folder.is_absolute() {
            return Err(Error::NotInstalled(format!(
                "Antidote's folder is not a local absolute path: {}",
                clip(&folder.to_string_lossy())
            )));
        }
        let console = folder.join(CONSOLE);
        let not_found = || Error::NotInstalled(format!("{CONSOLE} is not a file in {}", clip(&folder.to_string_lossy())));
        let real_folder = std::fs::canonicalize(folder).map_err(|_| not_found())?;
        let real_console = std::fs::canonicalize(&console).map_err(|_| not_found())?;
        let is_file = std::fs::metadata(&real_console).is_ok_and(|m| m.is_file());
        if !is_file || real_console.parent() != Some(real_folder.as_path()) {
            return Err(not_found());
        }
        Ok(console)
    }

    /// Runs `console --api` and reads its answer: `Ok(None)` if none came within `timeout`.
    fn ask(console: &Path, timeout: Duration) -> Result<Option<Value>, Error> {
        let mut child = Command::new(console)
            .arg("--api")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| Error::Console(format!("cannot launch {CONSOLE}: {e}")))?;
        let Some(mut stdout) = child.stdout.take() else {
            stop(&mut child, Duration::ZERO);
            return Err(Error::Console(format!("no output from {CONSOLE}")));
        };
        let (answer_tx, answer_rx) = mpsc::channel();
        let reader = thread::Builder::new().name("antidote-console".into()).spawn(move || {
            let _ = answer_tx.send(read_answer(&mut stdout));
        });
        if let Err(e) = reader {
            stop(&mut child, Duration::ZERO);
            return Err(Error::Console(format!("cannot read {CONSOLE}: {e}")));
        }
        let answer = answer_rx.recv_timeout(timeout);
        stop(&mut child, if answer.is_ok() { EXIT_WAIT } else { Duration::ZERO });
        match answer {
            Ok(Ok(value)) => Ok(Some(value)),
            Ok(Err(why)) => Err(Error::Console(why)),
            Err(_) => Ok(None),
        }
    }

    /// The console's JSON answer, as soon as it is whole (the console need not close its output
    /// first), at most [`MAX_ANSWER`] bytes.
    fn read_answer(stdout: &mut impl Read) -> Result<Value, String> {
        let mut answer = Vec::new();
        let mut chunk = [0u8; 512];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => return serde_json::from_slice(&answer).map_err(|_| format!("{CONSOLE} did not answer in JSON")),
                Ok(n) => {
                    answer.extend_from_slice(chunk.get(..n).unwrap_or_default());
                    if answer.len() > MAX_ANSWER {
                        return Err(format!("{CONSOLE} answered too much"));
                    }
                    if let Ok(value) = serde_json::from_slice(&answer) {
                        return Ok(value);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("reading {CONSOLE}: {e}")),
            }
        }
    }

    /// Lets the console exit within `grace`, then kills it: it never outlives the question.
    fn stop(child: &mut Child, grace: Duration) {
        let deadline = Instant::now() + grace;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                _ => break,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A new empty folder under the system's temporary folder.
        fn scratch(name: &str) -> PathBuf {
            let folder = std::env::temp_dir().join(format!("mailalm-antidote-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).unwrap();
            folder
        }

        #[test]
        fn the_console_must_be_a_file_in_the_folder() {
            let folder = scratch("ok");
            // An empty file: it is only looked at, never run.
            std::fs::write(folder.join(CONSOLE), b"").unwrap();
            assert_eq!(console_in(&folder).unwrap(), folder.join(CONSOLE));
            // With the trailing backslash the registry value has (the Python appends the name).
            let with_slash = PathBuf::from(format!("{}\\", folder.display()));
            assert!(console_in(&with_slash).is_ok());
            std::fs::remove_dir_all(&folder).unwrap();

            let empty = scratch("empty");
            assert!(matches!(console_in(&empty), Err(Error::NotInstalled(_))));
            std::fs::create_dir(empty.join(CONSOLE)).unwrap();
            assert!(matches!(console_in(&empty), Err(Error::NotInstalled(_))), "a folder, not a file");
            std::fs::remove_dir_all(&empty).unwrap();

            assert!(matches!(console_in(Path::new(r"Druide\Connectix")), Err(Error::NotInstalled(_))));
            assert!(matches!(console_in(Path::new(r"\Druide\Connectix")), Err(Error::NotInstalled(_))));
            assert!(matches!(console_in(Path::new(r"\\serveur\partage\Connectix")), Err(Error::NotInstalled(_))));
            assert!(matches!(console_in(Path::new("")), Err(Error::NotInstalled(_))));
        }

        #[test]
        fn the_answer_is_read_as_soon_as_it_is_whole() {
            let mut whole: &[u8] = b"{\"port\": 59004}\r\n";
            assert_eq!(read_answer(&mut whole).unwrap(), serde_json::json!({ "port": 59004 }));
            let mut not_json: &[u8] = b"Erreur";
            assert!(read_answer(&mut not_json).is_err());
            let long = format!("[{}]", "1,".repeat(MAX_ANSWER));
            assert!(read_answer(&mut long.as_bytes()).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn port_in_the_answer() {
        assert_eq!(port_in(&json!({ "port": 59004 })), Some(59004));
        assert_eq!(port_in(&json!({ "port": 1, "autre": true })), Some(1));
        assert_eq!(port_in(&json!({ "port": 0 })), None);
        assert_eq!(port_in(&json!({ "port": 65536 })), None);
        assert_eq!(port_in(&json!({ "port": -1 })), None);
        assert_eq!(port_in(&json!({ "port": "59004" })), None);
        assert_eq!(port_in(&json!({ "port": 59004.5 })), None);
        assert_eq!(port_in(&json!({})), None);
        assert_eq!(port_in(&json!([59004])), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn unavailable_elsewhere() {
        assert!(matches!(find_port(), Err(Error::Unavailable)));
        assert!(matches!(installed(), Err(Error::Unavailable)));
    }
}
