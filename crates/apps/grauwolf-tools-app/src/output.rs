//! Where a command writes: standard output and standard error behind one handle, so command
//! logic can run in-process and its output be captured in tests.

use std::fmt::Arguments;
use std::io::Write;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, MutexGuard};

type SharedWriter = Arc<Mutex<dyn Write + Send>>;

/// The two output streams of a command. Cloning shares the streams.
#[derive(Clone)]
pub struct Io {
    out: SharedWriter,
    err: SharedWriter,
}

impl Io {
    /// The process's standard output and standard error.
    pub fn stdio() -> Self {
        Self::new(std::io::stdout(), std::io::stderr())
    }

    pub fn new(out: impl Write + Send + 'static, err: impl Write + Send + 'static) -> Self {
        Self {
            out: Arc::new(Mutex::new(out)),
            err: Arc::new(Mutex::new(err)),
        }
    }

    /// An `Io` that collects everything written, and the handle to read it back.
    pub fn captured() -> (Self, Captured) {
        let captured = Captured::default();
        let io = Self::new(captured.out.clone(), captured.err.clone());
        (io, captured)
    }

    /// Writes to the output stream (like `print!`); write errors are ignored like a closed pipe.
    pub fn write_out(&self, args: Arguments<'_>) {
        write_to(&self.out, args);
    }

    /// Writes to the error stream (like `eprint!`).
    pub fn write_err(&self, args: Arguments<'_>) {
        write_to(&self.err, args);
    }

    /// The error stream as a `Write` that can be handed to a component (e.g. the event log).
    pub fn err_writer(&self) -> impl Write + Send + 'static {
        SharedHandle(Arc::clone(&self.err))
    }
}

fn lock(writer: &SharedWriter) -> MutexGuard<'_, dyn Write + Send + 'static> {
    writer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_to(writer: &SharedWriter, args: Arguments<'_>) {
    let mut writer = lock(writer);
    let _ = writer.write_fmt(args).and_then(|()| writer.flush());
}

struct SharedHandle(SharedWriter);

impl Write for SharedHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        lock(&self.0).flush()
    }
}

/// What a command wrote to a captured [`Io`].
#[derive(Default)]
pub struct Captured {
    out: Buffer,
    err: Buffer,
}

impl Captured {
    pub fn stdout(&self) -> String {
        self.out.text()
    }

    pub fn stderr(&self) -> String {
        self.err.text()
    }
}

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    fn text(&self) -> String {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `print!` on the output stream of an [`Io`].
#[macro_export]
#[doc(hidden)]
macro_rules! out {
    ($io:expr, $($arg:tt)*) => { $io.write_out(format_args!($($arg)*)) };
}

/// `println!` on the output stream of an [`Io`].
#[macro_export]
#[doc(hidden)]
macro_rules! outln {
    ($io:expr) => { $io.write_out(format_args!("\n")) };
    ($io:expr, $($arg:tt)*) => { $io.write_out(format_args!("{}\n", format_args!($($arg)*))) };
}

/// `eprint!` on the error stream of an [`Io`].
#[macro_export]
#[doc(hidden)]
macro_rules! err {
    ($io:expr, $($arg:tt)*) => { $io.write_err(format_args!($($arg)*)) };
}

/// `eprintln!` on the error stream of an [`Io`].
#[macro_export]
#[doc(hidden)]
macro_rules! errln {
    ($io:expr, $($arg:tt)*) => { $io.write_err(format_args!("{}\n", format_args!($($arg)*))) };
}

/// Prints errors to stderr and maps the result to the exit code.
pub fn report(io: &Io, result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            errln!(io, "{message}");
            ExitCode::FAILURE
        }
    }
}
