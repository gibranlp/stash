use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Clear, List, ListItem, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;
use crate::app::{App, AppScreen, DestBrowserFocus, InputMode};
use crate::healer::{HealerScreen, HealScanState, HealLookupState, HealStatus, EDIT_FIELD_NAMES};
use crate::browser::PaneType;
use crate::library::{LibraryPanel, LibrarySort, LibraryState, ScanState, TAG_FIELD_NAMES, is_smart_playlist, favorite_genre};
use crate::models::{PlaybackStatus, RepeatMode, VisualizerMode};
use crate::search::{matches_image_extension, matches_text_extension};
use image::GenericImageView;
use std::path::PathBuf;

pub fn render(f: &mut Frame, app: &mut App) {
    let show_player_and_vis = app.screen == AppScreen::Queue;
    let show_header = app.screen == AppScreen::Browser;

    // El layout cambia dependiendo de si estamos en la pantalla del queue o no
    let constraints = if show_player_and_vis {
        vec![
            Constraint::Length(0),
            Constraint::Min(5),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(1),
        ]
    } else if show_header {
        vec![
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ]
    } else {
        vec![
            Constraint::Length(0),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(0)
        .constraints(constraints)
        .split(f.size());

    if show_header {
        let current_dir_str = app.browser.current_dir.to_string_lossy();
        let header_text = format!(" Stash | Path: {} ", current_dir_str);
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
    }

    f.render_widget(Clear, chunks[1]);
    match app.screen {
        AppScreen::Browser => {
            // Jalamos todo el estado de audio en un solo lock para no bloquearnos dos veces
            let (browser_progress, current_playing_path) = {
                let state = app.audio.shared_state.lock().unwrap();
                let active = matches!(state.status, PlaybackStatus::Playing | PlaybackStatus::Paused);
                let progress = if active && state.duration_secs > 0 {
                    let status_icon = if matches!(state.status, PlaybackStatus::Playing) { "▶" } else { "⏸" };
                    let track_name = if let Some(ref meta) = state.metadata {
                        let title = meta.title.as_deref().unwrap_or("Unknown");
                        let artist = meta.artist.as_deref().unwrap_or("Unknown");
                        format!("{} - {}", artist, title)
                    } else if let Some(ref path) = state.current_track {
                        path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "Unknown".to_string())
                    } else {
                        "Unknown".to_string()
                    };
                    Some((status_icon, track_name, state.elapsed_secs, state.duration_secs))
                } else {
                    None
                };
                let playing = if active { state.current_track.clone() } else { None };
                (progress, playing)
            };
            let browser_progress = browser_progress;

            // Si hay algo reproduciéndose, le robamos una línea abajo para la barra de progreso
            let (browser_content_area, browser_progress_area) = if browser_progress.is_some() {
                let v = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(3), Constraint::Length(1)])
                    .split(chunks[1]);
                (v[0], Some(v[1]))
            } else {
                (chunks[1], None)
            };

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
                        Constraint::Percentage(40),
                        Constraint::Percentage(60)
                    ].as_ref())
                    .split(browser_content_area)
            } else {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(100)
                    ].as_ref())
                    .split(browser_content_area)
            };

            let (tree_area, preview_area) = if has_preview {
                (browser_chunks[0], Some(browser_chunks[1]))
            } else {
                (browser_chunks[0], None)
            };

            // Si hay unidades externas conectadas, les damos un panel abajo del árbol
            let drives = &app.external_drives;
            let (tree_area, devices_panel_area) = if !drives.is_empty() {
                let panel_height = (drives.len() as u16 + 2).min(6);
                let v = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(3), Constraint::Length(panel_height)])
                    .split(tree_area);
                (v[0], Some(v[1]))
            } else {
                (tree_area, None)
            };

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

            // Ojo: si el search está activo, pintamos los resultados del search en lugar de los archivos normales
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

            // Sacamos selected_paths antes del closure para evitar conflicto de borrows con files_to_render
            let selected_paths = &app.browser.selected_paths;

            let file_list_items: Vec<ListItem> = files_to_render
                .iter()
                .enumerate()
                .map(|(idx, file)| {
                    let is_highlighted = idx == file_index_to_use;
                    let is_now_playing = current_playing_path.as_ref().map(|p| p == &file.path).unwrap_or(false);
                    let is_in_range = if let Some(start) = app.browser.shift_start {
                        let min = start.min(file_index_to_use);
                        let max = start.max(file_index_to_use);
                        idx >= min && idx <= max
                    } else {
                        false
                    };

                    let prefix = if is_highlighted {
                        "> "
                    } else if is_in_range {
                        "» "
                    } else {
                        "  "
                    };

                    // Los marcadores de selección tienen lógica especial para carpetas:
                    // [*] = la carpeta entera está seleccionada
                    // [-] = no está seleccionada pero tiene archivos adentro seleccionados
                    let (select_marker, select_style) = if is_now_playing {
                        ("[♪] ", Style::default().fg(Color::LightGreen))
                    } else if file.is_dir {
                        if file.is_selected {
                            ("[*] ", Style::default().fg(Color::Cyan))
                        } else {
                            let has_any = selected_paths
                                .iter()
                                .any(|p| p != &file.path && p.starts_with(&file.path));
                            if has_any {
                                ("[-] ", Style::default().fg(Color::Yellow))
                            } else {
                                ("[ ] ", Style::default().fg(Color::DarkGray))
                            }
                        }
                    } else if file.is_selected {
                        ("[*] ", Style::default().fg(Color::Green))
                    } else {
                        ("[ ] ", Style::default().fg(Color::DarkGray))
                    };

                    let indent = "  ".repeat(file.depth);
                    let expand_icon = if file.is_dir {
                        if file.is_expanded { "▼ " } else { "▶ " }
                    } else {
                        "  "
                    };
                    let display_name = format!("{}{}{}", indent, expand_icon, file.name);

                    let filename_style = if is_now_playing {
                        Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD)
                    } else if is_highlighted {
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else if is_in_range {
                        Style::default().fg(Color::Cyan)
                    } else if file.is_dir {
                        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
                    } else if file.is_selected {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    let size_str = if file.is_dir {
                        "DIR".to_string()
                    } else {
                        format_size(file.size)
                    };

                    ListItem::new(Line::from(vec![
                        Span::styled(prefix, if is_highlighted || is_in_range { Style::default().fg(Color::Cyan) } else { Style::default() }),
                        Span::styled(select_marker, select_style),
                        Span::styled(display_name.clone(), filename_style),
                        Span::raw(" ".repeat(tree_area.width.saturating_sub(8 + display_name.width() as u16 + size_str.width() as u16) as usize)),
                        Span::styled(size_str, Style::default().fg(Color::DarkGray)),
                    ]))
                })
                .collect();

            let pane_title = if app.search.active {
                format!(" Search Results (query: {}) ", app.search.query)
            } else {
                format!(" Explorer: {} ", app.browser.current_dir.to_string_lossy())
            };

            let files_list = List::new(file_list_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(file_border_type)
                        .border_style(file_border_style)
                        .title(pane_title),
                );

            // Centramos el scroll para que el elemento seleccionado siempre quede a la mitad de la vista
            let h = tree_area.height.saturating_sub(2) as usize;
            let total = files_to_render.len();
            let target_offset = if total <= h {
                0
            } else {
                let half = h / 2;
                if file_index_to_use < half {
                    0
                } else if file_index_to_use >= total.saturating_sub(half) {
                    total.saturating_sub(h)
                } else {
                    file_index_to_use.saturating_sub(half)
                }
            };
            *app.files_list_state.offset_mut() = target_offset;
            app.files_list_state.select(Some(file_index_to_use));
            f.render_stateful_widget(files_list, tree_area, &mut app.files_list_state);

            if let Some(panel_area) = devices_panel_area {
                let drive_items: Vec<ratatui::widgets::ListItem> = app.external_drives.iter().map(|path| {
                    let name = path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.to_string_lossy().into_owned());
                    ratatui::widgets::ListItem::new(Line::from(vec![
                        Span::styled("  \u{f0a0} ", Style::default().fg(Color::Yellow)),
                        Span::styled(name, Style::default().fg(Color::White)),
                    ]))
                }).collect();
                let drives_list = List::new(drive_items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Plain)
                        .border_style(Style::default().fg(Color::DarkGray))
                        .title(Span::styled(" Drives [E] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                );
                f.render_widget(drives_list, panel_area);
            }

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
                    format!(" Image Preview [{}] ", proto_str)
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

            if let (Some(pb_area), Some((status_icon, track_name, elapsed, duration))) =
                (browser_progress_area, browser_progress)
            {
                let progress = (elapsed as f32 / duration as f32).min(1.0);
                let elapsed_str = format_time(elapsed);
                let duration_str = format_time(duration);
                let time_str = format!(" {} {}/{} ", status_icon, elapsed_str, duration_str);
                let time_width = time_str.chars().count() as u16;
                let bar_width = pb_area.width.saturating_sub(time_width + 2) as usize;
                let filled = (progress * bar_width as f32) as usize;
                let bar_str = format!(
                    "{}{}",
                    "█".repeat(filled),
                    "░".repeat(bar_width.saturating_sub(filled))
                );
                let max_name_width = pb_area.width.saturating_sub(time_width + bar_width as u16 + 4) as usize;
                let truncated_name: String = track_name.chars().take(max_name_width).collect();
                let pb_line = Line::from(vec![
                    Span::styled(time_str, Style::default().fg(Color::DarkGray)),
                    Span::styled(bar_str, Style::default().fg(Color::Cyan)),
                    Span::styled(format!("  {}", truncated_name), Style::default().fg(Color::DarkGray)),
                ]);
                f.render_widget(Paragraph::new(pb_line), pb_area);
            }
        }
        AppScreen::Queue => {
            let active_track = {
                let state = app.audio.shared_state.lock().unwrap();
                state.current_track.clone()
            };

            let queue_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(70),
                    Constraint::Percentage(30),
                ])
                .split(chunks[1]);

            let queue_border_style = Style::default().fg(Color::Cyan);
            let filtered_indices = app.get_filtered_queue_indices();

            let queue_list_items: Vec<ListItem> = filtered_indices
                .iter()
                .enumerate()
                .map(|(idx, &(base_idx, orig_idx))| {
                    let path = &app.queue.items[orig_idx];
                    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Unknown".to_string());
                    let is_highlighted = idx == app.queue_selected_index;
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

            // Mismo truco del scroll centrado que en el browser
            let h = queue_chunks[0].height.saturating_sub(2) as usize;
            let total = filtered_indices.len();
            let target_offset = if total <= h {
                0
            } else {
                let half = h / 2;
                if app.queue_selected_index < half {
                    0
                } else if app.queue_selected_index >= total.saturating_sub(half) {
                    total.saturating_sub(h)
                } else {
                    app.queue_selected_index.saturating_sub(half)
                }
            };
            *app.queue_list_state.offset_mut() = target_offset;
            app.queue_list_state.select(Some(app.queue_selected_index));
            f.render_stateful_widget(queue_list, queue_chunks[0], &mut app.queue_list_state);

            let info_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Magenta))
                .title(" Track Information ");

            let info_inner = info_block.inner(queue_chunks[1]);
            f.render_widget(info_block, queue_chunks[1]);

            if active_track.is_some() {
                let info_subchunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(12),
                        Constraint::Min(5),
                    ])
                    .split(info_inner);

                let cover_path = app.current_cover_path.clone();
                if let Some(ref c_path) = cover_path {
                    render_cover_preview(f, app, info_subchunks[0], c_path);
                } else {
                    let placeholder = Paragraph::new(vec![
                        Line::from(""),
                        Line::from(Span::styled("  [ No Album Art ]", Style::default().fg(Color::DarkGray))),
                    ]);
                    f.render_widget(placeholder, info_subchunks[0]);
                }

                let (metadata, lyrics_state) = {
                    let state = app.audio.shared_state.lock().unwrap();
                    (state.metadata.clone(), state.lyrics_state.clone())
                };

                let mut meta_lines = Vec::new();
                if let Some(ref meta) = metadata {
                    let title = meta.title.clone().unwrap_or_else(|| "Unknown".to_string());
                    let artist = meta.artist.clone().unwrap_or_else(|| "Unknown".to_string());
                    let album = meta.album.clone().unwrap_or_else(|| "Unknown".to_string());
                    let track = meta.track.map(|t| t.to_string()).unwrap_or_else(|| "Unknown".to_string());
                    let genre = meta.genre.clone().unwrap_or_else(|| "Unknown".to_string());
                    let year = meta.year.map(|y| y.to_string()).unwrap_or_else(|| "Unknown".to_string());

                    let bitrate = meta.bitrate.map(|b| format!("{} kbps", b)).unwrap_or_else(|| "Unknown".to_string());
                    let sample_rate = meta.sample_rate.map(|s| format!("{} Hz", s)).unwrap_or_else(|| "Unknown".to_string());
                    let length = meta.duration_secs.map(|d| format!("{}:{:02}", d / 60, d % 60)).unwrap_or_else(|| "Unknown".to_string());
                    let codec = meta.codec.clone().unwrap_or_else(|| "Unknown".to_string());

                    meta_lines.push(Line::from(vec![
                        Span::styled(" Title:       ", Style::default().fg(Color::Cyan)),
                        Span::styled(title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                    ]));
                    meta_lines.push(Line::from(vec![
                        Span::styled(" Artist:      ", Style::default().fg(Color::Cyan)),
                        Span::styled(artist, Style::default().fg(Color::Yellow)),
                    ]));
                    meta_lines.push(Line::from(vec![
                        Span::styled(" Album:       ", Style::default().fg(Color::Cyan)),
                        Span::styled(album, Style::default().fg(Color::White)),
                    ]));
                    meta_lines.push(Line::from(vec![
                        Span::styled(" Track:       ", Style::default().fg(Color::Cyan)),
                        Span::styled(track, Style::default().fg(Color::LightGreen)),
                    ]));
                    meta_lines.push(Line::from(vec![
                        Span::styled(" Genre:       ", Style::default().fg(Color::Cyan)),
                        Span::styled(genre, Style::default().fg(Color::LightBlue)),
                    ]));
                    meta_lines.push(Line::from(vec![
                        Span::styled(" Year:        ", Style::default().fg(Color::Cyan)),
                        Span::styled(year, Style::default().fg(Color::LightYellow)),
                    ]));
                    meta_lines.push(Line::from(vec![
                        Span::styled(" Length:      ", Style::default().fg(Color::Cyan)),
                        Span::styled(length, Style::default().fg(Color::White)),
                    ]));
                    meta_lines.push(Line::from(vec![
                        Span::styled(" Bitrate:     ", Style::default().fg(Color::Cyan)),
                        Span::styled(bitrate, Style::default().fg(Color::LightRed)),
                    ]));
                    meta_lines.push(Line::from(vec![
                        Span::styled(" Sample Rate: ", Style::default().fg(Color::Cyan)),
                        Span::styled(sample_rate, Style::default().fg(Color::LightMagenta)),
                    ]));
                    meta_lines.push(Line::from(vec![
                        Span::styled(" Codec:       ", Style::default().fg(Color::Cyan)),
                        Span::styled(codec, Style::default().fg(Color::White)),
                    ]));
                } else {
                    meta_lines.push(Line::from(""));
                    meta_lines.push(Line::from(Span::styled("  Reading metadata...", Style::default().fg(Color::Yellow))));
                }

                let meta_and_lyrics = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(11),
                        Constraint::Min(5),
                    ])
                    .split(info_subchunks[1]);

                let meta_para = Paragraph::new(meta_lines)
                    .wrap(ratatui::widgets::Wrap { trim: false });
                f.render_widget(meta_para, meta_and_lyrics[0]);

                let lyrics_border_style = if app.lyrics_focused {
                    Style::default().fg(Color::Magenta)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let lyrics_title = if app.lyrics_focused {
                    " Lyrics [Tab to exit] "
                } else {
                    " Lyrics [Tab to scroll] "
                };
                let lyrics_block = Block::default()
                    .borders(Borders::TOP)
                    .border_style(lyrics_border_style)
                    .title(lyrics_title);

                use crate::models::LyricsState;
                match &lyrics_state {
                    LyricsState::Found(text) => {
                        let lyrics_lines: Vec<Line> = text
                            .lines()
                            .map(|line| Line::from(Span::styled(line, Style::default().fg(Color::Gray))))
                            .collect();
                        let lyrics_para = Paragraph::new(lyrics_lines)
                            .block(lyrics_block)
                            .scroll((app.lyrics_scroll_offset as u16, 0))
                            .wrap(ratatui::widgets::Wrap { trim: false });
                        f.render_widget(lyrics_para, meta_and_lyrics[1]);
                    }
                    LyricsState::Loading => {
                        let lyrics_para = Paragraph::new(vec![
                            Line::from(""),
                            Line::from(Span::styled("  Loading metadata...", Style::default().fg(Color::Yellow))),
                        ])
                        .block(lyrics_block);
                        f.render_widget(lyrics_para, meta_and_lyrics[1]);
                    }
                    LyricsState::Fetching => {
                        let lyrics_para = Paragraph::new(vec![
                            Line::from(""),
                            Line::from(Span::styled("  Fetching lyrics online...", Style::default().fg(Color::Cyan))),
                        ])
                        .block(lyrics_block);
                        f.render_widget(lyrics_para, meta_and_lyrics[1]);
                    }
                    LyricsState::NotFound => {
                        let lyrics_para = Paragraph::new(vec![
                            Line::from(""),
                            Line::from(Span::styled("  No lyrics found", Style::default().fg(Color::DarkGray))),
                        ])
                        .block(lyrics_block);
                        f.render_widget(lyrics_para, meta_and_lyrics[1]);
                    }
                    LyricsState::Error(msg) => {
                        let lyrics_para = Paragraph::new(vec![
                            Line::from(""),
                            Line::from(Span::styled(format!("  Error fetching lyrics: {}", msg), Style::default().fg(Color::Red))),
                        ])
                        .block(lyrics_block)
                        .wrap(ratatui::widgets::Wrap { trim: false });
                        f.render_widget(lyrics_para, meta_and_lyrics[1]);
                    }
                }
            } else {
                let placeholder = Paragraph::new(vec![
                    Line::from(""),
                    Line::from(Span::styled("  No track loaded", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
                    Line::from(Span::styled("  Select an audio file from the Browser to play.", Style::default().fg(Color::DarkGray))),
                ]);
                f.render_widget(placeholder, info_inner);
            }
        }
        AppScreen::Library => {
            render_library(f, app, chunks[1]);
        }
        AppScreen::Healer => {
            render_healer(f, app, chunks[1]);
        }
    }

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

        // Interpolamos las barras del visualizador para que llenen el ancho disponible de la terminal
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
                    // Construimos el espectro fila por fila de abajo para arriba usando bloques Unicode graduales
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
                    Color::Red,
                    Color::Magenta,
                    Color::Yellow,
                    Color::Green,
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
                    // La onda simétrica: calculamos qué tan lejos está cada fila del centro (1.5)
                    for (row, line_str) in lines.iter_mut().enumerate() {
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
                        *line_str = line;
                    }
                }

                let row_colors = [
                    Color::Red,
                    Color::Magenta,
                    Color::Magenta,
                    Color::Red,
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

                let left_val = (audio_state.left_level * 1.3).min(1.0);
                let right_val = (audio_state.right_level * 1.3).min(1.0);

                let left_filled = (left_val * meter_width as f32) as usize;
                let right_filled = (right_val * meter_width as f32) as usize;

                // Closure que arma una línea de VU meter con colores verde/amarillo/rojo según nivel
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
                    Line::from(""),
                    build_vu_line("L ", left_filled),
                    build_vu_line("R ", right_filled),
                    Line::from(""),
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

        let player_widget = Paragraph::new(player_text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Audio Player "),
        );
        f.render_widget(player_widget, chunks[3]);
    }

    // Active screen tab: bright key + white label.  Inactive: dim number + dim label.
    let ak  = |s: &'static str| Span::styled(s, Style::default().fg(Color::Black).bg(Color::Cyan));
    let al  = |s: &'static str| Span::styled(s, Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    let ik  = |s: &'static str| Span::styled(s, Style::default().fg(Color::DarkGray));
    let sep = || Span::styled("  │  ", Style::default().fg(Color::DarkGray));
    let ck  = |s: &'static str| Span::styled(s, Style::default().fg(Color::Black).bg(Color::Cyan));
    let cl  = |s: &'static str| Span::styled(s, Style::default().fg(Color::White));

    let help_bar_spans = match app.screen {
        AppScreen::Browser => vec![
            ak(" 1 "), al(" Browser "),
            ik(" 2 "),
            ik(" 3 "),
            ik(" 4 "),
            sep(),
            ck(" Space "), cl(" Select "),
            ck(" q "), cl(" Queue "),
            ck(" v "), cl(" Copy "),
            ck(" a "), cl(" Playlist "),
            ck(" / "), cl(" Search "),
            ck(" ? "), cl(" Help "),
        ],
        AppScreen::Queue => vec![
            ik(" 1 "),
            ak(" 2 "), al(" Player "),
            ik(" 3 "),
            ik(" 4 "),
            sep(),
            ck(" Space "), cl(" Play/Pause "),
            ck(" n/b "), cl(" Next/Prev "),
            ck(" s "), cl(" Stop "),
            ck(" C "), cl(" Clear "),
            ck(" / "), cl(" Search "),
            ck(" ? "), cl(" Help "),
        ],
        AppScreen::Library => vec![
            ik(" 1 "),
            ik(" 2 "),
            ak(" 3 "), al(" Library "),
            ik(" 4 "),
            sep(),
            ck(" Tab "), cl(" Panels "),
            ck(" Enter "), cl(" Play All "),
            ck(" e "), cl(" Edit Tags "),
            ck(" s "), cl(" Sort "),
            ck(" / "), cl(" Filter "),
            ck(" ? "), cl(" Help "),
        ],
        AppScreen::Healer => vec![
            ik(" 1 "),
            ik(" 2 "),
            ik(" 3 "),
            ak(" 4 "), al(" Healer "),
            sep(),
            ck(" s "), cl(" Scan "),
            ck(" l "), cl(" Lookup "),
            ck(" Enter "), cl(" Apply "),
            ck(" Esc "), cl(" Back "),
            ck(" ? "), cl(" Help "),
        ],
    };

    let help_bar = Paragraph::new(Line::from(help_bar_spans));
    let help_chunk = if show_player_and_vis {
        chunks[4]
    } else {
        chunks[3]
    };
    f.render_widget(help_bar, help_chunk);

    if !show_player_and_vis {
        let update_state = app.update.lock().unwrap().clone();
        let sep_line = match update_state {
            crate::updater::UpdateProgress::Available { ref version, .. } => Line::from(vec![
                Span::styled("─── ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("New version {} available ", version),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(" U ", Style::default().fg(Color::Black).bg(Color::Yellow)),
                Span::styled(" Update ", Style::default().fg(Color::Yellow)),
                Span::styled(" ───", Style::default().fg(Color::DarkGray)),
            ]),
            crate::updater::UpdateProgress::Downloading { ref version, downloaded, total } => {
                let pct = if total > 0 { downloaded * 100 / total } else { 0 };
                let bar_width: usize = 20;
                let filled = (pct as usize * bar_width / 100).min(bar_width);
                let bar = format!(
                    "{}{}",
                    "█".repeat(filled),
                    "░".repeat(bar_width - filled)
                );
                Line::from(vec![
                    Span::styled("─── ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("Downloading {} ", version),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(bar, Style::default().fg(Color::Cyan)),
                    Span::styled(
                        format!(" {}% ───", pct),
                        Style::default().fg(Color::Cyan),
                    ),
                ])
            }
            crate::updater::UpdateProgress::Replacing => Line::from(Span::styled(
                "─── Replacing binary... ───",
                Style::default().fg(Color::Cyan),
            )),
            crate::updater::UpdateProgress::Done { ref version } => Line::from(vec![
                Span::styled("─── ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("Updated to {}! Restart stash to apply. ", version),
                    Style::default().fg(Color::Green),
                ),
                Span::styled("───", Style::default().fg(Color::DarkGray)),
            ]),
            crate::updater::UpdateProgress::Error(ref e) => Line::from(vec![
                Span::styled("─── ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("Update failed: {} ", e),
                    Style::default().fg(Color::Red),
                ),
                Span::styled("───", Style::default().fg(Color::DarkGray)),
            ]),
            _ => {
                if let Some((ref msg, _)) = app.notification {
                    let (icon, color) = if msg.starts_with("Already") {
                        ("○ ", Color::Yellow)
                    } else {
                        ("● ", Color::Green)
                    };
                    Line::from(vec![
                        Span::styled("─── ", Style::default().fg(Color::DarkGray)),
                        Span::styled(icon, Style::default().fg(color)),
                        Span::styled(msg.as_str(), Style::default().fg(color)),
                        Span::styled(" ───", Style::default().fg(Color::DarkGray)),
                    ])
                } else {
                    let sep = "─".repeat(chunks[2].width as usize);
                    Line::from(Span::styled(sep, Style::default().fg(Color::DarkGray)))
                }
            }
        };
        f.render_widget(Paragraph::new(sep_line), chunks[2]);
    }

    // Los overlays/diálogos van siempre al último para que queden encima de todo
    if app.input_mode == InputMode::CreateCollection {
        render_input_popup(f, " Create Collection ", "Enter collection name:", &app.input_value, app.input_cursor);
    } else if app.input_mode == InputMode::CopyPath || app.input_mode == InputMode::MovePath {
        if app.dest_browser.is_some() {
            render_dest_browser_popup(f, app);
        }
    } else if app.input_mode == InputMode::ConfirmDelete {
        render_confirm_popup(f, " Delete Selections ", "Are you sure you want to delete selected files? (y/n)");
    } else if app.input_mode == InputMode::Rename {
        render_input_popup(f, " Rename Item ", "Enter new name:", &app.input_value, app.input_cursor);
    } else if app.input_mode == InputMode::CreateFolder {
        render_input_popup(f, " Create Folder ", "Enter folder name:", &app.input_value, app.input_cursor);
    } else if app.input_mode == InputMode::AddToCollectionList {
        render_add_to_collection_popup(f, app);
    } else if app.input_mode == InputMode::ConfirmDeletePlaylist {
        let name = app.pending_delete_playlist.as_deref().unwrap_or("?");
        render_confirm_popup(f, " Delete Playlist ", &format!("Delete playlist '{}'? (y/n)", name));
    } else if app.input_mode == InputMode::ManageMusicFolders {
        render_manage_folders_popup(f, app);
    } else if app.input_mode == InputMode::TagEdit || (app.screen == AppScreen::Library && app.library.tag_editor.is_some()) {
        render_tag_editor_popup(f, app);
    } else if app.show_help {
        render_help_popup(f, app.help_scroll);
    } else if app.input_mode == InputMode::Search {
        // La barra de búsqueda es un overlay flotante en la parte de abajo del pane principal
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

        // Posicionamos el cursor del terminal justo después del texto del query
        let cursor_x = (search_rect.x + 9 + app.search.query.width() as u16)
            .min(search_rect.x + search_rect.width.saturating_sub(1));
        f.set_cursor(cursor_x, search_y);
    }

    if let Some(ref progress) = *app.file_progress.lock().unwrap() {
        render_progress_popup(f, progress);
        if progress.conflict_file.is_some() {
            render_conflict_popup(f, progress);
        }
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

// Calcula un Rect centrado en porcentaje tanto en X como en Y dentro de r
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

// Igual que centered_rect pero el alto es fijo en filas en lugar de porcentaje
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

fn render_dest_browser_popup(f: &mut Frame, app: &App) {
    let db = match &app.dest_browser {
        Some(s) => s,
        None => return,
    };

    let title = if db.is_move { " Move — Select Destination " } else { " Copy — Select Destination " };
    let area = centered_rect_fixed(90, 24, f.size());
    f.render_widget(Clear, area);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(title);

    let inner = popup_block.inner(area);
    f.render_widget(popup_block, area);

    let vchunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Min(1),
            ratatui::layout::Constraint::Length(3),
        ])
        .split(inner);

    let hchunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([
            ratatui::layout::Constraint::Percentage(30),
            ratatui::layout::Constraint::Percentage(70),
        ])
        .split(vchunks[0]);

    let left_focused = matches!(db.focus, DestBrowserFocus::Quick);
    let left_border_style = if left_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let left_block = Block::default()
        .borders(Borders::ALL)
        .border_style(left_border_style)
        .title(" Drives / MTP ");
    let left_inner = left_block.inner(hchunks[0]);
    f.render_widget(left_block, hchunks[0]);

    let quick_items: Vec<Line> = db.quick_paths.iter().enumerate().map(|(i, (label, _path))| {
        if i == db.quick_index && left_focused {
            Line::from(Span::styled(
                format!(" > {}", label),
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
            ))
        } else if i == db.quick_index {
            Line::from(Span::styled(
                format!(" > {}", label),
                Style::default().fg(Color::Yellow),
            ))
        } else {
            Line::from(Span::styled(
                format!("   {}", label),
                Style::default().fg(Color::White),
            ))
        }
    }).collect();

    // Scroll para que el item seleccionado nunca se vaya fuera del panel
    let scroll = db.quick_index.saturating_sub(left_inner.height.saturating_sub(1) as usize);
    f.render_widget(
        Paragraph::new(quick_items).scroll((scroll as u16, 0)),
        left_inner,
    );

    let right_focused = matches!(db.focus, DestBrowserFocus::Dirs);
    let right_border_style = if right_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let dir_title = format!(" {} ", db.current_dir.to_string_lossy());
    let right_block = Block::default()
        .borders(Borders::ALL)
        .border_style(right_border_style)
        .title(dir_title.as_str());
    let right_inner = right_block.inner(hchunks[1]);
    f.render_widget(right_block, hchunks[1]);

    if db.loading {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " Loading...",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            ))),
            right_inner,
        );
    } else {
        // dir_index 0 = "Copy here" virtual entry; 1..=N = actual subdirs
        let copy_here_selected = db.dir_index == 0;
        let op_label = if db.is_move { "Move here" } else { "Copy here" };
        let mut dir_items: Vec<Line> = Vec::new();

        // Virtual "Copy here" entry always at top
        dir_items.push(if copy_here_selected && right_focused {
            Line::from(Span::styled(
                format!(" ↵  {}", op_label),
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
            ))
        } else if copy_here_selected {
            Line::from(Span::styled(
                format!(" ↵  {}", op_label),
                Style::default().fg(Color::Yellow),
            ))
        } else {
            Line::from(Span::styled(
                format!("    {}", op_label),
                Style::default().fg(Color::DarkGray),
            ))
        });

        // Actual subdirectories (visual index = dir_index offset by 1)
        for (i, path) in db.dirs.iter().enumerate() {
            let dir_idx = i + 1;
            let name = path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            if dir_idx == db.dir_index && right_focused {
                dir_items.push(Line::from(Span::styled(
                    format!(" > {}/", name),
                    Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
                )));
            } else if dir_idx == db.dir_index {
                dir_items.push(Line::from(Span::styled(
                    format!(" > {}/", name),
                    Style::default().fg(Color::Cyan),
                )));
            } else {
                dir_items.push(Line::from(Span::styled(
                    format!("   {}/", name),
                    Style::default().fg(Color::White),
                )));
            }
        }

        if db.dirs.is_empty() {
            dir_items.push(Line::from(Span::styled(
                "   (no subdirectories)",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            )));
        }

        let dir_scroll = db.dir_scroll;
        f.render_widget(
            Paragraph::new(dir_items).scroll((dir_scroll as u16, 0)),
            right_inner,
        );
    }

    let dest_str = db.current_dir.to_string_lossy().into_owned();
    let bottom_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let bottom_inner = bottom_block.inner(vchunks[1]);
    f.render_widget(bottom_block, vchunks[1]);

    let hint_line = Line::from(vec![
        Span::styled(" Dest: ", Style::default().fg(Color::DarkGray)),
        Span::styled(dest_str, Style::default().fg(Color::White)),
    ]);
    let key_line = Line::from(vec![
        Span::styled(" Tab", Style::default().fg(Color::Black).bg(Color::DarkGray)),
        Span::styled(":switch  ", Style::default().fg(Color::DarkGray)),
        Span::styled("j/k", Style::default().fg(Color::Black).bg(Color::DarkGray)),
        Span::styled(":nav  ", Style::default().fg(Color::DarkGray)),
        Span::styled("l/Enter", Style::default().fg(Color::Black).bg(Color::DarkGray)),
        Span::styled(":open  ", Style::default().fg(Color::DarkGray)),
        Span::styled("h/Bsp", Style::default().fg(Color::Black).bg(Color::DarkGray)),
        Span::styled(":up  ", Style::default().fg(Color::DarkGray)),
        Span::styled("c", Style::default().fg(Color::Black).bg(Color::DarkGray)),
        Span::styled(":copy here  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Black).bg(Color::DarkGray)),
        Span::styled(":cancel", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(vec![hint_line, key_line]), bottom_inner);
}

fn render_conflict_popup(f: &mut Frame, progress: &crate::app::FileOperationProgress) {
    let filename = progress.conflict_file.as_deref().unwrap_or("");
    let area = centered_rect_fixed(62, 9, f.size());
    f.render_widget(Clear, area);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Magenta))
        .title(" File Already Exists ");

    let src_size = format_size(progress.conflict_src_size);
    let dest_size = format_size(progress.conflict_dest_size);

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  File: ", Style::default().fg(Color::DarkGray)),
            Span::styled(filename, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  Incoming: ", Style::default().fg(Color::DarkGray)),
            Span::styled(src_size, Style::default().fg(Color::Cyan)),
            Span::styled("   Existing: ", Style::default().fg(Color::DarkGray)),
            Span::styled(dest_size, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  [r] Replace  ", Style::default().fg(Color::Green)),
            Span::styled("[R] Replace All  ", Style::default().fg(Color::LightGreen)),
            Span::styled("[s] Skip  ", Style::default().fg(Color::Red)),
            Span::styled("[S] Skip All", Style::default().fg(Color::LightRed)),
        ]),
    ];

    let para = Paragraph::new(lines).block(popup_block);
    f.render_widget(para, area);
}

fn render_input_popup(f: &mut Frame, title: &str, label: &str, value: &str, cursor_pos: usize) {
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

    // Calculamos la posición X del cursor contando el ancho real en pantalla del substring antes del cursor
    let substring: String = value.chars().take(cursor_pos).collect();
    let cursor_x = (area.x + 3 + substring.width() as u16)
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

fn render_manage_folders_popup(f: &mut Frame, app: &App) {
    let height = (app.config.music_folders.len() as u16 + 6).max(8).min(24);
    let area = centered_rect_fixed(60, height, f.size());
    f.render_widget(Clear, area);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Music Folders  d/x remove  Esc close ");

    let inner = popup_block.inner(area);
    f.render_widget(popup_block, area);

    if app.config.music_folders.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No music folders configured.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Browse to a folder in the Browser and press m to add it.",
                Style::default().fg(Color::White),
            )),
        ]);
        f.render_widget(msg, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .config
        .music_folders
        .iter()
        .enumerate()
        .map(|(i, folder)| {
            let is_sel = i == app.manage_folders_index;
            let prefix = if is_sel { "> " } else { "  " };
            let style = if is_sel {
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::Cyan)),
                Span::styled(folder.as_str(), style),
            ]))
        })
        .collect();

    let hint = Line::from(vec![
        Span::styled("  Browse to a folder in Browser and press ", Style::default().fg(Color::DarkGray)),
        Span::styled("m", Style::default().fg(Color::Yellow)),
        Span::styled(" to add it", Style::default().fg(Color::DarkGray)),
    ]);

    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Min(1),
            ratatui::layout::Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(List::new(items), chunks[0]);
    f.render_widget(Paragraph::new(hint), chunks[1]);
}

fn render_add_to_collection_popup(f: &mut Frame, app: &mut App) {
    let area = centered_rect(50, 40, f.size());
    f.render_widget(Clear, area);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Add to Playlist ");

    let inner = popup_block.inner(area);
    f.render_widget(popup_block, area);

    // Split inner: list on top, input bar at bottom (shown only when creating)
    let input_height = if app.add_coll_creating { 3u16 } else { 0 };
    let hint_height = 1u16;
    let vchunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Min(1),
            ratatui::layout::Constraint::Length(input_height),
            ratatui::layout::Constraint::Length(hint_height),
        ])
        .split(inner);

    // Index 0 = "New playlist", 1..=N = existing collections (sorted)
    let mut coll_names: Vec<String> = app.collections.collections.keys().cloned().collect();
    coll_names.sort();

    let mut lines: Vec<Line> = Vec::new();

    // Virtual "New playlist" entry at index 0
    let new_selected = app.selected_add_collection_index == 0;
    lines.push(Line::from(vec![
        Span::styled(
            if new_selected { " > " } else { "   " },
            Style::default().fg(Color::Green),
        ),
        Span::styled(
            "+ New playlist",
            if new_selected {
                Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Green)
            },
        ),
    ]));

    // Existing collections at indices 1..=N
    for (i, name) in coll_names.iter().enumerate() {
        let idx = i + 1;
        let is_highlighted = idx == app.selected_add_collection_index;
        lines.push(Line::from(vec![
            Span::styled(
                if is_highlighted { " > " } else { "   " },
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(
                name.clone(),
                if is_highlighted {
                    Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            ),
        ]));
    }

    let scroll_offset = app.selected_add_collection_index.saturating_sub(
        vchunks[0].height.saturating_sub(1) as usize
    );
    f.render_widget(
        ratatui::widgets::Paragraph::new(lines).scroll((scroll_offset as u16, 0)),
        vchunks[0],
    );

    // Inline "New playlist" input bar
    if app.add_coll_creating {
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green))
            .title(" Playlist name ");
        let input_inner = input_block.inner(vchunks[1]);
        f.render_widget(input_block, vchunks[1]);
        let display = format!("{}_", app.input_value);
        f.render_widget(
            ratatui::widgets::Paragraph::new(display).style(Style::default().fg(Color::White)),
            input_inner,
        );
    }

    // Hint line
    let hint = if app.add_coll_creating {
        Line::from(vec![
            Span::styled(" Enter", Style::default().fg(Color::Black).bg(Color::DarkGray)),
            Span::styled(":create  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Black).bg(Color::DarkGray)),
            Span::styled(":back", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" j/k", Style::default().fg(Color::Black).bg(Color::DarkGray)),
            Span::styled(":nav  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::Black).bg(Color::DarkGray)),
            Span::styled(":select  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Black).bg(Color::DarkGray)),
            Span::styled(":cancel", Style::default().fg(Color::DarkGray)),
        ])
    };
    f.render_widget(ratatui::widgets::Paragraph::new(hint), vchunks[2]);
}

fn render_help_popup(f: &mut Frame, scroll: u16) {
    let area = centered_rect_fixed(70, (f.size().height).saturating_sub(4).min(52), f.size());
    f.render_widget(Clear, area);

    let h = |s: &'static str| Span::styled(s, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    let k = |s: &'static str| Span::styled(s, Style::default().fg(Color::Yellow));
    let d = |s: &'static str| Span::raw(s);

    let help_text = vec![
        Line::from(h("── Screens ────────────────────────────────────────────────")),
        Line::from(vec![k("  1 / 2 / 3 / 4    "), d("Browser / Queue / Library / Healer")]),
        Line::from(vec![k("  Q                "), d("Toggle Queue overlay from any screen")]),
        Line::from(""),
        Line::from(h("── Browser — Navigation ───────────────────────────────────")),
        Line::from(vec![k("  j / k  ↑ / ↓     "), d("Move cursor")]),
        Line::from(vec![k("  PgUp / PgDn       "), d("Jump 10 rows")]),
        Line::from(vec![k("  Home / End        "), d("First / last item")]),
        Line::from(vec![k("  l / →             "), d("Expand folder")]),
        Line::from(vec![k("  ← / Backspace     "), d("Collapse folder / go to parent")]),
        Line::from(vec![k("  Tab / Shift+Tab   "), d("Cycle focus: Files → Preview")]),
        Line::from(""),
        Line::from(h("── Browser — File Operations ──────────────────────────────")),
        Line::from(vec![k("  Space             "), d("Toggle selection on file or folder")]),
        Line::from(vec![k("  Shift+↑ / ↓       "), d("Range-select multiple items")]),
        Line::from(vec![k("  *                 "), d("Select all / deselect all")]),
        Line::from(vec![k("  q                 "), d("Add selection / highlighted item to queue")]),
        Line::from(vec![k("  a                 "), d("Add selection to a playlist")]),
        Line::from(vec![k("  m                 "), d("Add highlighted folder to music library")]),
        Line::from(vec![k("  v                 "), d("Copy selection (opens destination browser)")]),
        Line::from(vec![k("  y                 "), d("Move selection (opens destination browser)")]),
        Line::from(vec![k("  Y                 "), d("Copy path(s) to system clipboard")]),
        Line::from(vec![k("  d                 "), d("Delete selection (asks confirmation)")]),
        Line::from(vec![k("  F2                "), d("Rename highlighted file or folder")]),
        Line::from(vec![k("  n                 "), d("Create new folder")]),
        Line::from(vec![k("  h                 "), d("Toggle hidden files")]),
        Line::from(vec![k("  M                 "), d("Jump to MTP device mounts (Android / USB)")]),
        Line::from(vec![k("  E                 "), d("Jump to external drives")]),
        Line::from(vec![k("  /                 "), d("Search files")]),
        Line::from(""),
        Line::from(h("── Library ────────────────────────────────────────────────")),
        Line::from(vec![k("  Tab / Shift+Tab   "), d("Switch Playlists ↔ Tracks panel")]),
        Line::from(vec![k("  j / k  ↑ / ↓     "), d("Navigate")]),
        Line::from(vec![k("  PgUp / PgDn       "), d("Jump 10 rows")]),
        Line::from(vec![k("  Enter             "), d("Play all visible tracks from cursor")]),
        Line::from(vec![k("  Space             "), d("Append track to queue")]),
        Line::from(vec![k("  e                 "), d("Open tag editor for selected track")]),
        Line::from(vec![k("  s                 "), d("Cycle sort: Default → Title → Artist → Album → Year → Duration")]),
        Line::from(vec![k("  a                 "), d("Add track to a playlist")]),
        Line::from(vec![k("  N                 "), d("New playlist (Playlists panel)")]),
        Line::from(vec![k("  D                 "), d("Delete playlist (with confirmation)")]),
        Line::from(vec![k("  x                 "), d("Remove track from current playlist")]),
        Line::from(vec![k("  R                 "), d("Re-scan music folders")]),
        Line::from(vec![k("  F                 "), d("Manage music folders")]),
        Line::from(vec![k("  /                 "), d("Filter by title / artist / album / filename")]),
        Line::from(""),
        Line::from(h("── Tag Editor (Library → e) ───────────────────────────────")),
        Line::from(vec![k("  Tab / Shift+Tab   "), d("Move between fields")]),
        Line::from(vec![k("  Enter             "), d("Start typing in focused field")]),
        Line::from(vec![k("  Esc               "), d("Stop typing / close editor")]),
        Line::from(vec![k("  w                 "), d("Save tags to file")]),
        Line::from(""),
        Line::from(h("── Playback ───────────────────────────────────────────────")),
        Line::from(vec![k("  Space  (Queue)    "), d("Pause / Resume")]),
        Line::from(vec![k("  p / P             "), d("Pause / Resume (any screen)")]),
        Line::from(vec![k("  n / b             "), d("Next / Previous track")]),
        Line::from(vec![k("  s                 "), d("Stop playback (Browser / Queue)")]),
        Line::from(vec![k("  Shift+← / →       "), d("Seek −5 / +5 seconds")]),
        Line::from(vec![k("  H / L             "), d("Seek −5 / +5 seconds (Vim style)")]),
        Line::from(vec![k("  + / -             "), d("Volume up / down")]),
        Line::from(vec![k("  r                 "), d("Cycle Repeat: Off → All → One")]),
        Line::from(vec![k("  z                 "), d("Toggle Shuffle")]),
        Line::from(""),
        Line::from(h("── Queue ──────────────────────────────────────────────────")),
        Line::from(vec![k("  d / x             "), d("Remove selected item")]),
        Line::from(vec![k("  K / J  Ctrl+↑/↓   "), d("Move item up / down")]),
        Line::from(vec![k("  C                 "), d("Clear entire queue")]),
        Line::from(vec![k("  v                 "), d("Cycle visualizer: Spectrum → Waveform → Levels")]),
        Line::from(vec![k("  [ / ]             "), d("Visualizer decay slower / faster")]),
        Line::from(vec![k("  PgUp / PgDn       "), d("Scroll queue / lyrics")]),
        Line::from(""),
        Line::from(h("── Library Healer (4) ─────────────────────────────────────")),
        Line::from(vec![k("  s / Enter         "), d("Start scan  (Menu)")]),
        Line::from(vec![k("  r                 "), d("View report  (Menu)")]),
        Line::from(vec![k("  f                 "), d("Go to file list  (Menu / Report)")]),
        Line::from(vec![k("  j / k  PgUp/PgDn  "), d("Navigate file list")]),
        Line::from(vec![k("  l                 "), d("Lookup tags online  (FileList / Preview)")]),
        Line::from(vec![k("  Enter             "), d("Preview matches  (FileList)")]),
        Line::from(vec![k("  ← / →             "), d("Cycle between matches  (Preview)")]),
        Line::from(vec![k("  Enter / a          "), d("Apply highlighted match  (Preview)")]),
        Line::from(vec![k("  e                 "), d("Open tag editor  (FileList / Preview)")]),
        Line::from(vec![k("  w / s             "), d("Save tags  (Editor)")]),
        Line::from(vec![k("  s                 "), d("Skip file  (FileList / Preview)")]),
        Line::from(vec![k("  Esc / q           "), d("Back")]),
        Line::from(""),
        Line::from(h("── General ────────────────────────────────────────────────")),
        Line::from(vec![k("  U                 "), d("Apply pending self-update (shown in title bar)")]),
        Line::from(vec![k("  ?                 "), d("Toggle this help  —  j / k or ↑↓ to scroll")]),
        Line::from(vec![k("  Esc               "), d("Close dialog / cancel")]),
        Line::from(vec![k("  q / Ctrl+C        "), d("Quit")]),
    ];

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Keyboard Shortcuts — j/k scroll, any other key to close ");

    let help_para = Paragraph::new(help_text)
        .block(popup_block)
        .scroll((scroll, 0));
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

    // Si tenemos info de bytes jalamos porcentaje por bytes; si no, por archivos completados
    let percent = if progress.total_bytes > 0 {
        (progress.bytes_copied as f64 / progress.total_bytes as f64 * 100.0).min(100.0) as u16
    } else if progress.total_files > 0 {
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

    let progress_details_str = if progress.total_bytes > 0 {
        format!(
            " {} {} of {} files ({} / {}) - {}%",
            progress.op_type,
            progress.completed_files,
            progress.total_files,
            format_size(progress.bytes_copied),
            format_size(progress.total_bytes),
            percent
        )
    } else {
        format!(
            " {} {} of {} files...",
            progress.op_type,
            progress.completed_files,
            progress.total_files
        )
    };

    let mut details = vec![
        Line::from(""),
        Line::from(progress_details_str),
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

    // Si el hilo de carga todavía está trabajando en este path, mostramos el indicador y nos vamos
    let is_loading = app.loading_image.try_lock()
        .map(|lock| matches!(&*lock, Some((p, None)) if p == path))
        .unwrap_or(false);
    if is_loading || app.current_image_data.as_ref().map(|(p, _)| p != path).unwrap_or(true) {
        f.render_widget(
            Paragraph::new("Loading preview...").style(Style::default().fg(Color::Yellow)),
            area,
        );
        return;
    }

    if let Some(picker_fs) = app.picker.as_ref().map(|p| p.font_size) {
        let cached = app.current_image_protocol.as_ref()
            .map(|(p, w, h, _)| p == path && *w == area.width && *h == area.height)
            .unwrap_or(false);

        if !cached
            && let Some((_, ref img)) = app.current_image_data {
                // ratatui-image con Resize::Fit solo achica, nunca agranda. Por eso
                // pre-escalamos al tamaño del pane en píxeles antes de pasárselo al picker,
                // así imágenes chicas (thumbnails) llenan el área igual que las grandes.
                let px_w = area.width as u32 * picker_fs.0 as u32;
                let px_h = area.height as u32 * picker_fs.1 as u32;
                let sized = img.resize(px_w, px_h, image::imageops::FilterType::CatmullRom);
                if let Some(picker) = app.picker.as_mut() {
                    let proto = picker.new_resize_protocol(sized);
                    app.current_image_protocol = Some((path.clone(), area.width, area.height, proto));
                }
            }

        if let Some((_, _, _, ref mut proto)) = app.current_image_protocol {
            f.render_stateful_widget(ratatui_image::ResizeImage::new(None), area, proto);
            return;
        }
    }

    // Fallback halfblock: si no hay picker (terminal sin soporte gráfico), pintamos con ▄ RGB
    let cached = app.current_image_lines.as_ref()
        .map(|(p, w, h, _)| p == path && *w == area.width && *h == area.height)
        .unwrap_or(false);

    if cached {
        if let Some((_, _, _, ref lines)) = app.current_image_lines {
            let res_w = lines.first().map(|l| l.width() as u16).unwrap_or(0);
            let res_h = lines.len() as u16;
            let dx = area.x + (area.width.saturating_sub(res_w)) / 2;
            let dy = area.y + (area.height.saturating_sub(res_h)) / 2;
            for (y, line) in lines.iter().enumerate() {
                f.render_widget(Paragraph::new(line.clone()), Rect::new(dx, dy + y as u16, res_w, 1));
            }
        }
        return;
    }

    if let Some((_, ref img)) = app.current_image_data {
        // Cada carácter ▄ representa dos filas de píxeles: fg = pixel de abajo, bg = pixel de arriba
        let area_w = area.width as u32;
        let area_h = (area.height as u32).saturating_mul(2);
        let resized = img.resize(area_w, area_h, image::imageops::FilterType::Triangle);
        let res_w = resized.width() as u16;
        let res_h = resized.height() as u16;
        let dx = area.x + (area.width.saturating_sub(res_w)) / 2;
        let dy = area.y + (area.height.saturating_sub(res_h / 2)) / 2;
        let mut lines = Vec::new();
        for y in 0..(res_h / 2) {
            let mut spans = Vec::with_capacity(res_w as usize);
            for x in 0..res_w {
                let p_top = resized.get_pixel(x as u32, y as u32 * 2);
                let p_bot = if y as u32 * 2 + 1 < res_h as u32 {
                    resized.get_pixel(x as u32, y as u32 * 2 + 1)
                } else {
                    p_top
                };
                spans.push(Span::styled(
                    "▄",
                    Style::default()
                        .fg(Color::Rgb(p_bot[0], p_bot[1], p_bot[2]))
                        .bg(Color::Rgb(p_top[0], p_top[1], p_top[2])),
                ));
            }
            lines.push(Line::from(spans));
        }
        app.current_image_lines = Some((path.clone(), area.width, area.height, lines.clone()));
        for (y, line) in lines.iter().enumerate() {
            f.render_widget(Paragraph::new(line.clone()), Rect::new(dx, dy + y as u16, res_w, 1));
        }
    }
}

fn render_cover_preview(f: &mut Frame, app: &mut App, area: Rect, path: &PathBuf) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let is_loading = app.loading_cover.try_lock()
        .map(|lock| matches!(&*lock, Some((p, None)) if p == path))
        .unwrap_or(false);
    if is_loading || app.current_cover_data.as_ref().map(|(p, _)| p != path).unwrap_or(true) {
        f.render_widget(
            Paragraph::new("Loading preview...").style(Style::default().fg(Color::Yellow)),
            area,
        );
        return;
    }

    if let Some(picker_fs) = app.picker.as_ref().map(|p| p.font_size) {
        let cached = app.current_cover_protocol.as_ref()
            .map(|(p, w, h, _)| p == path && *w == area.width && *h == area.height)
            .unwrap_or(false);

        if !cached
            && let Some((_, ref img)) = app.current_cover_data {
                let px_w = area.width as u32 * picker_fs.0 as u32;
                let px_h = area.height as u32 * picker_fs.1 as u32;
                let sized = img.resize(px_w, px_h, image::imageops::FilterType::CatmullRom);
                if let Some(picker) = app.picker.as_mut() {
                    let proto = picker.new_resize_protocol(sized);
                    app.current_cover_protocol = Some((path.clone(), area.width, area.height, proto));
                }
            }

        if let Some((_, _, _, ref mut proto)) = app.current_cover_protocol {
            f.render_stateful_widget(ratatui_image::ResizeImage::new(None), area, proto);
            return;
        }
    }

    // Mismo fallback halfblock que render_image_preview
    let cached = app.current_cover_lines.as_ref()
        .map(|(p, w, h, _)| p == path && *w == area.width && *h == area.height)
        .unwrap_or(false);

    if cached {
        if let Some((_, _, _, ref lines)) = app.current_cover_lines {
            let res_w = lines.first().map(|l| l.width() as u16).unwrap_or(0);
            let res_h = lines.len() as u16;
            let dx = area.x + (area.width.saturating_sub(res_w)) / 2;
            let dy = area.y + (area.height.saturating_sub(res_h)) / 2;
            for (y, line) in lines.iter().enumerate() {
                f.render_widget(Paragraph::new(line.clone()), Rect::new(dx, dy + y as u16, res_w, 1));
            }
        }
        return;
    }

    if let Some((_, ref img)) = app.current_cover_data {
        use image::GenericImageView;
        let area_w = area.width as u32;
        let area_h = (area.height as u32).saturating_mul(2);
        let resized = img.resize(area_w, area_h, image::imageops::FilterType::Triangle);
        let res_w = resized.width() as u16;
        let res_h = resized.height() as u16;
        let dx = area.x + (area.width.saturating_sub(res_w)) / 2;
        let dy = area.y + (area.height.saturating_sub(res_h / 2)) / 2;
        let mut lines = Vec::new();
        for y in 0..(res_h / 2) {
            let mut spans = Vec::with_capacity(res_w as usize);
            for x in 0..res_w {
                let p_top = resized.get_pixel(x as u32, y as u32 * 2);
                let p_bot = if y as u32 * 2 + 1 < res_h as u32 {
                    resized.get_pixel(x as u32, y as u32 * 2 + 1)
                } else {
                    p_top
                };
                spans.push(Span::styled(
                    "▄",
                    Style::default()
                        .fg(Color::Rgb(p_bot[0], p_bot[1], p_bot[2]))
                        .bg(Color::Rgb(p_top[0], p_top[1], p_top[2])),
                ));
            }
            lines.push(Line::from(spans));
        }
        app.current_cover_lines = Some((path.clone(), area.width, area.height, lines.clone()));
        for (y, line) in lines.iter().enumerate() {
            f.render_widget(Paragraph::new(line.clone()), Rect::new(dx, dy + y as u16, res_w, 1));
        }
    }
}

// Parsea un archivo .desktop y muestra sus campos clave de forma legible en lugar del texto crudo
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
    if area.height == 0 || area.width == 0 {
        return;
    }

    let cached = app.current_text_data.as_ref()
        .map(|(p, _)| p == path)
        .unwrap_or(false);

    if !cached {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled("  Loading preview...", Style::default().fg(Color::Yellow))),
            ]),
            area,
        );
        return;
    }

    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Los .desktop los manejamos aparte con su propio render especializado
    if ext == "desktop" {
        if let Some((_, ref lines)) = app.current_text_data {
            let lines = lines.clone();
            render_desktop_preview(f, app, area, &lines);
        }
        return;
    }

    let total_lines = app.current_text_data.as_ref().map(|(_, l)| l.len()).unwrap_or(0);
    let viewport_height = area.height as usize;
    let start_idx = app.text_scroll_offset.min(total_lines.saturating_sub(1));
    // Solo pintamos las líneas visibles, no hay buffer extra
    let end_idx = (start_idx + viewport_height).min(total_lines);

    let list_items: Vec<Line> = if let Some((_, ref lines)) = app.current_text_data {
        lines[start_idx..end_idx]
            .iter()
            .enumerate()
            .map(|(i, line_content)| {
                let line_num = start_idx + i + 1;
                let num_span = Span::styled(
                    format!("{:>4} │ ", line_num),
                    Style::default().fg(Color::DarkGray),
                );
                let mut spans = vec![num_span];
                spans.extend(highlight_line(line_content, &ext));
                Line::from(spans)
            })
            .collect()
    } else {
        vec![]
    };

    f.render_widget(Paragraph::new(list_items), area);
}

// Tokenizador minimalista para syntax highlighting en el preview de código.
// No es un parser completo, pero cubre keywords, strings, números y comentarios
// para los lenguajes más comunes.
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

fn render_library(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(28), Constraint::Percentage(72)].as_ref())
        .split(area);

    render_library_playlists(f, app, chunks[0]);
    render_library_tracks(f, app, chunks[1]);  // needs &mut for ListState
}

fn render_library_playlists(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.library.focused_panel == LibraryPanel::Playlists;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let names = LibraryState::playlist_names(&app.collections);
    let filter_query = if app.search.active { app.search.query.as_str() } else { "" };
    let total_tracks = app.library.tracks.len();

    let items: Vec<ListItem> = names.iter().enumerate().map(|(i, name)| {
        let is_sel = i == app.library.playlist_index;
        let smart = is_smart_playlist(name.as_str());

        let label = if i == 0 {
            format!(" {} ({})", name, total_tracks)
        } else if smart {
            if name == "Stats" {
                format!(" ~ {}", name)
            } else if is_sel {
                let count = app.library.visible_tracks(&app.collections, "", &app.stats).len();
                format!(" ~ {} ({})", name, count)
            } else {
                format!(" ~ {}", name)
            }
        } else if let Some(paths) = app.collections.collections.get(name.as_str()) {
            format!(" {} ({})", name, paths.len())
        } else {
            format!(" {} (0)", name)
        };

        let style = if is_sel && focused {
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else if is_sel {
            Style::default().fg(Color::Cyan)
        } else if smart {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };
        ListItem::new(Line::from(Span::styled(label, style)))
    }).collect();

    let title = if filter_query.is_empty() {
        " Playlists ".to_string()
    } else {
        format!(" Playlists  / {} ", filter_query)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title);

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn render_library_tracks(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.library.focused_panel == LibraryPanel::Tracks;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let filter_query = if app.search.active { app.search.query.as_str() } else { "" };

    // Stats panel replaces the track list
    let playlist_name = LibraryState::playlist_names(&app.collections)
        .get(app.library.playlist_index)
        .cloned()
        .unwrap_or_default();
    if playlist_name == "Stats" {
        render_stats_panel(f, app, area);
        return;
    }

    let visible = app.library.visible_tracks(&app.collections, filter_query, &app.stats);

    let active_track = {
        let state = app.audio.shared_state.lock().unwrap();
        state.current_track.clone()
    };

    let col_w = area.width.saturating_sub(2) as usize;

    let items: Vec<ListItem> = visible.iter().enumerate().map(|(i, track)| {
        let is_sel = i == app.library.track_index;
        let is_playing = active_track.as_ref().map(|p| p == &track.path).unwrap_or(false);

        let track_num = track.track.map(|n| format!("{:02}", n)).unwrap_or_else(|| "--".to_string());
        let artist = track.artist.as_deref().unwrap_or("Unknown Artist");
        let title = track.title.as_deref().unwrap_or_else(|| {
            track.path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
        });
        let dur = track.duration_secs.map(format_time).unwrap_or_else(|| "--:--".to_string());

        // For stats-based smart playlists, show play/skip count instead of album
        let third_col: String = match playlist_name.as_str() {
            "Most Played" | "Top 100" => {
                let n = app.stats.tracks.get(&track.path).map(|s| s.play_count).unwrap_or(0);
                format!("{} plays", n)
            }
            "Most Skipped" => {
                let n = app.stats.tracks.get(&track.path).map(|s| s.skip_count).unwrap_or(0);
                format!("{} skips", n)
            }
            _ => track.album.as_deref().unwrap_or("").to_string(),
        };

        // Layout: [♪/space][#][artist][title][album/count][dur]
        let icon = if is_playing { "♪ " } else { "  " };
        let track_col = format!("{:3} ", track_num);
        let dur_col = format!("{:6}", dur);
        // Remaining space for artist/title/third_col
        let left = col_w.saturating_sub(2 + 4 + 6 + 2);
        let artist_w = left / 4;
        let title_w = left / 2;
        let album_w = left.saturating_sub(artist_w + title_w);
        let artist_str = truncate_str(artist, artist_w);
        let title_str = truncate_str(title, title_w);
        let album_str = truncate_str(&third_col, album_w);

        let text = format!("{}{}{:<aw$}{:<tw$}{:<bw$} {}",
            icon, track_col,
            artist_str, title_str, album_str,
            dur_col,
            aw = artist_w, tw = title_w, bw = album_w
        );

        let style = if is_sel && focused {
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else if is_sel {
            Style::default().fg(Color::Cyan)
        } else if is_playing {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::White)
        };

        ListItem::new(Line::from(Span::styled(text, style)))
    }).collect();

    let scan_suffix = match app.library.scan_state {
        ScanState::Scanning => " [Scanning…]",
        ScanState::Done => "",
        ScanState::Idle => "",
    };
    let count = visible.len();
    let filter_suffix = if filter_query.is_empty() {
        String::new()
    } else {
        format!(" / {} ({} results)", filter_query, count)
    };
    let sort_label = if app.library.sort != LibrarySort::Default {
        format!(" [Sort: {}]", app.library.sort.label())
    } else {
        String::new()
    };
    let title = format!(" Library — {} tracks{}{}{}  [s] Sort ", app.library.tracks.len(), filter_suffix, sort_label, scan_suffix);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title);

    if app.library.tracks.is_empty() && app.library.scan_state == ScanState::Idle {
        let msg = if app.config.music_folders.is_empty() {
            "Add music_folders to ~/.config/stash/config.json and press R to scan"
        } else {
            "Press R to scan your music library"
        };
        let p = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", msg),
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            )),
        ]).block(block);
        f.render_widget(p, area);
        return;
    }

    let list = List::new(items).block(block);

    // Centered scroll: keep selected track in the middle of the visible area
    let h = area.height.saturating_sub(2) as usize;
    let total = visible.len();
    let sel = app.library.track_index;
    let target_offset = if total <= h {
        0
    } else {
        let half = h / 2;
        if sel < half {
            0
        } else if sel >= total.saturating_sub(half) {
            total.saturating_sub(h)
        } else {
            sel.saturating_sub(half)
        }
    };
    *app.library_track_list_state.offset_mut() = target_offset;
    app.library_track_list_state.select(Some(sel));
    f.render_stateful_widget(list, area, &mut app.library_track_list_state);
}

fn render_stats_panel(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.library.focused_panel == LibraryPanel::Tracks;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(" Listening Stats ");

    let stats = &app.stats;
    let tracks = &app.library.tracks;

    let total_secs = stats.total_listen_secs();
    let total_h = total_secs / 3600;
    let total_m = (total_secs % 3600) / 60;

    let avg_secs = stats.avg_session_secs();
    let avg_h = avg_secs / 3600;
    let avg_m = (avg_secs % 3600) / 60;

    let longest = stats.longest_session_secs();
    let long_h = longest / 3600;
    let long_m = (longest % 3600) / 60;

    let most_played = stats.most_played_paths(1).into_iter().next()
        .and_then(|(path, count)| {
            let name = tracks.iter().find(|t| t.path == path)
                .and_then(|t| t.title.clone())
                .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))?;
            Some(format!("{} ({} plays)", name, count))
        })
        .unwrap_or_else(|| "—".to_string());

    let most_skipped = stats.most_skipped_paths(1).into_iter().next()
        .and_then(|(path, count)| {
            let name = tracks.iter().find(|t| t.path == path)
                .and_then(|t| t.title.clone())
                .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))?;
            Some(format!("{} ({} skips)", name, count))
        })
        .unwrap_or_else(|| "—".to_string());

    let fav_genre = favorite_genre(stats, tracks).unwrap_or_else(|| "—".to_string());

    let label = |s: &str| Span::styled(format!("  {:22}", s), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    let value = |s: String| Span::styled(s, Style::default().fg(Color::White));
    let dim_value = |s: String| Span::styled(s, Style::default().fg(Color::Gray));

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(vec![label("Hours Listened:"), value(format!("{}h {}m", total_h, total_m))]),
        Line::from(vec![label("Total Plays:"), value(stats.total_plays().to_string())]),
        Line::from(vec![label("Total Skips:"), value(stats.total_skips().to_string())]),
        Line::from(""),
        Line::from(vec![label("Sessions:"), value(stats.sessions.len().to_string())]),
        Line::from(vec![label("Avg Session:"), if avg_secs == 0 { dim_value("—".to_string()) } else { value(if avg_h > 0 { format!("{}h {}m", avg_h, avg_m) } else { format!("{}m", avg_m) }) }]),
        Line::from(vec![label("Longest Session:"), if longest == 0 { dim_value("—".to_string()) } else { value(if long_h > 0 { format!("{}h {}m", long_h, long_m) } else { format!("{}m", long_m) }) }]),
        Line::from(""),
        Line::from(vec![label("Most Played:"), Span::styled(most_played, Style::default().fg(Color::Green))]),
        Line::from(vec![label("Most Skipped:"), Span::styled(most_skipped, Style::default().fg(Color::Yellow))]),
        Line::from(vec![label("Favorite Genre:"), Span::styled(fav_genre, Style::default().fg(Color::Magenta))]),
    ];

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

fn render_tag_editor_popup(f: &mut Frame, app: &App) {
    let editor = match &app.library.tag_editor {
        Some(e) => e,
        None => return,
    };

    let area = centered_rect_fixed(72, 16, f.size());
    f.render_widget(Clear, area);

    let popup_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" Tag Editor — {} ", editor.path.file_name().and_then(|n| n.to_str()).unwrap_or("?")));

    let inner = popup_block.inner(area);
    f.render_widget(popup_block, area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    for (i, field_name) in TAG_FIELD_NAMES.iter().enumerate() {
        let is_active = i == editor.active_field;
        let is_editing = is_active && app.input_mode == InputMode::TagEdit;
        let value = &editor.fields[i];

        let label_style = if is_active {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let value_style = if is_editing {
            Style::default().fg(Color::Yellow)
        } else if is_active {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::White)
        };
        let prefix = if is_active { "> " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(format!("{}  {:<9}: ", prefix, field_name), label_style),
            Span::styled(value.as_str(), value_style),
            if is_editing { Span::styled("█", Style::default().fg(Color::Yellow)) } else { Span::raw("") },
        ]));
    }
    lines.push(Line::from(""));

    let status_line = if let Some(ref result) = editor.save_result {
        match result {
            Ok(()) => Line::from(Span::styled("  Saved successfully!", Style::default().fg(Color::Green))),
            Err(e) => Line::from(Span::styled(format!("  Error: {}", e), Style::default().fg(Color::Red))),
        }
    } else {
        Line::from(vec![
            Span::styled("  Enter", Style::default().fg(Color::Cyan)),
            Span::styled(": edit  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Tab", Style::default().fg(Color::Cyan)),
            Span::styled(": next field  ", Style::default().fg(Color::DarkGray)),
            Span::styled("w", Style::default().fg(Color::Cyan)),
            Span::styled(": save  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Cyan)),
            Span::styled(": close", Style::default().fg(Color::DarkGray)),
        ])
    };
    lines.push(status_line);

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);

    // Position terminal cursor when editing
    if app.input_mode == InputMode::TagEdit {
        let field_line_y = inner.y + 1 + editor.active_field as u16 + 1;
        let prefix_width = 2 + 2 + 9 + 2; // "> " + "  " + fieldname padded + ": "
        let value_before: String = editor.fields[editor.active_field].chars().take(editor.cursor_pos).collect();
        let cursor_x = (inner.x + prefix_width + value_before.width() as u16)
            .min(inner.x + inner.width.saturating_sub(2));
        f.set_cursor(cursor_x, field_line_y);
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if max == 0 { return String::new(); }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        format!("{:<width$}", s, width = max)
    } else {
        let mut result: String = chars[..max.saturating_sub(1)].iter().collect();
        result.push('…');
        result
    }
}

// ── Library Healer UI ─────────────────────────────────────────────────────────

fn render_healer(f: &mut Frame, app: &mut App, area: Rect) {
    match app.healer.screen {
        HealerScreen::Menu     => render_healer_menu(f, app, area),
        HealerScreen::Scanning => render_healer_scanning(f, app, area),
        HealerScreen::Report   => render_healer_report(f, app, area),
        HealerScreen::FileList => render_healer_filelist(f, app, area),
        HealerScreen::Preview  => render_healer_preview(f, app, area),
        HealerScreen::Editor   => render_healer_editor(f, app, area),
    }
}

fn render_healer_menu(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Library Healer ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));

    let scan_status = match app.healer.scan_state {
        HealScanState::Idle     => "Not scanned yet",
        HealScanState::Scanning => "Scanning...",
        HealScanState::Done     => "Scan complete",
    };
    let file_count = app.healer.files.len();

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("  Library Healer scans your music files for missing metadata", Style::default().fg(Color::Gray))),
        Line::from(Span::styled("  and suggests fixes using filename patterns and MusicBrainz.", Style::default().fg(Color::Gray))),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Status: ", Style::default().fg(Color::DarkGray)),
            Span::styled(scan_status, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("  Files with issues: ", Style::default().fg(Color::DarkGray)),
            Span::styled(file_count.to_string(), Style::default().fg(Color::Red)),
        ]),
        Line::from(""),
        Line::from(Span::styled("  [s/Enter]  Scan library", Style::default().fg(Color::Cyan))),
        Line::from(Span::styled("  [r]        View report", Style::default().fg(Color::Cyan))),
        Line::from(Span::styled("  [f]        Browse files with issues", Style::default().fg(Color::Cyan))),
        Line::from(""),
        Line::from(Span::styled("  [1] Browser  [2] Queue  [3] Library  [Esc] Back", Style::default().fg(Color::DarkGray))),
    ];

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

fn render_healer_scanning(f: &mut Frame, app: &App, area: Rect) {
    let (done, total, current_file) = {
        let p = app.healer.scan_progress.lock().unwrap();
        (p.0, p.1, p.2.clone())
    };

    let block = Block::default()
        .title(" Scanning Library... ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));

    let progress_pct = if total > 0 { done * 100 / total } else { 0 };
    let bar_width = area.width.saturating_sub(10) as usize;
    let filled = bar_width * progress_pct / 100;
    let bar = format!("[{}{}]", "=".repeat(filled), " ".repeat(bar_width.saturating_sub(filled)));

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(format!("  Scanning: {}/{}", done, total), Style::default().fg(Color::White))),
        Line::from(""),
        Line::from(Span::styled(format!("  {}", bar), Style::default().fg(Color::Green))),
        Line::from(""),
        Line::from(Span::styled(format!("  {}", current_file), Style::default().fg(Color::DarkGray))),
        Line::from(""),
        Line::from(Span::styled("  [Esc] Cancel", Style::default().fg(Color::DarkGray))),
    ];

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_healer_report(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Health Report ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));

    if let Some(ref r) = app.healer.report {
        let sick = r.total.saturating_sub(r.healthy);
        let lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("  Total scanned:     ", Style::default().fg(Color::DarkGray)),
                Span::styled(r.total.to_string(), Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("  Healthy:           ", Style::default().fg(Color::DarkGray)),
                Span::styled(r.healthy.to_string(), Style::default().fg(Color::Green)),
            ]),
            Line::from(vec![
                Span::styled("  Files with issues: ", Style::default().fg(Color::DarkGray)),
                Span::styled(sick.to_string(), Style::default().fg(Color::Red)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Missing Title:     ", Style::default().fg(Color::DarkGray)),
                Span::styled(r.missing_title.to_string(), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled("  Missing Artist:    ", Style::default().fg(Color::DarkGray)),
                Span::styled(r.missing_artist.to_string(), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled("  Missing Album:     ", Style::default().fg(Color::DarkGray)),
                Span::styled(r.missing_album.to_string(), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled("  Missing Year:      ", Style::default().fg(Color::DarkGray)),
                Span::styled(r.missing_year.to_string(), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled("  Missing Track#:    ", Style::default().fg(Color::DarkGray)),
                Span::styled(r.missing_track.to_string(), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Have matches:      ", Style::default().fg(Color::DarkGray)),
                Span::styled(r.has_matches.to_string(), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::styled("  No match found:    ", Style::default().fg(Color::DarkGray)),
                Span::styled(r.no_match.to_string(), Style::default().fg(Color::Red)),
            ]),
            Line::from(""),
            Line::from(Span::styled("  [Enter/f] Browse files  [s] Re-scan  [Esc] Menu", Style::default().fg(Color::DarkGray))),
        ];
        f.render_widget(Paragraph::new(lines).block(block), area);
    } else {
        f.render_widget(Paragraph::new(vec![Line::from("  No report available.")]).block(block), area);
    }
}

fn render_healer_filelist(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .title(" Files With Issues ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));

    let sick_files = &app.healer.files;

    let items: Vec<ListItem> = sick_files.iter().enumerate().map(|(i, hf)| {
        let status_char = match hf.status {
            HealStatus::Pending => "?",
            HealStatus::Skipped => "-",
            HealStatus::NoMatch => "!",
        };
        let fname = hf.path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let issues_str: Vec<&str> = hf.issues.iter().map(|i| i.label()).collect();
        let match_hint = if !hf.matches.is_empty() {
            format!(" [{}%]", hf.matches[0].confidence)
        } else {
            String::new()
        };
        let label = format!(" [{}] {}{} | {}", status_char, fname, match_hint, issues_str.join(", "));
        let style = if i == app.healer.list_idx {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            match hf.status {
                HealStatus::Skipped => Style::default().fg(Color::DarkGray),
                HealStatus::NoMatch => Style::default().fg(Color::Red),
                HealStatus::Pending => Style::default().fg(Color::White),
            }
        };
        ListItem::new(Line::from(Span::styled(label, style)))
    }).collect();

    let list = List::new(items).block(block);
    f.render_stateful_widget(list, area, &mut app.healer.list_state);
}

fn render_healer_preview(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Preview Match ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));

    let hf = match app.healer.current_file() {
        Some(f) => f,
        None => {
            f.render_widget(Paragraph::new("  No file selected.").block(block), area);
            return;
        }
    };

    let match_count = hf.matches.len();
    let current_match = hf.matches.get(app.healer.match_idx);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("  File: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                hf.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Issues: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                hf.issues.iter().map(|i| i.label()).collect::<Vec<_>>().join(", "),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(""),
    ];

    if let Some(m) = current_match {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  Match {}/{} | ", app.healer.match_idx + 1, match_count),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(m.source.label(), Style::default().fg(Color::Cyan)),
            Span::styled(format!(" ({}%)", m.confidence), Style::default().fg(Color::Green)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Note: ", Style::default().fg(Color::DarkGray)),
            Span::styled(m.note.clone(), Style::default().fg(Color::Gray)),
        ]));
        lines.push(Line::from(""));

        let orig_track = hf.original.track.map(|n| n.to_string());
        let new_track  = m.tags.track.map(|n| n.to_string());
        let orig_year  = hf.original.year.map(|n| n.to_string());
        let new_year   = m.tags.year.map(|n| n.to_string());

        lines.push(make_healer_diff_line("Title",        hf.original.title.as_ref(),        m.tags.title.as_ref()));
        lines.push(make_healer_diff_line("Artist",       hf.original.artist.as_ref(),       m.tags.artist.as_ref()));
        lines.push(make_healer_diff_line("Album",        hf.original.album.as_ref(),        m.tags.album.as_ref()));
        lines.push(make_healer_diff_line("Album Artist", hf.original.album_artist.as_ref(), m.tags.album_artist.as_ref()));
        lines.push(make_healer_diff_line("Track",        orig_track.as_ref(),               new_track.as_ref()));
        lines.push(make_healer_diff_line("Year",         orig_year.as_ref(),                new_year.as_ref()));
        lines.push(make_healer_diff_line("Genre",        hf.original.genre.as_ref(),        m.tags.genre.as_ref()));

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  [Enter/a] Apply  [e] Edit  [h/m] Prev/Next Match  [L] Lookup Online  [s] Skip  [Esc] Back",
            Style::default().fg(Color::DarkGray),
        )));
    } else if match_count == 0 {
        lines.push(Line::from(Span::styled(
            "  No matches found. Press [L] to search online.",
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(Span::styled(
            "  [e] Manually edit tags  [s] Skip  [Esc] Back",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let lookup_status = match app.healer.lookup_state {
        HealLookupState::Idle      => "",
        HealLookupState::Searching => "  Searching online...",
        HealLookupState::Done      => "  Online search complete.",
        HealLookupState::Failed    => "  Search failed.",
    };
    if !lookup_status.is_empty() {
        lines.push(Line::from(Span::styled(lookup_status, Style::default().fg(Color::Yellow))));
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn make_healer_diff_line(label: &str, orig: Option<&String>, new: Option<&String>) -> Line<'static> {
    let orig_s   = orig.map(|s| s.as_str()).unwrap_or("—").to_string();
    let new_s    = new.map(|s| s.as_str()).unwrap_or("—").to_string();
    let changed  = orig.as_deref() != new.as_deref();
    Line::from(vec![
        Span::styled(format!("  {:<14} ", label), Style::default().fg(Color::DarkGray)),
        Span::styled(orig_s, Style::default().fg(Color::White)),
        Span::styled(" -> ".to_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(new_s, Style::default().fg(if changed { Color::Green } else { Color::White })),
    ])
}

fn render_healer_editor(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Tag Editor ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![
        Line::from(Span::styled(
            "  Edit tags manually. [Enter] to type, [Tab/j/k] navigate, [w] save.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
    ];

    for (i, name) in EDIT_FIELD_NAMES.iter().enumerate() {
        let is_active = i == app.healer.edit_field_idx;
        let is_typing = is_active && app.healer.edit_typing;
        let prefix = if is_active { "> " } else { "  " };
        let value  = &app.healer.edit_fields[i];

        let field_style = if is_active {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(Color::White)
        };
        let label_style = if is_active {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{}{:<14} ", prefix, name), label_style),
            Span::styled(
                if is_typing { format!("{}_", value) } else { value.clone() },
                field_style,
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  [Tab/j/k] Navigate  [Enter] Edit  [w] Save & Apply  [Esc] Cancel",
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(Paragraph::new(lines), inner);
}
