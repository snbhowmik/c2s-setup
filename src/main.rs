use std::io;
use std::time::Duration;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

mod app;
mod cli;
mod installer;
mod ui;
mod user_mgr;
mod network;
mod sys_validation;

use app::{App, InputMode};
use cli::Cli;
use installer::config::LabConfig;

/// A panic anywhere in the TUI (e.g. a poisoned mutex from a background task)
/// would otherwise unwind straight past the cleanup at the end of main(),
/// leaving the user's real terminal stuck in raw mode / the alternate screen
/// with no visible cursor - it looks like the terminal itself is broken until
/// they run `reset`. Restore it first, then let the default hook print as usual.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = execute!(io::stdout(), crossterm::cursor::Show);
        default_hook(info);
    }));
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    install_panic_hook();

    let cli_args = Cli::parse();

    // Determine current running user
    let sudo_user = std::env::var("SUDO_USER").unwrap_or_else(|_| "sysadmin".to_string());

    // Load config or default to machine #1
    let config = LabConfig::load_from_state(&sudo_user)
        .unwrap_or_else(|| LabConfig::new(1, &sudo_user));

    // Any flag at all runs headlessly and exits - no TUI, no raw mode/alt
    // screen ever entered. This is what makes the installer scriptable over
    // SSH across the lab's 20 workstations.
    if cli_args.wants_cli_mode() {
        if let Err(e) = cli::run(cli_args, config).await {
            eprintln!("[ERROR] {}", e);
            std::process::exit(1);
        }
        return Ok(());
    }

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create App state
    let mut app = App::new(config);

    let res = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("Error running TUI application: {:?}", err);
    }

    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        app.poll_logs();
        app.on_tick();
        terminal.draw(|f| ui::layout::draw(f, app))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                match app.input_mode {
                    InputMode::Normal => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc | KeyCode::Left => {
                            if app.focus == app::Focus::LogStream {
                                app.focus = app::Focus::MainMenu;
                            } else if app.focus == app::Focus::SubMenu {
                                app.focus = app::Focus::MainMenu;
                                app.sub_menu_state.select(Some(0));
                            }
                        }
                        KeyCode::Right => {
                            if app.focus == app::Focus::MainMenu {
                                app.focus = app::Focus::SubMenu;
                            }
                        }
                        KeyCode::Up => {
                            if app.focus == app::Focus::MainMenu {
                                let i = match app.main_menu_state.selected() {
                                    Some(i) => if i == 0 { 0 } else { i - 1 },
                                    None => 0,
                                };
                                app.main_menu_state.select(Some(i));
                                app.sub_menu_state.select(Some(0));
                            } else if app.focus == app::Focus::SubMenu {
                                let i = match app.sub_menu_state.selected() {
                                    Some(i) => if i == 0 { 0 } else { i - 1 },
                                    None => 0,
                                };
                                app.sub_menu_state.select(Some(i));
                            }
                        }
                        KeyCode::Down => {
                            if app.focus == app::Focus::MainMenu {
                                let max_idx = app.current_main_menu_max_idx();
                                let i = match app.main_menu_state.selected() {
                                    Some(i) => if i >= max_idx { max_idx } else { i + 1 },
                                    None => 0,
                                };
                                app.main_menu_state.select(Some(i));
                                app.sub_menu_state.select(Some(0));
                            } else if app.focus == app::Focus::SubMenu {
                                let max_idx = app.current_sub_menu_max_idx();
                                let i = match app.sub_menu_state.selected() {
                                    Some(i) => if i >= max_idx { max_idx } else { i + 1 },
                                    None => 0,
                                };
                                app.sub_menu_state.select(Some(i));
                            }
                        }
                        KeyCode::Enter => {
                            if app.focus == app::Focus::MainMenu {
                                app.focus = app::Focus::SubMenu;
                            } else if app.focus == app::Focus::SubMenu {
                                app.handle_sub_menu_execute();
                            }
                        }
                        KeyCode::Char('a') if app.main_menu_state.selected() == Some(5) => {
                            app.input_mode = InputMode::AddUserPrompt;
                            app.input_buffer.clear();
                        }
                        KeyCode::Char('m') => {
                            app.input_mode = InputMode::MachineConfigPrompt;
                            app.input_buffer.clear();
                        }
                        KeyCode::Char('p') => {
                            app.input_mode = InputMode::DependencyPrompt;
                            app.input_buffer.clear();
                        }
                        _ => {}
                    },
                    InputMode::MachineConfigPrompt => match key.code {
                        KeyCode::Enter => {
                            if let Ok(num) = app.input_buffer.trim().parse::<u8>() {
                                if num >= 1 && num <= 20 {
                                    let sudo_user = std::env::var("SUDO_USER").unwrap_or_else(|_| "sysadmin".to_string());
                                    app.config = LabConfig::new(num, &sudo_user);
                                    let _ = app.config.save_state_key("MACHINE_NUMBER", &num.to_string());
                                    app.add_log(format!("[CONFIG] Machine updated to #{} ({})", num, app.config.hostname_fqdn));
                                    app.refresh_users();
                                }
                            }
                            app.input_buffer.clear();
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Esc => {
                            app.input_buffer.clear();
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Char(c) => {
                            app.input_buffer.push(c);
                        }
                        KeyCode::Backspace => {
                            app.input_buffer.pop();
                        }
                        _ => {}
                    },
                    InputMode::AddUserPrompt => match key.code {
                        KeyCode::Enter => {
                            app.handle_add_user_submit();
                        }
                        KeyCode::Esc => {
                            app.input_buffer.clear();
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Char(c) => {
                            app.input_buffer.push(c);
                        }
                        KeyCode::Backspace => {
                            app.input_buffer.pop();
                        }
                        _ => {}
                    },
                    InputMode::DependencyPrompt => match key.code {
                        KeyCode::Enter => {
                            app.handle_dependency_submit();
                        }
                        KeyCode::Esc => {
                            app.input_buffer.clear();
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Char(c) => {
                            app.input_buffer.push(c);
                        }
                        KeyCode::Backspace => {
                            app.input_buffer.pop();
                        }
                        _ => {}
                    },
                    InputMode::GrantAccessPrompt => match key.code {
                        KeyCode::Enter => {
                            app.handle_grant_access_submit();
                        }
                        KeyCode::Esc => {
                            app.input_buffer.clear();
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Char(c) => {
                            app.input_buffer.push(c);
                        }
                        KeyCode::Backspace => {
                            app.input_buffer.pop();
                        }
                        _ => {}
                    },
                }
            }
        }
    }
}
