//! Subprocess helper — build a tokio `Command` that never flashes a console
//! window on Windows.
//!
//! Brain2 shells out to `wsl`, `cmd`/`claude`, `git`, and `ollama` constantly
//! (supervisor probes, gbrain retrieval, the brain feeder, the agent). Spawned
//! from a GUI app on Windows, each of those pops a black console window for a
//! moment — which makes the app feel broken. `CREATE_NO_WINDOW` suppresses it
//! so the subprocesses run silently in the background, as an integrated desktop
//! app should.

use tokio::process::Command;

/// Windows `CREATE_NO_WINDOW` flag — the child gets no console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Like `Command::new(program)` but with the no-console-window flag set on
/// Windows. Use this for EVERY external process Brain2 spawns.
pub fn command(program: &str) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}
