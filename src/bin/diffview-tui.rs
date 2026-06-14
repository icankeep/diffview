#[path = "../app.rs"]
mod app;
#[path = "../git.rs"]
mod git;
#[path = "../highlight.rs"]
mod highlight;
#[path = "../model.rs"]
mod model;
#[path = "../tree.rs"]
mod tree;
#[path = "../ui.rs"]
mod ui;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use crate::app::Source;

/// IDE-style side-by-side git diff viewer for the terminal.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Directory to preview (defaults to cwd)
    path: Option<PathBuf>,
    /// Base revision to diff against
    #[arg(short, long, default_value = "HEAD")]
    base: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let start = args.path.unwrap_or_else(|| PathBuf::from("."));
    let (root, source) = match git::repo_root(&start) {
        Ok(root) => (root, Source::Git { base: args.base }),
        Err(_) if start.is_dir() => (start, Source::Directory),
        Err(err) => return Err(err),
    };
    let mut app = app::App::new(root, source)?;
    if app.files.is_empty() {
        match &app.source {
            Source::Git { base } => println!("No changes against {base}."),
            Source::Directory => println!("No files under {}.", app.root.display()),
        }
        return Ok(());
    }
    let mut terminal = ratatui::init();
    ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableMouseCapture
    )?;
    let result = run(&mut terminal, &mut app);
    let mouse_result = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableMouseCapture
    );
    ratatui::restore();
    result?;
    mouse_result?;
    Ok(())
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> Result<()> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key(k),
            Event::Mouse(m) => app.on_mouse(m),
            _ => {}
        }
        if app.quit {
            return Ok(());
        }
    }
}
