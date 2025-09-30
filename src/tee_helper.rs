use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::thread;

pub struct Captured {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: std::process::ExitStatus,
}

/// Run a pre-configured `Command` so that its stdout & stderr are
/// both forwarded live to the parent’s stdout/stderr and also captured.
pub fn run_and_capture(mut cmd: Command) -> io::Result<Captured> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;

    let mut child_stdout = child.stdout.take().unwrap();
    let mut child_stderr = child.stderr.take().unwrap();

    let stdout_buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let out_clone = stdout_buf.clone();
    let out_handle = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut out = io::stdout();
        while let Ok(n) = child_stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            out.write_all(chunk).ok();
            out.flush().ok();
            out_clone.lock().unwrap().extend_from_slice(chunk);
        }
    });

    let err_clone = stderr_buf.clone();
    let err_handle = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut err = io::stderr();
        while let Ok(n) = child_stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            err.write_all(chunk).ok();
            err.flush().ok();
            err_clone.lock().unwrap().extend_from_slice(chunk);
        }
    });

    let status = child.wait()?;
    out_handle.join().unwrap();
    err_handle.join().unwrap();

    let stdout = stdout_buf.lock().unwrap().clone();
    let stderr = stderr_buf.lock().unwrap().clone();

    Ok(Captured {
        stdout,
        stderr,
        status,
    })
}
