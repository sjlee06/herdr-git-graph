use crossterm::{
    cursor::Show,
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, Write};

type UiTerminal = Terminal<CrosstermBackend<Box<dyn Write>>>;

fn restore() {
    // stdout and stderr can both belong to a closed PTY. Cleanup must not print
    // an error to stderr (and panic) when writing to that terminal fails.
    let _ = execute!(io::stdout(), DisableMouseCapture, Show);
    let _ = ratatui::try_restore();
}

struct RestoreOnDrop;

impl Drop for RestoreOnDrop {
    fn drop(&mut self) {
        restore();
    }
}

pub struct TerminalSession {
    terminal: UiTerminal,
    _restore: RestoreOnDrop,
}

impl TerminalSession {
    pub fn start() -> io::Result<Self> {
        let old_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            old_hook(info);
        }));
        // Install cleanup before enabling raw mode so partial setup also unwinds.
        let guard = RestoreOnDrop;
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        let writer: Box<dyn Write> = Box::new(io::stdout());
        let terminal = Terminal::new(CrosstermBackend::new(writer))?;
        Ok(Self {
            terminal,
            _restore: guard,
        })
    }

    pub fn terminal(&mut self) -> &mut UiTerminal {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Ratatui's Terminal destructor prints to stderr if restoring the cursor
        // fails. We restore the real terminal ourselves and let its destructor
        // write to a sink, including when stdout/stderr have disconnected.
        *self.terminal.backend_mut() = CrosstermBackend::new(Box::new(io::sink()));
    }
}
