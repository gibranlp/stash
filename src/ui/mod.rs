use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Clear, List, ListItem, Paragraph},
    Frame,
};
use crate::app::{App, AppScreen, InputMode};
use crate::browser::PaneType;
use crate::models::PlaybackStatus;

pub fn render(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(0)
        .constraints(
            [
                Constraint::Length(3), // Header
                Constraint::Min(5),    // Main Pane
                Constraint::Length(6), // Live visualizer spectrum pane (height 6)
                Constraint::Length(5), // Footer/Audio Player
                Constraint::Length(1), // Shortcuts bar
            ]
            .as_ref(),
        )
        .split(f.size());

    // 1. Render Header
    let current_dir_str = app.browser.current_dir.to_string_lossy();
    let header_text = format!(" STASH v0.1 | Path: {} ", current_dir_str);
    let header = Paragraph::new(Line::from(vec![
        Span::styled(&header_text, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(header, chunks[0]);

    // 2. Render Main Screen
    match app.screen {
        AppScreen::Browser => {
            let browser_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
                .split(chunks[1]);

            // Left Pane - Directories
            let is_dirs_focused = app.browser.focused_pane == PaneType::Directories
                && app.input_mode == InputMode::Normal;
            let dir_border_style = if is_dirs_focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let dir_border_type = if is_dirs_focused {
                BorderType::Double
            } else {
                BorderType::Plain
            };

            let dir_list_items: Vec<ListItem> = app
                .browser
                .directories
                .iter()
                .enumerate()
                .map(|(idx, path)| {
                    let dir_name = if path.ends_with(".") {
                        ".".to_string()
                    } else if path.ends_with("..") {
                        "..".to_string()
                    } else {
                        path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "/".to_string())
                    };
                    let prefix = if is_dirs_focused && idx == app.browser.dir_index {
                        "> "
                    } else {
                        "  "
                    };
                    let style = if is_dirs_focused && idx == app.browser.dir_index {
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(Color::Cyan)),
                        Span::styled(dir_name, style),
                    ]))
                })
                .collect();

            let dirs_list = List::new(dir_list_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(dir_border_type)
                        .border_style(dir_border_style)
                        .title(" Directories "),
                );
            app.dirs_list_state.select(Some(app.browser.dir_index));
            f.render_stateful_widget(dirs_list, browser_chunks[0], &mut app.dirs_list_state);

            // Right Pane - Files (or Search Results if search is active)
            let is_files_focused = app.browser.focused_pane == PaneType::Files
                && app.input_mode == InputMode::Normal;
            let file_border_style = if is_files_focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let file_border_type = if is_files_focused {
                BorderType::Double
            } else {
                BorderType::Plain
            };

            let files_to_render = if app.search.active {
                &app.search.results
            } else {
                &app.browser.files
            };
            let file_index_to_use = if app.search.active {
                app.search.selected_index
            } else {
                app.browser.file_index
            };

            let file_list_items: Vec<ListItem> = files_to_render
                .iter()
                .enumerate()
                .map(|(idx, file)| {
                    let is_highlighted = idx == file_index_to_use;
                    let prefix = if is_highlighted { "> " } else { "  " };
                    let select_marker = if file.is_selected { "[*] " } else { "[ ] " };
                    
                    let filename_style = if is_highlighted {
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else if file.is_selected {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    let select_style = if file.is_selected {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };

                    let size_str = format_size(file.size);
                    
                    ListItem::new(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(Color::Cyan)),
                        Span::styled(select_marker, select_style),
                        Span::styled(&file.name, filename_style),
                        Span::raw(" ".repeat(browser_chunks[1].width.saturating_sub(file.name.len() as u16 + size_str.len() as u16 + 20) as usize)),
                        Span::styled(size_str, Style::default().fg(Color::DarkGray)),
                    ]))
                })
                .collect();

            let pane_title = if app.search.active {
                format!(" Search Results (query: {}) ", app.search.query)
            } else {
                " Files ".to_string()
            };

            let files_list = List::new(file_list_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(file_border_type)
                        .border_style(file_border_style)
                        .title(pane_title),
                );
            app.files_list_state.select(Some(file_index_to_use));
            f.render_stateful_widget(files_list, browser_chunks[1], &mut app.files_list_state);
        }
        AppScreen::Queue => {
            // Render Playlist Queue view
            let queue_border_style = Style::default().fg(Color::Cyan);
            let queue_list_items: Vec<ListItem> = app
                .queue
                .items
                .iter()
                .enumerate()
                .map(|(idx, path)| {
                    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Unknown".to_string());
                    let is_highlighted = idx == app.queue_selected_index;
                    
                    let active_track = {
                        let state = app.audio.shared_state.lock().unwrap();
                        state.current_track.clone()
                    };
                    
                    let is_playing_track = Some(path) == active_track.as_ref();
                    
                    let prefix = if is_highlighted { "> " } else { "  " };
                    let playing_marker = if is_playing_track { "=> " } else { "   " };
                    
                    let text_style = if is_playing_track {
                        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                    } else if is_highlighted {
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    ListItem::new(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(Color::Cyan)),
                        Span::styled(playing_marker, Style::default().fg(Color::Green)),
                        Span::styled(format!("{}. ", idx + 1), Style::default().fg(Color::DarkGray)),
                        Span::styled(name, text_style),
                    ]))
                })
                .collect();

            let queue_list = List::new(queue_list_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Double)
                        .border_style(queue_border_style)
                        .title(" Playback Queue [Q] "),
                );
            app.queue_list_state.select(Some(app.queue_selected_index));
            f.render_stateful_widget(queue_list, chunks[1], &mut app.queue_list_state);
        }
        AppScreen::Collections => {
            let coll_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
                .split(chunks[1]);

            // Left Pane - Collections
            let collections_list_items: Vec<ListItem> = app
                .collections
                .collections
                .keys()
                .enumerate()
                .map(|(idx, name)| {
                    let count = app.collections.collections.get(name).map(|v| v.len()).unwrap_or(0);
                    let is_highlighted = idx == app.selected_collection_index;
                    let prefix = if is_highlighted { "> " } else { "  " };
                    let style = if is_highlighted {
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(Color::Cyan)),
                        Span::styled(name, style),
                        Span::styled(format!(" ({})", count), Style::default().fg(Color::DarkGray)),
                    ]))
                })
                .collect();

            let colls_list = List::new(collections_list_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Double)
                        .border_style(Style::default().fg(Color::Cyan))
                        .title(" Collections [C] "),
                );
            app.colls_list_state.select(Some(app.selected_collection_index));
            f.render_stateful_widget(colls_list, coll_chunks[0], &mut app.colls_list_state);

            // Right Pane - Selected Collection contents
            let coll_names: Vec<&String> = app.collections.collections.keys().collect();
            let selected_coll_files_items: Vec<ListItem> = if let Some(&name) = coll_names.get(app.selected_collection_index) {
                if let Some(files) = app.collections.collections.get(name) {
                    files
                        .iter()
                        .enumerate()
                        .map(|(idx, path)| {
                            let filename = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Unknown".to_string());
                            let is_highlighted = idx == app.active_collection_file_index;
                            let prefix = if is_highlighted { "> " } else { "  " };
                            let style = if is_highlighted {
                                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::White)
                            };
                            ListItem::new(Line::from(vec![
                                Span::styled(prefix, Style::default().fg(Color::Cyan)),
                                Span::styled(filename, style),
                            ]))
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            let right_pane_title = if let Some(&name) = coll_names.get(app.selected_collection_index) {
                format!(" Files in Collection: {} ", name)
            } else {
                " Collection Files ".to_string()
            };

            let coll_files_list = List::new(selected_coll_files_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Plain)
                        .border_style(Style::default().fg(Color::DarkGray))
                        .title(right_pane_title),
                );
            app.coll_files_list_state.select(Some(app.active_collection_file_index));
            f.render_stateful_widget(coll_files_list, coll_chunks[1], &mut app.coll_files_list_state);
        }
    }

    // 3. Render Footer / Now Playing Audio status
    let audio_state = app.audio.shared_state.lock().unwrap();
    
    let now_playing_title = if let Some(ref meta) = audio_state.metadata {
        let title = meta.title.as_deref().unwrap_or("Unknown Track");
        let artist = meta.artist.as_deref().unwrap_or("Unknown Artist");
        format!("{} - {}", artist, title)
    } else if let Some(ref path) = audio_state.current_track {
        path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "Unknown".to_string())
    } else {
        "Stopped".to_string()
    };

    let play_status_label = match audio_state.status {
        PlaybackStatus::Playing => "[PLAYING]",
        PlaybackStatus::Paused => "[PAUSED]",
        PlaybackStatus::Stopped => "[STOPPED]",
    };

    let elapsed = format_time(audio_state.elapsed_secs);
    let duration = format_time(audio_state.duration_secs);
    
    // Custom progress bar
    let progress_percentage = if audio_state.duration_secs > 0 {
        (audio_state.elapsed_secs as f32 / audio_state.duration_secs as f32).min(1.0)
    } else {
        0.0
    };
    let bar_width = chunks[2].width.saturating_sub(40) as usize;
    let filled_width = (progress_percentage * bar_width as f32) as usize;
    let progress_bar_str = format!(
        "[{}{}]",
        "=".repeat(filled_width),
        "-".repeat(bar_width.saturating_sub(filled_width))
    );

    let details_line = format!(
        " {}  {}  Volume: {}%  Selected: {}  Queue: {}  (repeat: {}, shuffle: {})",
        play_status_label,
        now_playing_title,
        audio_state.volume,
        app.browser.selected_paths.len(),
        app.queue.items.len(),
        if audio_state.repeat { "on" } else { "off" },
        if audio_state.shuffle { "on" } else { "off" }
    );

    let progress_line = format!(" {} {} / {} ", progress_bar_str, elapsed, duration);

    let player_text = vec![
        Line::from(vec![Span::styled(details_line, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::styled(progress_line, Style::default().fg(Color::Cyan))]),
    ];

    let bars = audio_state.visualizer_data.clone();
    let visualizer_height = 4;
    let mut lines = vec![String::new(); visualizer_height];

    // Compute target number of bars to fill width left-to-right (each takes 2 characters: "█ ")
    let available_width = chunks[2].width.saturating_sub(2);
    let target_bars = (available_width / 2) as usize;

    let mut interpolated_bars = Vec::new();
    if target_bars > 0 && !bars.is_empty() {
        let src_len = bars.len();
        if target_bars == 1 {
            interpolated_bars.push(bars[0]);
        } else {
            for j in 0..target_bars {
                let src_idx = (j as f32 / (target_bars - 1) as f32) * (src_len - 1) as f32;
                let idx_low = src_idx.floor() as usize;
                let idx_high = src_idx.ceil() as usize;
                let weight = src_idx - idx_low as f32;
                let val = if idx_low < src_len && idx_high < src_len {
                    (1.0 - weight) * bars[idx_low] + weight * bars[idx_high]
                } else {
                    bars[idx_low.min(src_len - 1)]
                };
                interpolated_bars.push(val);
            }
        }
    }

    let num_bars = interpolated_bars.len();
    let padding_str = if num_bars > 0 {
        let bar_width_total = num_bars * 2;
        let padding_width = chunks[2].width.saturating_sub(bar_width_total as u16 + 2) / 2;
        " ".repeat(padding_width as usize)
    } else {
        String::new()
    };

    if num_bars > 0 {
        let chars = [" ", " ", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
        for row in (0..visualizer_height).rev() {
            let mut line = String::new();
            for &val in &interpolated_bars {
                let scaled_val = val * visualizer_height as f32;
                let row_floor = row as f32;
                if scaled_val >= row_floor + 1.0 {
                    line.push_str("█ ");
                } else if scaled_val > row_floor {
                    let fraction = scaled_val - row_floor;
                    let char_idx = (fraction * 8.0) as usize;
                    let idx = char_idx.min(8);
                    line.push_str(&format!("{} ", chars[idx]));
                } else {
                    line.push_str("  ");
                }
            }
            lines[visualizer_height - 1 - row] = line;
        }
    }

    let row_colors = [
        Color::Red,     // Top row
        Color::Magenta, // Middle-high row
        Color::Yellow,  // Middle-low row
        Color::Green,   // Bottom row
    ];

    let vis_para = Paragraph::new(
        lines
            .iter()
            .enumerate()
            .map(|(row_idx, l)| {
                let color = row_colors.get(row_idx).copied().unwrap_or(Color::Cyan);
                Line::from(vec![
                    Span::raw(&padding_str),
                    Span::styled(l, Style::default().fg(color).add_modifier(Modifier::BOLD)),
                ])
            })
            .collect::<Vec<_>>(),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Visualizer "),
    );
    f.render_widget(vis_para, chunks[2]);

    // 4. Render Audio System Pane (chunks[3])
    let player_widget = Paragraph::new(player_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Audio Player "),
    );
    f.render_widget(player_widget, chunks[3]);

    // Render Key Bindings Help Bar (chunks[4])
    let help_bar_spans = match app.screen {
        AppScreen::Browser => vec![
            Span::styled(" ? ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Help ", Style::default().fg(Color::White)),
            Span::styled(" Space ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Select ", Style::default().fg(Color::White)),
            Span::styled(" Enter ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Play/Open ", Style::default().fg(Color::White)),
            Span::styled(" r ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Repeat ", Style::default().fg(Color::White)),
            Span::styled(" z ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Shuffle ", Style::default().fg(Color::White)),
            Span::styled(" v ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Copy ", Style::default().fg(Color::White)),
            Span::styled(" y ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Move ", Style::default().fg(Color::White)),
            Span::styled(" d ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Del ", Style::default().fg(Color::White)),
            Span::styled(" c ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" New ", Style::default().fg(Color::White)),
            Span::styled(" a ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Add ", Style::default().fg(Color::White)),
            Span::styled(" C ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Colls ", Style::default().fg(Color::White)),
            Span::styled(" q ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Q-Add ", Style::default().fg(Color::White)),
            Span::styled(" Q ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Queue ", Style::default().fg(Color::White)),
            Span::styled(" / ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Search ", Style::default().fg(Color::White)),
        ],
        AppScreen::Queue => vec![
            Span::styled(" ? ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Help  ", Style::default().fg(Color::White)),
            Span::styled(" Shift+<-/-> ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Seek  ", Style::default().fg(Color::White)),
            Span::styled(" Space ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Play/Pause  ", Style::default().fg(Color::White)),
            Span::styled(" Enter ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Play track  ", Style::default().fg(Color::White)),
            Span::styled(" Esc ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Browser  ", Style::default().fg(Color::White)),
            Span::styled(" n/b ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Next/Prev  ", Style::default().fg(Color::White)),
            Span::styled(" s ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Stop  ", Style::default().fg(Color::White)),
        ],
        AppScreen::Collections => vec![
            Span::styled(" ? ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Help  ", Style::default().fg(Color::White)),
            Span::styled(" Space ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Play/Pause  ", Style::default().fg(Color::White)),
            Span::styled(" Enter ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Play track  ", Style::default().fg(Color::White)),
            Span::styled(" Esc ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Browser  ", Style::default().fg(Color::White)),
            Span::styled(" Left/Right ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Switch Pane  ", Style::default().fg(Color::White)),
        ],
    };

    let help_bar = Paragraph::new(Line::from(help_bar_spans));
    f.render_widget(help_bar, chunks[4]);

    // Drop guard
    drop(audio_state);

    // 5. Overlays / Dialogs
    if app.input_mode == InputMode::CreateCollection {
        render_input_popup(f, " Create Collection ", "Enter collection name:", &app.input_value);
    } else if app.input_mode == InputMode::CopyPath {
        render_input_popup(f, " Copy Selected Files ", "Enter target directory:", &app.input_value);
    } else if app.input_mode == InputMode::MovePath {
        render_input_popup(f, " Move Selected Files ", "Enter target directory:", &app.input_value);
    } else if app.input_mode == InputMode::ConfirmDelete {
        render_confirm_popup(f, " Delete Selections ", "Are you sure you want to delete selected files? (y/n)");
    } else if app.input_mode == InputMode::AddToCollectionList {
        render_add_to_collection_popup(f, app);
    } else if app.show_help {
        render_help_popup(f);
    } else if app.input_mode == InputMode::Search {
        // Draw small overlay search input bar at the bottom
        let search_rect = Rect::new(
            chunks[3].x + 2,
            chunks[3].y + chunks[3].height - 2,
            chunks[3].width - 4,
            1,
        );
        let search_para = Paragraph::new(Line::from(vec![
            Span::styled("Search /: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(&app.search.query, Style::default().fg(Color::White)),
        ]));
        f.render_widget(search_para, search_rect);
    }

    if let Some(ref progress) = *app.file_progress.lock().unwrap() {
        render_progress_popup(f, progress);
    }
}

fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    let sizes = ["B", "KB", "MB", "GB", "TB"];
    let mut count = 0;
    let mut f_bytes = bytes as f64;
    while f_bytes >= 1024.0 && count < sizes.len() - 1 {
        f_bytes /= 1024.0;
        count += 1;
    }
    format!("{:.1} {}", f_bytes, sizes[count])
}

fn format_time(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}

fn render_input_popup(f: &mut Frame, title: &str, label: &str, value: &str) {
    let area = centered_rect(60, 20, f.size());
    f.render_widget(Clear, area);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(title);

    let input_para = Paragraph::new(vec![
        Line::from(label),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled(value, Style::default().fg(Color::White)),
        ]),
    ])
    .block(popup_block);

    f.render_widget(input_para, area);
}

fn render_confirm_popup(f: &mut Frame, title: &str, label: &str) {
    let area = centered_rect(50, 20, f.size());
    f.render_widget(Clear, area);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Red))
        .title(title);

    let confirm_para = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(label, Style::default().add_modifier(Modifier::BOLD))),
    ])
    .block(popup_block);

    f.render_widget(confirm_para, area);
}

fn render_add_to_collection_popup(f: &mut Frame, app: &mut App) {
    let area = centered_rect(50, 40, f.size());
    f.render_widget(Clear, area);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Add Selections to Collection ");

    let coll_names: Vec<&String> = app.collections.collections.keys().collect();
    let list_items: Vec<ListItem> = coll_names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            let is_highlighted = idx == app.selected_add_collection_index;
            let prefix = if is_highlighted { "> " } else { "  " };
            let style = if is_highlighted {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::Cyan)),
                Span::styled(name.as_str(), style),
            ]))
        })
        .collect();

    let list = List::new(list_items)
        .block(popup_block)
        .highlight_style(Style::default().bg(Color::DarkGray));

    app.add_coll_list_state.select(Some(app.selected_add_collection_index));
    f.render_stateful_widget(list, area, &mut app.add_coll_list_state);
}

fn render_help_popup(f: &mut Frame) {
    let area = centered_rect(60, 60, f.size());
    f.render_widget(Clear, area);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Help Guide ");

    let help_text = vec![
        Line::from(Span::styled("Navigation keys:", Style::default().add_modifier(Modifier::UNDERLINED))),
        Line::from("  Up/Down (j/k) - Move selection inside active pane"),
        Line::from("  Left (h)      - Focus Directories, or Go to parent folder if focused"),
        Line::from("  Right (l)     - Focus Files pane"),
        Line::from("  Enter         - Enter folder (left pane) or Play audio (right pane)"),
        Line::from("  Backspace     - Go to parent folder"),
        Line::from(""),
        Line::from(Span::styled("Selection & File Operations:", Style::default().add_modifier(Modifier::UNDERLINED))),
        Line::from("  Space         - Toggle file selection"),
        Line::from("  c             - Create new collection"),
        Line::from("  a             - Add selected files to a collection"),
        Line::from("  C             - Toggle Collections manager screen"),
        Line::from("  q             - Add selected files to queue"),
        Line::from("  Q             - Toggle Playback Queue screen"),
        Line::from("  y             - Move selected files (prompts path)"),
        Line::from("  v             - Copy selected files (prompts path)"),
        Line::from("  d             - Delete selected files (asks confirm)"),
        Line::from(""),
        Line::from(Span::styled("Playback control keys:", Style::default().add_modifier(Modifier::UNDERLINED))),
        Line::from("  Space (player)- Pause / Resume active audio playback"),
        Line::from("  Shift+<- / -> - Seek song backward / forward by 5 seconds"),
        Line::from("  H / L         - Seek song backward / forward by 5 seconds (Vim)"),
        Line::from("  n             - Skip to Next queue track"),
        Line::from("  b             - Skip back to Previous queue track"),
        Line::from("  s             - Stop active audio playback"),
        Line::from("  + / -         - Volume Up / Down"),
        Line::from("  r             - Toggle Repeat mode"),
        Line::from("  z             - Toggle Shuffle mode"),
        Line::from(""),
        Line::from(Span::styled("Search & General:", Style::default().add_modifier(Modifier::UNDERLINED))),
        Line::from("  /             - Incremental walkdir search"),
        Line::from("  ?             - Toggle this Help overlay screen"),
        Line::from("  Esc           - Close dialogue / Exit screens"),
        Line::from("  Ctrl + C      - Quit STASH"),
    ];

    let help_para = Paragraph::new(help_text).block(popup_block);
    f.render_widget(help_para, area);
}

fn render_progress_popup(f: &mut Frame, progress: &crate::app::FileOperationProgress) {
    let area = centered_rect(60, 25, f.size());
    f.render_widget(Clear, area);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" {} Files ", progress.op_type));

    // Construct a progress bar string
    let percent = if progress.total_files > 0 {
        (progress.completed_files as f32 / progress.total_files as f32 * 100.0) as u16
    } else {
        100
    };

    let bar_width = area.width.saturating_sub(4) as usize;
    let filled_width = (bar_width * percent as usize / 100).min(bar_width);
    let empty_width = bar_width.saturating_sub(filled_width);
    let progress_bar_str = format!(
        "{}{}",
        "█".repeat(filled_width),
        "░".repeat(empty_width)
    );

    let details = vec![
        Line::from(""),
        Line::from(format!(" {} {} of {} files...", progress.op_type, progress.completed_files, progress.total_files)),
        Line::from(""),
        Line::from(Span::styled(progress_bar_str, Style::default().fg(Color::Cyan))),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Current: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&progress.current_file, Style::default().fg(Color::White)),
        ]),
    ];

    let progress_para = Paragraph::new(details)
        .block(popup_block);

    f.render_widget(progress_para, area);
}
