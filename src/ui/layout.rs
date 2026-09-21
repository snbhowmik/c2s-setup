use ratatui::{
    layout::{Constraint, Direction, Layout, Rect, Alignment},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Clear},
    Frame,
};
use crate::app::{App, Focus, InputMode};

pub fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(f.size());

    let title_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::Cyan));
        
    let title = Paragraph::new(Span::styled(
        " 🚀 C2S CHIPIN EDA Installer (v1.12.9) ",
        Style::default().add_modifier(Modifier::BOLD),
    ))
    .block(title_block)
    .alignment(Alignment::Center);
    f.render_widget(title, chunks[0]);

    if app.focus == Focus::LogStream {
        draw_log_stream_tab(f, app, chunks[1]);
    } else {
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25),
                Constraint::Percentage(35),
                Constraint::Percentage(40),
            ])
            .split(chunks[1]);

        draw_main_menu(f, app, main_chunks[0]);
        draw_sub_menu(f, app, main_chunks[1]);
        draw_details(f, app, main_chunks[2]);
    }

    let footer_text = match app.focus {
        Focus::LogStream => if !app.is_busy() {
            if app.blink_on { " [Task Complete] Press [Esc] to return " } else { " " }
        } else {
            " Task is running... Please wait "
        },
        _ => "Use [↑/↓/←/→] to navigate, [Enter] execute action, [m] Machine Config, [p] Custom Dep, [q] quit.",
    };

    let footer = Paragraph::new(Span::styled(
        footer_text,
        Style::default().fg(Color::DarkGray),
    ))
    .alignment(Alignment::Center);
    f.render_widget(footer, chunks[2]);

    match app.input_mode {
        InputMode::DependencyPrompt => {
            let area = centered_rect(60, 25, f.size());
            let block = create_block(" Install Custom Dependency ");
            let text = format!("\nEnter package name (e.g., libXp, tcsh) and press Enter:\n\n> {}_", app.input_buffer);
            let p = Paragraph::new(text).block(block).alignment(Alignment::Center);
            f.render_widget(Clear, area);
            f.render_widget(p, area);
        },
        InputMode::MachineConfigPrompt => {
            let area = centered_rect(60, 25, f.size());
            let block = create_block(" Set Machine Number ");
            let text = format!("\nEnter machine number (1-20) and press Enter:\n\n> {}_", app.input_buffer);
            let p = Paragraph::new(text).block(block).alignment(Alignment::Center);
            f.render_widget(Clear, area);
            f.render_widget(p, area);
        },
        InputMode::AddUserPrompt => {
            let area = centered_rect(60, 25, f.size());
            let block = create_block(" Add Lab User ");
            let text = format!("\nEnter user format: USERNAME,ROLE,REG_NO and press Enter:\n\n> {}_", app.input_buffer);
            let p = Paragraph::new(text).block(block).alignment(Alignment::Center);
            f.render_widget(Clear, area);
            f.render_widget(p, area);
        },
        _ => {}
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn create_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(title)
        .style(Style::default().fg(Color::LightBlue))
}

fn draw_main_menu(f: &mut Frame, app: &mut App, area: Rect) {
    let items = vec![
        ListItem::new(" 0. Dashboard "),
        ListItem::new(" 1. Pre-requisites "),
        ListItem::new(" 2. Essential Apps "),
        ListItem::new(" 3. EDA Tools Setup "),
        ListItem::new(" 4. Network Diagnostics "),
        ListItem::new(" 5. User Management "),
        ListItem::new(" 6. View Installer Logs "),
    ];

    let is_focused = app.focus == Focus::MainMenu;
    let block = create_block(" Main Menu ")
        .style(if is_focused { Style::default().fg(Color::LightBlue) } else { Style::default().fg(Color::DarkGray) });

    let list = List::new(items)
        .block(block)
        .highlight_style(if is_focused {
            Style::default().bg(Color::Magenta).fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        })
        .highlight_symbol(">> ");
    f.render_stateful_widget(list, area, &mut app.main_menu_state);
}

fn draw_sub_menu(f: &mut Frame, app: &mut App, area: Rect) {
    let main_idx = app.main_menu_state.selected().unwrap_or(0);

    let mut items = vec![];
    match main_idx {
        0 => items.push(ListItem::new(" [ Refresh Dashboard ] ")),
        1 => items.push(ListItem::new(" [ Execute PreInstall ] ")),
        2 => items.push(ListItem::new(" [ Refresh Checks ] ")),
        3 => {
            items.push(ListItem::new(" Xilinx - Install Binary "));
            items.push(ListItem::new(" Xilinx - Recreate Env "));
            items.push(ListItem::new(" Cadence - Install Binary "));
            items.push(ListItem::new(" Cadence - Recreate Env "));
            items.push(ListItem::new(" Silvaco - Install Binary "));
            items.push(ListItem::new(" Silvaco - Recreate Env "));
            items.push(ListItem::new(" Cadre - Install Binary "));
            items.push(ListItem::new(" Cadre - Recreate Env "));
        },
        4 => items.push(ListItem::new(" [ Refresh Scans ] ")),
        5 => items.push(ListItem::new(" [ Add New Lab User ] ")),
        6 => items.push(ListItem::new(" [ View Logs ] ")),
        _ => {}
    }

    let is_focused = app.focus == Focus::SubMenu;
    let block = create_block(" Actions ")
        .style(if is_focused { Style::default().fg(Color::LightBlue) } else { Style::default().fg(Color::DarkGray) });

    let list = List::new(items)
        .block(block)
        .highlight_style(if is_focused {
            Style::default().bg(Color::Magenta).fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        })
        .highlight_symbol(">> ");
    f.render_stateful_widget(list, area, &mut app.sub_menu_state);
}

fn draw_details(f: &mut Frame, app: &mut App, area: Rect) {
    let main_idx = app.main_menu_state.selected().unwrap_or(0);
    let sub_idx = app.sub_menu_state.selected().unwrap_or(0);
    let block = create_block(" Details ").style(Style::default().fg(Color::Cyan));

    match main_idx {
        0 => {
            let val_state = app.validation_state.lock().unwrap();
            let mut text = vec![Line::from(Span::styled("--- Installed EDA Tools ---", Style::default().add_modifier(Modifier::BOLD)))];
            for (t, b) in &val_state.tool_binary_status {
                text.push(Line::from(format!(" {}: {}", t, if *b { "✔" } else { "✖" })));
            }
            text.push(Line::from(""));
            text.push(Line::from(Span::styled("--- Configured Users (.bashrc) ---", Style::default().add_modifier(Modifier::BOLD))));
            if val_state.configured_users.is_empty() {
                text.push(Line::from(" No users found or checking..."));
            } else {
                for (u, c) in &val_state.configured_users {
                    text.push(Line::from(format!(" {}: {}", u, if *c { "✔" } else { "✖" })));
                }
            }
            let p = Paragraph::new(text).block(block);
            f.render_widget(p, area);
        },
        1 => {
            let val_state = app.validation_state.lock().unwrap();
            let mut text = vec![
                Line::from(Span::styled("System Package Status:", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("Press [Enter] on Execute PreInstall in the middle pane to install missing deps.")
            ];
            for (pkg, b) in &val_state.prereqs_details {
                text.push(Line::from(format!(" {} [{}]", pkg, if *b { "✔" } else { "✖" })));
            }
            let p = Paragraph::new(text).block(block);
            f.render_widget(p, area);
        },
        2 => {
            let val_state = app.validation_state.lock().unwrap();
            let mut text = vec![Line::from(Span::styled("Essential Apps Status:", Style::default().add_modifier(Modifier::BOLD)))];
            for (pkg, b) in &val_state.apps_details {
                text.push(Line::from(format!(" {} [{}]", pkg, if *b { "✔" } else { "✖" })));
            }
            let p = Paragraph::new(text).block(block);
            f.render_widget(p, area);
        },
        3 => {
            let tool_name = match sub_idx / 2 { 0 => "Xilinx", 1 => "Cadence", 2 => "Silvaco", _ => "Cadre" };
            let action = if sub_idx % 2 == 0 { "Install Binary" } else { "Recreate Environment" };
            
            let val_state = app.validation_state.lock().unwrap();
            let mut bin_installed = false;
            let mut env_installed = false;
            if let Some((_, exists)) = val_state.tool_binary_status.iter().find(|(n, _)| n.eq_ignore_ascii_case(tool_name)) { bin_installed = *exists; }
            if let Some((_, exists)) = val_state.tool_env_status.iter().find(|(n, _)| n.eq_ignore_ascii_case(tool_name)) { env_installed = *exists; }

            let text = vec![
                Line::from(Span::styled(format!("Tool: {}", tool_name), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                Line::from(format!("Binary Present: {}", if bin_installed { "✔" } else { "✖" })),
                Line::from(format!("Environment Script: {}", if env_installed { "✔" } else { "✖" })),
                Line::from(""),
                Line::from(Span::styled(format!("Selected Action: {}", action), Style::default().fg(Color::LightGreen))),
                Line::from(if action == "Install Binary" { "Installs the heavy tool binaries." } else { "Generates the wrapper scripts in /opt/vlsilab/env." }),
                Line::from("Press [Enter] while focused on the middle pane to execute.")
            ];
            let p = Paragraph::new(text).block(block);
            f.render_widget(p, area);
        },
        4 => {
            let state = app.network_state.lock().unwrap();
            let p_ip = state.private_ip.as_deref().unwrap_or("Unknown");
            let g_ip = state.gateway_ip.as_deref().unwrap_or("Unknown");
            
            let mut text = vec![
                Line::from(Span::styled("Local Network:", Style::default().add_modifier(Modifier::BOLD))),
                Line::from(format!("  Private IP: {}", p_ip)),
                Line::from(format!("  Default Gateway: {}", g_ip)),
                Line::from(""),
                Line::from(Span::styled("Port Checks:", Style::default().add_modifier(Modifier::BOLD))),
            ];
            for pc in &state.port_checks {
                let status = match pc.is_reachable { Some(true) => "✔ UP", Some(false) => "✖ DOWN", None => "? UNKNOWN" };
                text.push(Line::from(format!("  {}: {}", pc.tool_name, status)));
            }
            let p = Paragraph::new(text).block(block);
            f.render_widget(p, area);
        },
        5 => {
            let mut text = vec![Line::from(Span::styled("Current Lab Users:", Style::default().add_modifier(Modifier::BOLD)))];
            for u in &app.users_list {
                text.push(Line::from(format!(" {} ({}) - .bashrc: {}", u.username, u.role, if u.bashrc_configured { "✔" } else { "✖" })));
            }
            let p = Paragraph::new(text).block(block);
            f.render_widget(p, area);
        },
        _ => {}
    }
}

fn draw_log_stream_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let mut logs = app.logs.clone();
    
    if !app.is_busy() && app.blink_on {
        logs.push(String::new());
        logs.push("==================================================".to_string());
        logs.push(" [TASK COMPLETE] Press [Esc] to return to the menu ".to_string());
        logs.push("==================================================".to_string());
    }

    let text: Vec<Line> = logs.iter()
        .map(|s| {
            if s.contains("[ERROR]") {
                Line::from(Span::styled(s, Style::default().fg(Color::Red)))
            } else if s.contains("[WARN]") {
                Line::from(Span::styled(s, Style::default().fg(Color::Yellow)))
            } else if s.contains("TASK COMPLETE") {
                Line::from(Span::styled(s, Style::default().fg(Color::LightGreen).bg(Color::DarkGray).add_modifier(Modifier::BOLD)))
            } else {
                Line::from(s.as_str())
            }
        })
        .collect();

    let mut scroll_offset = 0;
    if text.len() > area.height.saturating_sub(2) as usize {
        scroll_offset = (text.len() - area.height.saturating_sub(2) as usize) as u16;
    }

    let p = Paragraph::new(text)
        .block(create_block(" Terminal Output "))
        .scroll((scroll_offset, 0));
    f.render_widget(p, area);
}
