//! Terminal de Grove : PTY + émulation `alacritty_terminal`, sans dépendance à l'UI.
//!
//! Le PTY et le parseur tournent dans le thread d'E/S d'alacritty (`EventLoop`).
//! L'UI reçoit des [`TerminalEvent`] via un callback et lit l'écran avec
//! [`Terminal::snapshot`].

mod colors;
mod graphics;
mod links;
mod progress;
mod pty;
mod terminal;

pub use colors::Palette;
pub use graphics::{Image, Placeholder};
pub use links::{Link, is_openable};
pub use progress::Progress;
pub use terminal::{
    CellFlags, CursorShape, RenderCell, Rgb, Snapshot, TermMode, Terminal, TerminalEvent,
    TerminalOptions,
};
