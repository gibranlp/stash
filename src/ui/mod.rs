use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Clear, List, ListItem, Paragraph},
    Frame,
};
use crate::app::{App, AppScreen, InputMode};
use crate::browser::PaneType;
use crate::models::{PlaybackStatus, RepeatMode, VisualizerMode};
use crate::search::{matches_image_extension, matches_text_extension};
use image::GenericImageView;
use std::path::PathBuf;

pub fn render(f: &mut Frame, app: &mut App) {
    let show_player_and_vis = app.screen == AppScreen::Queue;

    let constraints = if show_player_and_vis {
        vec![
            Constraint::Length(3), // Header
            Constraint::Min(5),    // Main Pane
            Constraint::Length(6), // Live visualizer spectrum pane (height 6)
            Constraint::Length(5), // Footer/Audio Player
            Constraint::Length(1), // Shortcuts bar
        ]
    } else {
        vec![
            Constraint::Length(3), // Header
            Constraint::Min(5),    // Main Pane
            Constraint::Length(1), // Shortcuts bar
        ]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(0)
        .constraints(constraints)
        .split(f.size());

    // 1. Render Header
    let current_dir_str = app.browser.current_dir.to_string_lossy();
    let header_text = format!(" STASH v0.3.0 | Path: {} ", current_dir_str);
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
    f.render_widget(Clear, chunks[1]);
    match app.screen {
        AppScreen::Browser => {
            let has_preview = if !app.browser.files.is_empty() && app.browser.file_index < app.browser.files.len() {
                let path = &app.browser.files[app.browser.file_index].path;
                matches_image_extension(path) || matches_text_extension(path)
            } else {
                false
            };

            let browser_chunks = if has_preview {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(25), // Directories pane
                        Constraint::Percentage(25), // Files pane
                        Constraint::Percentage(50)  // Preview pane
                    ].as_ref())
                    .split(chunks[1])
            } else {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(30), // Directories pane
                        Constraint::Percentage(70)  // Files pane
                    ].as_ref())
                    .split(chunks[1])
            };

            let (dir_area, file_area, preview_area) = if has_preview {
                (browser_chunks[0], browser_chunks[1], Some(browser_chunks[2]))
            } else {
                (browser_chunks[0], browser_chunks[1], None)
            };

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
                    let has_subdirs = app.browser.directories_has_subdirs.get(idx).copied().unwrap_or(false);
                    let display_name = if has_subdirs {
                        format!("{} ▸", dir_name)
                    } else {
                        dir_name
                    };
                    let prefix = if is_dirs_focused && idx == app.browser.dir_index {
                        "> "
                    } else {
                        "  "
                    };
                    let is_selected = app.browser.selected_paths.contains(path);
                    let is_special_dir = path.ends_with(".") || path.ends_with("..");
                    let select_marker = if is_special_dir {
                        ""
                    } else if is_selected {
                        "[*] "
                    } else {
                        "[ ] "
                    };
                    let select_style = if is_selected {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let style = if is_dirs_focused && idx == app.browser.dir_index {
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else if is_selected {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(Color::Cyan)),
                        Span::styled(select_marker, select_style),
                        Span::styled(display_name, style),
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
            f.render_stateful_widget(dirs_list, dir_area, &mut app.dirs_list_state);

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
                        Span::raw(" ".repeat(file_area.width.saturating_sub(8 + file.name.len() as u16 + size_str.len() as u16) as usize)),
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
            f.render_stateful_widget(files_list, file_area, &mut app.files_list_state);

            if let Some(area) = preview_area {
                let is_preview_focused = app.browser.focused_pane == PaneType::Preview
                    && app.input_mode == InputMode::Normal;
                let preview_border_style = if is_preview_focused {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let preview_border_type = if is_preview_focused {
                    BorderType::Double
                } else {
                    BorderType::Plain
                };

                let file_path = app.browser.files[app.browser.file_index].path.clone();
                let is_text = matches_text_extension(&file_path);

                let title = if is_text {
                    " Code Preview (Scroll: j/k / PgUp/PgDn / Arrows) ".to_string()
                } else {
                    let proto_str = if let Some(ref picker) = app.picker {
                        format!("{:?}", picker.protocol_type)
                    } else {
                        "None".to_string()
                    };
                    format!(" Image Preview [{}] (Zoom/Pan: +/- / Arrows) ", proto_str)
                };

                let preview_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(preview_border_type)
                    .border_style(preview_border_style)
                    .title(title);
                
                let inner_area = preview_block.inner(area);
                f.render_widget(preview_block, area);

                if is_text {
                    render_text_preview(f, app, inner_area, &file_path);
                } else {
                    render_image_preview(f, app, inner_area, &file_path);
                }
            }
        }
        AppScreen::Queue => {
            // Render Playlist Queue view
            let queue_border_style = Style::default().fg(Color::Cyan);
            
            let filtered_indices = app.get_filtered_queue_indices();

            let queue_list_items: Vec<ListItem> = filtered_indices
                .iter()
                .enumerate()
                .map(|(idx, &(base_idx, orig_idx))| {
                    let path = &app.queue.items[orig_idx];
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
                        Span::styled(format!("{}. ", base_idx + 1), Style::default().fg(Color::DarkGray)),
                        Span::styled(name, text_style),
                    ]))
                })
                .collect();

            let pane_title = if app.search.active && !app.search.query.is_empty() {
                format!(" Playback Queue (filtered: {}) [Q] ", app.search.query)
            } else {
                " Playback Queue [Q] ".to_string()
            };

            let queue_list = List::new(queue_list_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Double)
                        .border_style(queue_border_style)
                        .title(pane_title),
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

    // 3. Render Footer / Now Playing Audio status & Visualizer (only when in Queue screen)
    if show_player_and_vis {
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
            PlaybackStatus::Playing => "[NOW]",
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
        let bar_width = chunks[3].width.saturating_sub(40) as usize;
        let filled_width = (progress_percentage * bar_width as f32) as usize;
        let progress_bar_str = format!(
            "[{}{}]",
            "=".repeat(filled_width),
            "-".repeat(bar_width.saturating_sub(filled_width))
        );

        let status_title_line = format!(
            " {}  {}  (repeat: {}, shuffle: {})",
            play_status_label,
            now_playing_title,
            match audio_state.repeat {
                RepeatMode::Off => "off",
                RepeatMode::All => "all",
                RepeatMode::One => "1",
            },
            if audio_state.shuffle { "on" } else { "off" }
        );

        let progress_line = format!(" {} {} / {} ", progress_bar_str, elapsed, duration);

        let info_line = format!(
            " Volume: {}%    Selected: {}    Queue: {}",
            audio_state.volume,
            app.browser.selected_paths.len(),
            app.queue.items.len()
        );

        let mut player_lines = vec![
            Line::from(vec![Span::styled(status_title_line, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))]),
            Line::from(vec![Span::styled(progress_line, Style::default().fg(Color::Cyan))]),
            Line::from(vec![Span::styled(info_line, Style::default().fg(Color::Yellow))]),
        ];

        if let Some(ref dev_err) = audio_state.device_error {
            let error_msg = format!(" [Audio Device Error: {}] (Hint: try running with 'sudo -E stash <path>')", dev_err);
            player_lines.push(Line::from(vec![Span::styled(error_msg, Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))]));
        }

        let player_text = player_lines;

        let bars = audio_state.visualizer_data.clone();
        let visualizer_height = 4;

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
        let mode_name = match app.config.visualizer_mode {
            VisualizerMode::Spectrum => "Spectrum",
            VisualizerMode::Waveform => "Waveform",
            VisualizerMode::SignalLevels => "VU Meters",
        };
        let vis_title = format!(
            " Visualizer (Mode: {} | Decay: {:.2}) ",
            mode_name,
            app.config.visualizer_decay
        );

        let vis_para = match app.config.visualizer_mode {
            VisualizerMode::Spectrum => {
                let mut lines = vec![String::new(); visualizer_height];
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

                Paragraph::new(
                    lines
                        .iter()
                        .enumerate()
                        .map(|(row_idx, l)| {
                            let color = row_colors.get(row_idx).copied().unwrap_or(Color::Cyan);
                            Line::from(vec![
                                Span::raw(padding_str.clone()),
                                Span::styled(l.clone(), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                            ])
                        })
                        .collect::<Vec<_>>(),
                )
            }
            VisualizerMode::Waveform => {
                let mut lines = vec![String::new(); visualizer_height];
                let padding_str = if num_bars > 0 {
                    let bar_width_total = num_bars * 2;
                    let padding_width = chunks[2].width.saturating_sub(bar_width_total as u16 + 2) / 2;
                    " ".repeat(padding_width as usize)
                } else {
                    String::new()
                };

                if num_bars > 0 {
                    for row in 0..visualizer_height {
                        let mut line = String::new();
                        for &val in &interpolated_bars {
                            let dist = (row as f32 - 1.5).abs();
                            let scaled_val = (val * 2.5).min(1.5);
                            if scaled_val >= dist {
                                line.push_str("█ ");
                            } else {
                                line.push_str("  ");
                            }
                        }
                        lines[row] = line;
                    }
                }

                let row_colors = [
                    Color::Red,     // Top row
                    Color::Magenta, // Middle-high row
                    Color::Magenta, // Middle-low row
                    Color::Red,     // Bottom row
                ];

                Paragraph::new(
                    lines
                        .iter()
                        .enumerate()
                        .map(|(row_idx, l)| {
                            let color = row_colors.get(row_idx).copied().unwrap_or(Color::Cyan);
                            Line::from(vec![
                                Span::raw(padding_str.clone()),
                                Span::styled(l.clone(), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                            ])
                        })
                        .collect::<Vec<_>>(),
                )
            }
            VisualizerMode::SignalLevels => {
                let meter_width = (available_width as usize).saturating_sub(8);
                
                let (left_val, right_val) = if num_bars > 2 {
                    let mid = num_bars / 2;
                    let left_avg: f32 = interpolated_bars[0..mid].iter().sum::<f32>() / mid as f32;
                    let right_avg: f32 = interpolated_bars[mid..].iter().sum::<f32>() / (num_bars - mid) as f32;
                    ((left_avg * 3.5).min(1.0), (right_avg * 3.5).min(1.0))
                } else {
                    (0.0, 0.0)
                };

                let left_filled = (left_val * meter_width as f32) as usize;
                let right_filled = (right_val * meter_width as f32) as usize;

                let build_vu_line = |prefix: &'static str, filled: usize| {
                    let mut spans = vec![
                        Span::styled(prefix, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                        Span::styled(" [", Style::default().fg(Color::DarkGray)),
                    ];

                    let green_cutoff = (meter_width * 70) / 100;
                    let yellow_cutoff = (meter_width * 90) / 100;

                    for idx in 0..meter_width {
                        if idx < filled {
                            let color = if idx < green_cutoff {
                                Color::Green
                            } else if idx < yellow_cutoff {
                                Color::Yellow
                            } else {
                                Color::Red
                            };
                            spans.push(Span::styled("█", Style::default().fg(color)));
                        } else {
                            spans.push(Span::styled(" ", Style::default().fg(Color::DarkGray)));
                        }
                    }
                    spans.push(Span::styled("]", Style::default().fg(Color::DarkGray)));
                    Line::from(spans)
                };

                Paragraph::new(vec![
                    Line::from(""), // spacing
                    build_vu_line("L ", left_filled),
                    build_vu_line("R ", right_filled),
                    Line::from(""), // spacing
                ])
            }
        }
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(vis_title),
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
    }

    // Render Key Bindings Help Bar
    let help_bar_spans = match app.screen {
        AppScreen::Browser => vec![
            Span::styled(" ? ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Help ", Style::default().fg(Color::White)),
        ],
        AppScreen::Queue => vec![
            Span::styled(" ? ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Help ", Style::default().fg(Color::White)),
            Span::styled(" Space ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Play/Pause ", Style::default().fg(Color::White)),
            Span::styled(" Enter ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Play ", Style::default().fg(Color::White)),
            Span::styled(" d/x ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Remove ", Style::default().fg(Color::White)),
            Span::styled(" K/J ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Move U/D ", Style::default().fg(Color::White)),
            Span::styled(" C ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Clear ", Style::default().fg(Color::White)),
            Span::styled(" v ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Visuals ", Style::default().fg(Color::White)),
            Span::styled(" [/] ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Decay ", Style::default().fg(Color::White)),
            Span::styled(" / ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Search ", Style::default().fg(Color::White)),
            Span::styled(" Esc ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(" Browser ", Style::default().fg(Color::White)),
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
    let help_chunk = if show_player_and_vis {
        chunks[4]
    } else {
        chunks[2]
    };
    f.render_widget(help_bar, help_chunk);

    // 5. Overlays / Dialogs
    if app.input_mode == InputMode::CreateCollection {
        render_input_popup(f, " Create Collection ", "Enter collection name:", &app.input_value);
    } else if app.input_mode == InputMode::CopyPath {
        render_input_popup(f, " Copy Selected Files ", "Enter target directory:", &app.input_value);
    } else if app.input_mode == InputMode::MovePath {
        render_input_popup(f, " Move Selected Files ", "Enter target directory:", &app.input_value);
    } else if app.input_mode == InputMode::ConfirmDelete {
        render_confirm_popup(f, " Delete Selections ", "Are you sure you want to delete selected files? (y/n)");
    } else if app.input_mode == InputMode::Rename {
        render_input_popup(f, " Rename Item ", "Enter new name:", &app.input_value);
    } else if app.input_mode == InputMode::AddToCollectionList {
        render_add_to_collection_popup(f, app);
    } else if app.show_help {
        render_help_popup(f);
    } else if app.input_mode == InputMode::Search {
        // Draw small overlay search input bar at the bottom
        let search_y = if show_player_and_vis {
            chunks[3].y + chunks[3].height - 2
        } else {
            chunks[1].y + chunks[1].height - 2
        };
        let search_rect = Rect::new(
            if show_player_and_vis { chunks[3].x + 2 } else { chunks[1].x + 2 },
            search_y,
            if show_player_and_vis { chunks[3].width - 4 } else { chunks[1].width - 4 },
            1,
        );
        let search_para = Paragraph::new(Line::from(vec![
            Span::styled("Search /: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(&app.search.query, Style::default().fg(Color::White)),
        ]));
        f.render_widget(search_para, search_rect);
        
        let cursor_x = (search_rect.x + 9 + app.search.query.chars().count() as u16)
            .min(search_rect.x + search_rect.width.saturating_sub(1));
        f.set_cursor(cursor_x, search_y);
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

fn centered_rect_fixed(percent_x: u16, height_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(r.height.saturating_sub(height_y) / 2),
                Constraint::Length(height_y),
                Constraint::Min(0),
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
    let area = centered_rect_fixed(60, 5, f.size());
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

    let cursor_x = (area.x + 3 + value.chars().count() as u16)
        .min(area.x + area.width.saturating_sub(2));
    let cursor_y = area.y + 3;
    f.set_cursor(cursor_x, cursor_y);
}

fn render_confirm_popup(f: &mut Frame, title: &str, label: &str) {
    let area = centered_rect_fixed(50, 5, f.size());
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
        Line::from("  Tab / Shift+Tab - Cycle focus between active panes"),
        Line::from("  Left (h)      - Go to parent folder"),
        Line::from("  Right (l)     - Enter selected directory (left pane)"),
        Line::from("  Enter         - Enter folder (left pane) or Play audio (right pane)"),
        Line::from("  Backspace     - Go to parent folder"),
        Line::from(""),
        Line::from(Span::styled("Selection & File Operations:", Style::default().add_modifier(Modifier::UNDERLINED))),
        Line::from("  Space         - Toggle file selection"),
        Line::from("  q             - Add selected files to queue"),
        Line::from("  Q             - Toggle Playback Queue screen"),
        Line::from("  y             - Move selected files (prompts path)"),
        Line::from("  v             - Copy selected files (prompts path)"),
        Line::from("  Y             - Copy selected/highlighted to system clipboard (export)"),
        Line::from("  Drag & Drop   - Drop external files/folders into browser (import)"),
        Line::from("  d             - Delete selected files (asks confirm)"),
        Line::from("  F2            - Rename highlighted file or folder"),
        Line::from(""),
        Line::from(Span::styled("Playback control keys:", Style::default().add_modifier(Modifier::UNDERLINED))),
        Line::from("  Space (player)- Pause / Resume active audio playback"),
        Line::from("  Shift+<- / -> - Seek song backward / forward by 5 seconds"),
        Line::from("  H / L         - Seek song backward / forward by 5 seconds (Vim)"),
        Line::from("  n             - Skip to Next queue track"),
        Line::from("  b             - Skip back to Previous queue track"),
        Line::from("  s             - Stop active audio playback"),
        Line::from("  + / -         - Volume Up / Down"),
        Line::from("  r             - Cycle Repeat mode (Off -> All -> 1)"),
        Line::from("  z             - Toggle Shuffle mode"),
        Line::from(""),
        Line::from(Span::styled("Search, Queue Management & Visualizer:", Style::default().add_modifier(Modifier::UNDERLINED))),
        Line::from("  /             - Incremental walkdir search"),
        Line::from("  d / x         - Remove selected item from queue (Queue screen)"),
        Line::from("  Ctrl+Up/Down  - Move selected queue item up/down (Queue screen)"),
        Line::from("  J / K         - Move selected queue item up/down (Vim) (Queue screen)"),
        Line::from("  v             - Cycle visualizer mode (Queue screen)"),
        Line::from("  [ / ]         - Decrease / Increase visualizer decay (Queue screen)"),
        Line::from("  ?             - Toggle this Help overlay screen"),
        Line::from("  Esc           - Close dialogue / Exit screens"),
        Line::from("  Ctrl + C      - Quit STASH"),
    ];

    let help_para = Paragraph::new(help_text).block(popup_block);
    f.render_widget(help_para, area);
}

fn render_progress_popup(f: &mut Frame, progress: &crate::app::FileOperationProgress) {
    let area = centered_rect_fixed(60, 10, f.size());
    f.render_widget(Clear, area);

    let border_color = if progress.error.is_some() {
        Color::Red
    } else if progress.canceled {
        Color::Magenta
    } else {
        Color::Yellow
    };

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(border_color))
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

    let mut details = vec![
        Line::from(""),
        Line::from(format!(" {} {} of {} files...", progress.op_type, progress.completed_files, progress.total_files)),
        Line::from(""),
        Line::from(Span::styled(progress_bar_str, Style::default().fg(Color::Cyan))),
        Line::from(""),
    ];

    if let Some(ref err) = progress.error {
        details.push(Line::from(vec![
            Span::styled(" Error: ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::styled(err, Style::default().fg(Color::LightRed)),
        ]));
        details.push(Line::from(""));
        details.push(Line::from(Span::styled(" Press Esc or Enter to acknowledge ", Style::default().fg(Color::DarkGray))));
    } else if progress.canceled {
        details.push(Line::from(vec![
            Span::styled(" Canceled ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
        ]));
        details.push(Line::from(""));
        details.push(Line::from(Span::styled(" Press Esc or Enter to dismiss ", Style::default().fg(Color::DarkGray))));
    } else {
        details.push(Line::from(vec![
            Span::styled(" Current: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&progress.current_file, Style::default().fg(Color::White)),
        ]));
        details.push(Line::from(""));
        details.push(Line::from(Span::styled(" Press Esc to cancel ", Style::default().fg(Color::DarkGray))));
    }

    let progress_para = Paragraph::new(details)
        .block(popup_block);

    f.render_widget(progress_para, area);
}

fn render_image_preview(f: &mut Frame, app: &mut App, area: Rect, path: &PathBuf) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let img = if let Some((ref cached_path, ref cached_img)) = app.current_image_data {
        if cached_path == path {
            Some(cached_img.clone())
        } else {
            None
        }
    } else {
        None
    };

    let img = match img {
        Some(i) => i,
        None => {
            if let Ok(i) = image::open(path) {
                app.current_image_data = Some((path.clone(), i.clone()));
                i
            } else {
                return;
            }
        }
    };

    let img_w = img.width();
    let img_h = img.height();

    let crop_w = (img_w as f64 / app.image_zoom) as u32;
    let crop_h = (img_h as f64 / app.image_zoom) as u32;

    let crop_w = crop_w.clamp(1, img_w);
    let crop_h = crop_h.clamp(1, img_h);

    let center_x = (img_w / 2) as i32 + app.image_offset_x;
    let center_y = (img_h / 2) as i32 + app.image_offset_y;

    let crop_x = (center_x - (crop_w / 2) as i32).clamp(0, (img_w - crop_w) as i32) as u32;
    let crop_y = (center_y - (crop_h / 2) as i32).clamp(0, (img_h - crop_h) as i32) as u32;

    let cropped = img.crop_imm(crop_x, crop_y, crop_w, crop_h);

    if let Some(ref mut picker) = app.picker {
        let cached_valid = if let Some((ref cached_path, cached_zoom, cached_ox, cached_oy, _)) = app.current_image_protocol {
            cached_path == path
                && (cached_zoom - app.image_zoom).abs() < 1e-5
                && cached_ox == app.image_offset_x
                && cached_oy == app.image_offset_y
        } else {
            false
        };

        if !cached_valid {
            let proto = picker.new_resize_protocol(cropped.clone());
            app.current_image_protocol = Some((path.clone(), app.image_zoom, app.image_offset_x, app.image_offset_y, proto));
        }

        if let Some((_, _, _, _, ref mut proto)) = app.current_image_protocol {
            let image_widget = ratatui_image::ResizeImage::new(None);
            f.render_stateful_widget(image_widget, area, proto);
            return;
        }
    }

    let area_w = area.width as u32;
    let area_h = (area.height as u32).saturating_mul(2);

    let resized = cropped.resize(area_w, area_h, image::imageops::FilterType::CatmullRom);
    let res_w = resized.width() as u16;
    let res_h = resized.height() as u16;

    let dx = area.x + (area.width.saturating_sub(res_w)) / 2;
    let dy = area.y + (area.height.saturating_sub(res_h / 2)) / 2;

    for y in 0..(res_h / 2) {
        let mut spans = Vec::with_capacity(res_w as usize);
        for x in 0..res_w {
            let p_top = resized.get_pixel(x as u32, y as u32 * 2);
            let p_bot = if y as u32 * 2 + 1 < res_h as u32 {
                resized.get_pixel(x as u32, y as u32 * 2 + 1)
            } else {
                p_top
            };

            let color_top = Color::Rgb(p_top[0], p_top[1], p_top[2]);
            let color_bot = Color::Rgb(p_bot[0], p_bot[1], p_bot[2]);

            spans.push(Span::styled("▄", Style::default().fg(color_bot).bg(color_top)));
        }
        let line = Line::from(spans);
        f.render_widget(Paragraph::new(line), Rect::new(dx, dy + y, res_w, 1));
    }
}

fn render_desktop_preview(f: &mut Frame, _app: &mut App, area: Rect, lines: &[String]) {
    let mut name = None;
    let mut generic_name = None;
    let mut comment = None;
    let mut exec = None;
    let mut icon = None;
    let mut terminal = None;
    let mut entry_type = None;
    let mut categories = None;
    let mut mime_type = None;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if let Some(pos) = trimmed.find('=') {
            let key = trimmed[..pos].trim();
            let val = trimmed[pos + 1..].trim().to_string();
            match key {
                "Name" => name = Some(val),
                "GenericName" => generic_name = Some(val),
                "Comment" => comment = Some(val),
                "Exec" => exec = Some(val),
                "Icon" => icon = Some(val),
                "Terminal" => terminal = Some(val),
                "Type" => entry_type = Some(val),
                "Categories" => categories = Some(val),
                "MimeType" => mime_type = Some(val),
                _ => {}
            }
        }
    }

    let mut list_items = Vec::new();
    
    // Header
    list_items.push(Line::from(vec![
        Span::styled("Desktop Entry Configuration", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
    ]));
    list_items.push(Line::from(vec![
        Span::styled("─".repeat(area.width.saturating_sub(2) as usize), Style::default().fg(Color::DarkGray)),
    ]));
    list_items.push(Line::from(vec![]));

    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);
    let highlight_value_style = Style::default().fg(Color::Yellow);

    if let Some(n) = name {
        list_items.push(Line::from(vec![
            Span::styled("  Name:         ", label_style),
            Span::styled(n, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]));
    }
    if let Some(gn) = generic_name {
        list_items.push(Line::from(vec![
            Span::styled("  Generic Name: ", label_style),
            Span::styled(gn, value_style),
        ]));
    }
    if let Some(t) = entry_type {
        list_items.push(Line::from(vec![
            Span::styled("  Type:         ", label_style),
            Span::styled(t, value_style),
        ]));
    }
    if let Some(e) = exec {
        list_items.push(Line::from(vec![
            Span::styled("  Exec Command: ", label_style),
            Span::styled(e, highlight_value_style),
        ]));
    }
    if let Some(term) = terminal {
        list_items.push(Line::from(vec![
            Span::styled("  Terminal:     ", label_style),
            Span::styled(term, value_style),
        ]));
    }
    if let Some(i) = icon {
        list_items.push(Line::from(vec![
            Span::styled("  Icon Name:    ", label_style),
            Span::styled(i, value_style),
        ]));
    }
    if let Some(c) = categories {
        list_items.push(Line::from(vec![
            Span::styled("  Categories:   ", label_style),
            Span::styled(c, value_style),
        ]));
    }
    if let Some(m) = mime_type {
        list_items.push(Line::from(vec![
            Span::styled("  Mime Types:   ", label_style),
            Span::styled(m, value_style),
        ]));
    }
    if let Some(comm) = comment {
        list_items.push(Line::from(vec![]));
        list_items.push(Line::from(vec![
            Span::styled("  Comment:", label_style),
        ]));
        list_items.push(Line::from(vec![
            Span::styled(format!("    {}", comm), Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC)),
        ]));
    }

    list_items.push(Line::from(vec![]));
    list_items.push(Line::from(vec![
        Span::styled("─".repeat(area.width.saturating_sub(2) as usize), Style::default().fg(Color::DarkGray)),
    ]));
    list_items.push(Line::from(vec![
        Span::styled("  Raw File Contents (First 5 lines):", label_style),
    ]));

    let raw_preview_lines = lines.iter().take(5);
    for raw_line in raw_preview_lines {
        list_items.push(Line::from(vec![
            Span::styled(format!("    {}", raw_line), Style::default().fg(Color::DarkGray)),
        ]));
    }

    let paragraph = Paragraph::new(list_items).wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_text_preview(f: &mut Frame, app: &mut App, area: Rect, path: &PathBuf) {
    let lines = if let Some((ref cached_path, ref cached_lines)) = app.current_text_data {
        if cached_path == path {
            Some(cached_lines.clone())
        } else {
            None
        }
    } else {
        None
    };

    let lines = match lines {
        Some(l) => l,
        None => {
            if let Ok(content) = std::fs::read_to_string(path) {
                let parsed_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                app.current_text_data = Some((path.clone(), parsed_lines.clone()));
                parsed_lines
            } else {
                vec!["Error reading file contents or file is not valid UTF-8".to_string()]
            }
        }
    };

    if area.height == 0 || area.width == 0 {
        return;
    }

    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "desktop" {
        render_desktop_preview(f, app, area, &lines);
        return;
    }

    let total_lines = lines.len();
    let viewport_height = area.height as usize;
    let start_idx = if total_lines > 0 {
        app.text_scroll_offset.min(total_lines.saturating_sub(1))
    } else {
        0
    };
    let end_idx = (start_idx + viewport_height * 2).min(total_lines);

    let mut list_items = Vec::new();
    for idx in start_idx..end_idx {
        let line_num = idx + 1;
        let line_content = &lines[idx];

        let num_span = Span::styled(
            format!("{:>4} │ ", line_num),
            Style::default().fg(Color::DarkGray),
        );

        let mut spans = vec![num_span];
        spans.extend(highlight_line(line_content, &ext));

        list_items.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(list_items).wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn highlight_line(line: &str, ext: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let len = chars.len();

    let comment_char = if matches!(ext, "py" | "sh" | "toml" | "yaml" | "yml" | "md") {
        Some('#')
    } else {
        None
    };

    while i < len {
        if comment_char == Some(chars[i]) {
            let comment_text: String = chars[i..].iter().collect();
            spans.push(Span::styled(comment_text, Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)));
            break;
        }

        if i + 1 < len && chars[i] == '/' && chars[i+1] == '/' && !matches!(ext, "py" | "sh" | "toml" | "yaml" | "yml" | "md") {
            let comment_text: String = chars[i..].iter().collect();
            spans.push(Span::styled(comment_text, Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)));
            break;
        }

        if i + 1 < len && chars[i] == '-' && chars[i+1] == '-' && ext == "sql" {
            let comment_text: String = chars[i..].iter().collect();
            spans.push(Span::styled(comment_text, Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)));
            break;
        }

        if chars[i] == '"' || chars[i] == '\'' {
            let quote_char = chars[i];
            let mut string_val = String::new();
            string_val.push(quote_char);
            i += 1;
            let mut escaped = false;
            while i < len {
                let c = chars[i];
                string_val.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == quote_char {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(string_val, Style::default().fg(Color::Green)));
            continue;
        }

        if chars[i].is_alphabetic() || chars[i] == '_' {
            let mut word = String::new();
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                word.push(chars[i]);
                i += 1;
            }

            let style = if is_keyword(&word, ext) {
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
            } else if is_type_or_builtin(&word) {
                Style::default().fg(Color::Yellow)
            } else if word.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                Style::default().fg(Color::Blue)
            } else {
                Style::default().fg(Color::White)
            };
            spans.push(Span::styled(word, style));
            continue;
        }

        if chars[i].is_numeric() {
            let mut num_str = String::new();
            while i < len && (chars[i].is_numeric() || chars[i] == '.') {
                num_str.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(num_str, Style::default().fg(Color::Cyan)));
            continue;
        }

        let c = chars[i];
        let c_str = c.to_string();
        let style = if "{}[Option]().,;+-*/%&|^!~=<>:?".contains(c) {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };
        spans.push(Span::styled(c_str, style));
        i += 1;
    }

    spans
}

fn is_keyword(word: &str, ext: &str) -> bool {
    match ext {
        "rs" => matches!(
            word,
            "as" | "break" | "const" | "continue" | "crate" | "else" | "enum" | "extern" | "false"
                | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop" | "match" | "mod" | "move"
                | "mut" | "pub" | "ref" | "return" | "self" | "Self" | "static" | "struct" | "super"
                | "trait" | "true" | "type" | "unsafe" | "use" | "where" | "while" | "async" | "await"
                | "dyn"
        ),
        "py" => matches!(
            word,
            "False" | "None" | "True" | "and" | "as" | "assert" | "async" | "await" | "break"
                | "class" | "continue" | "def" | "del" | "elif" | "else" | "except" | "finally"
                | "for" | "from" | "global" | "if" | "import" | "in" | "is" | "lambda" | "nonlocal"
                | "not" | "or" | "pass" | "raise" | "return" | "try" | "while" | "with" | "yield"
        ),
        "js" | "ts" => matches!(
            word,
            "break" | "case" | "catch" | "class" | "const" | "continue" | "debugger" | "default"
                | "delete" | "do" | "else" | "export" | "extends" | "finally" | "for" | "function"
                | "if" | "import" | "in" | "instanceof" | "new" | "return" | "super" | "switch"
                | "this" | "throw" | "try" | "typeof" | "var" | "void" | "while" | "with" | "yield"
                | "let" | "package" | "private" | "protected" | "public" | "static" | "interface"
                | "type" | "from" | "as"
        ),
        "c" | "cpp" | "h" | "hpp" => matches!(
            word,
            "auto" | "break" | "case" | "char" | "const" | "continue" | "default" | "do" | "double"
                | "else" | "enum" | "extern" | "float" | "for" | "goto" | "if" | "int" | "long"
                | "register" | "return" | "short" | "signed" | "sizeof" | "static" | "struct"
                | "switch" | "typedef" | "union" | "unsigned" | "void" | "volatile" | "while"
                | "class" | "namespace" | "using" | "template" | "typename" | "public" | "private"
                | "protected" | "virtual" | "override" | "inline"
        ),
        "go" => matches!(
            word,
            "break" | "default" | "func" | "interface" | "select" | "case" | "defer" | "go" | "map"
                | "struct" | "chan" | "else" | "goto" | "package" | "switch" | "const" | "fallthrough"
                | "if" | "range" | "type" | "continue" | "for" | "import" | "return" | "var"
        ),
        "java" | "kt" => matches!(
            word,
            "abstract" | "assert" | "boolean" | "break" | "byte" | "case" | "catch" | "char" | "class"
                | "const" | "continue" | "default" | "do" | "double" | "else" | "enum" | "extends"
                | "final" | "finally" | "float" | "for" | "goto" | "if" | "implements" | "import"
                | "instanceof" | "int" | "interface" | "long" | "native" | "new" | "package" | "private"
                | "protected" | "public" | "return" | "short" | "static" | "strictfp" | "super"
                | "switch" | "synchronized" | "this" | "throw" | "throws" | "transient" | "try"
                | "void" | "volatile" | "while" | "fun" | "val" | "var" | "when"
        ),
        "toml" | "yaml" | "yml" | "json" | "xml" | "html" | "css" | "md" | "sh" | "sql" => matches!(
            word,
            "true" | "false" | "null" | "select" | "insert" | "update" | "delete" | "from" | "where"
                | "and" | "or" | "not" | "join" | "on" | "group" | "by" | "order" | "having" | "limit"
        ),
        _ => false,
    }
}

fn is_type_or_builtin(word: &str) -> bool {
    matches!(
        word,
        "usize" | "u8" | "u16" | "u32" | "u64" | "u128" | "isize" | "i8" | "i16" | "i32" | "i64" | "i128"
            | "f32" | "f64" | "str" | "bool" | "char" | "String" | "Option" | "Result" | "Some" | "None"
            | "Ok" | "Err" | "Self" | "self" | "print" | "println" | "format" | "vec" | "Vec"
            | "int" | "float" | "double" | "boolean" | "void" | "string" | "number" | "any" | "unknown"
    )
}

