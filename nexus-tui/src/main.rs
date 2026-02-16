use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use nexus_core::models::Story;
use nexus_core::{resolver, sites::get_site};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;

fn main() -> io::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to create tokio runtime");

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app_result = run_app(&mut terminal, &rt);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    app_result
}

#[derive(Default)]
struct App {
    input_url: String,
    status: String,
    story: Option<Story>,
    selected_chapter: usize,
    chapter_text: String,
    chapter_scroll: u16,
}

fn run_app(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    rt: &tokio::runtime::Runtime,
) -> io::Result<()> {
    let mut app = App {
        status: "Type URL and press Enter to scrape. q to quit. d to download. l to load chapter text.".into(),
        ..Default::default()
    };

    loop {
        terminal.draw(|frame| draw_ui(frame, &app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('j') => {
                        if let Some(story) = &app.story {
                            if app.selected_chapter + 1 < story.chapters.len() {
                                app.selected_chapter += 1;
                                app.chapter_scroll = 0;
                            }
                        }
                    }
                    KeyCode::Char('k') => {
                        if app.selected_chapter > 0 {
                            app.selected_chapter -= 1;
                            app.chapter_scroll = 0;
                        }
                    }
                    KeyCode::Char('d') => {
                        match download_story(&app.story) {
                            Ok(path) => app.status = format!("Downloaded to {}", path),
                            Err(e) => app.status = format!("Download failed: {}", e),
                        }
                    }
                    KeyCode::Char('l') => {
                        match rt.block_on(load_selected_chapter_text(&app)) {
                            Ok(text) => {
                                app.chapter_text = text;
                                app.chapter_scroll = 0;
                                app.status = "Loaded chapter text".into();
                            }
                            Err(e) => app.status = format!("Load chapter failed: {}", e),
                        }
                    }
                    KeyCode::Down => app.chapter_scroll = app.chapter_scroll.saturating_add(2),
                    KeyCode::Up => app.chapter_scroll = app.chapter_scroll.saturating_sub(2),
                    KeyCode::Enter => {
                        let url = app.input_url.trim().to_string();
                        if url.is_empty() {
                            app.status = "Please enter a URL".into();
                            continue;
                        }
                        app.status = "Scraping online...".into();

                        match rt.block_on(fetch_story(&url)) {
                            Ok(story) => {
                                app.selected_chapter = 0;
                                app.chapter_scroll = 0;
                                app.chapter_text.clear();
                                app.status = format!(
                                    "Loaded '{}' with {} chapters",
                                    story.story_name.clone().unwrap_or_else(|| "Unknown title".into()),
                                    story.chapters.len()
                                );
                                app.story = Some(story);
                            }
                            Err(e) => app.status = format!("Fetch failed: {}", e),
                        }
                    }
                    KeyCode::Backspace => {
                        app.input_url.pop();
                    }
                    KeyCode::Char(c) => {
                        app.input_url.push(c);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn draw_ui(frame: &mut ratatui::Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let input = Paragraph::new(app.input_url.as_str())
        .block(Block::default().title("Story URL (type + Enter)").borders(Borders::ALL));
    frame.render_widget(input, chunks[0]);

    let url_preview = Paragraph::new(app.input_url.as_str())
        .block(Block::default().title("URL Preview (full, wrapped)").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(url_preview, chunks[1]);

    let status = Paragraph::new(app.status.as_str()).block(
        Block::default()
            .title("Status")
            .borders(Borders::ALL),
    );
    frame.render_widget(status, chunks[2]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(chunks[3]);

    let items: Vec<ListItem> = app
        .story
        .as_ref()
        .map(|s| {
            s.chapters
                .iter()
                .enumerate()
                .map(|(i, ch)| {
                    let title = ch.title.clone().unwrap_or_else(|| "Untitled".into());
                    let number = ch.chapter_number.unwrap_or((i + 1) as u32);
                    ListItem::new(format!("{:>3}. {}", number, title))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(app.selected_chapter));
    }

    let chapters = List::new(items)
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol("> ")
        .block(Block::default().title("Chapters (j/k)").borders(Borders::ALL));
    frame.render_stateful_widget(chapters, body[0], &mut state);

    let mut lines = Vec::new();
    if let Some(story) = &app.story {
        lines.push(Line::from(format!(
            "Title: {}",
            story.story_name.clone().unwrap_or_else(|| "Unknown".into())
        )));
        lines.push(Line::from(format!(
            "Author: {}",
            story.author_name.clone().unwrap_or_else(|| "Unknown".into())
        )));
        lines.push(Line::from(format!("Site: {}", story.site)));
        lines.push(Line::from(""));

        if !app.chapter_text.is_empty() {
            lines.push(Line::from("Chapter text (loaded):"));
            lines.push(Line::from(""));
            lines.extend(Text::from(app.chapter_text.as_str()).lines);
        } else {
            lines.push(Line::from("Press 'l' to load selected chapter text online."));
            lines.push(Line::from("Press 'd' to download story.json."));
        }
    } else {
        lines.push(Line::from("No story loaded yet."));
    }

    let reader = Paragraph::new(lines)
        .block(Block::default().title("Reader").borders(Borders::ALL))
        .wrap(Wrap { trim: false })
        .scroll((app.chapter_scroll, 0));
    frame.render_widget(reader, body[1]);

    let help = Paragraph::new("Enter=fetch URL | j/k=select chapter | l=load chapter | d=download | Up/Down=scroll | q=quit");
    frame.render_widget(help, chunks[4]);
}

async fn fetch_story(url: &str) -> Result<Story, String> {
    let client = reqwest::Client::new();
    resolver::fetch_story_data_from_url(url, None, &client)
        .await
        .map_err(|e| e.to_string())
}

async fn load_selected_chapter_text(app: &App) -> Result<String, String> {
    let story = app
        .story
        .as_ref()
        .ok_or_else(|| "No story loaded".to_string())?;

    let chapter = story
        .chapters
        .get(app.selected_chapter)
        .ok_or_else(|| "No chapter selected".to_string())?;

    if let Some(text) = &chapter.text {
        if !text.trim().is_empty() {
            return Ok(text.clone());
        }
    }

    // If content is missing, fetch from network for the selected chapter.
    let story_id = story
        .story_id
        .ok_or_else(|| "Story id missing in current story".to_string())?;
    let chapter_id = chapter.chapter_id.unwrap_or(0);
    let chapter_number = chapter.chapter_number.unwrap_or((app.selected_chapter + 1) as u32);

    let site = get_site(&story.site).map_err(|e| e.to_string())?;
    let client = reqwest::Client::new();
    let loaded = site
        .fetch_chapter(story_id, chapter_id, chapter_number, &client)
        .await
        .map_err(|e| e.to_string())?;

    Ok(loaded
        .text
        .unwrap_or_else(|| "(chapter has no text payload)".into()))
}

fn download_story(story: &Option<Story>) -> Result<String, String> {
    let story = story
        .as_ref()
        .ok_or_else(|| "No story loaded to download".to_string())?;

    let title = story
        .story_name
        .clone()
        .unwrap_or_else(|| "story".into())
        .replace(['/', ' ', '\\'], "_");
    let file = format!("{}_story.json", title);
    let json = serde_json::to_string_pretty(story).map_err(|e| e.to_string())?;
    std::fs::write(&file, json).map_err(|e| e.to_string())?;

    Ok(file)
}
