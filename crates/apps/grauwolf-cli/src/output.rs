//! Where a command writes: the `Io` handle (standard output and standard error, capturable in
//! tests) and the print macros live in `grauwolf-tools-app`, which the tool commands need as
//! well (ADR-0118); the CLI re-exports them.

pub use grauwolf_tools_app::{Captured, Io, report};
pub(crate) use grauwolf_tools_app::{err, errln, out, outln};
