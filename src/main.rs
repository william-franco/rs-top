use std::cmp::Ordering;
use std::io::{self, stdout};
use std::time::{Duration, Instant};

// Crossterm for terminal input/output
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

// Ratatui for UI rendering
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table, Wrap};

// System info library
use sysinfo::{CpuExt, PidExt, ProcessExt, System, SystemExt};

// Refresh interval and thresholds
const REFRESH_INTERVAL: Duration = Duration::from_millis(1000); // 1s
const HIGH_CPU_THRESHOLD: f32 = 50.0;
const HIGH_MEM_MB_THRESHOLD: u64 = 200;

// Sorting options for processes
#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    CpuDesc,
    MemDesc,
    PidAsc,
}

// Application state
struct App {
    sys: System,                    // System information
    processes: Vec<ProcessInfo>,    // List of processes
    selected: usize,                // Selected row
    offset: usize,                  // Scroll offset
    sort_by: SortBy,                // Current sorting
    filter: Option<String>,         // Optional filter
    filter_mode: bool,              // Filter input active
    filter_input: String,           // Current filter text
    last_refresh: Instant,          // Last refresh time
    status_message: Option<String>, // Status messages
}

// Process info structure
#[derive(Clone)]
struct ProcessInfo {
    pid: i32,
    name: String,
    cpu: f32,
    mem_mb: u64,
    cmd: String,
}

impl App {
    // Initialize App and refresh system info
    fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();

        Self {
            sys,
            processes: Vec::new(),
            selected: 0,
            offset: 0,
            sort_by: SortBy::CpuDesc,
            filter: None,
            filter_mode: false,
            filter_input: String::new(),
            last_refresh: Instant::now(),
            status_message: None,
        }
    }

    // Refresh process list and apply filter/sorting
    fn refresh(&mut self) {
        self.sys.refresh_all();

        let mut procs: Vec<ProcessInfo> = self
            .sys
            .processes()
            .iter()
            .map(|(pid_ref, proc_)| {
                let mem_kb = proc_.memory();
                let mem_mb = mem_kb / 1024;
                ProcessInfo {
                    pid: pid_ref.as_u32() as i32,
                    name: proc_.name().to_string(),
                    cpu: proc_.cpu_usage(),
                    mem_mb,
                    cmd: proc_.cmd().join(" "),
                }
            })
            .collect();

        // Apply filter if active
        if let Some(f) = &self.filter {
            let f_lower = f.to_lowercase();
            procs.retain(|p| {
                p.name.to_lowercase().contains(&f_lower) || p.cmd.to_lowercase().contains(&f_lower)
            });
        }

        // Sort processes
        match self.sort_by {
            SortBy::CpuDesc => {
                procs.sort_unstable_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(Ordering::Equal))
            }
            SortBy::MemDesc => procs.sort_unstable_by(|a, b| b.mem_mb.cmp(&a.mem_mb)),
            SortBy::PidAsc => procs.sort_unstable_by(|a, b| a.pid.cmp(&b.pid)),
        }

        self.processes = procs;

        // Adjust selected index if out of bounds
        if self.selected >= self.processes.len() && !self.processes.is_empty() {
            self.selected = self.processes.len() - 1;
        } else if self.processes.is_empty() {
            self.selected = 0;
        }

        self.last_refresh = Instant::now();
        self.status_message = None;
    }

    // Navigation functions
    fn previous(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.offset {
                self.offset = self.selected;
            }
        }
    }

    fn next(&mut self, height: usize) {
        if self.selected + 1 < self.processes.len() {
            self.selected += 1;
            let visible = height.saturating_sub(5);
            if self.selected >= self.offset + visible {
                self.offset = self.selected.saturating_sub(visible - 1);
            }
        }
    }

    fn page_up(&mut self, height: usize) {
        let visible = height.saturating_sub(5);
        if self.selected > visible {
            self.selected -= visible;
        } else {
            self.selected = 0;
        }
        if self.selected < self.offset {
            self.offset = self.selected;
        }
    }

    fn page_down(&mut self, height: usize) {
        let visible = height.saturating_sub(5);
        self.selected = (self.selected + visible).min(self.processes.len().saturating_sub(1));
        let max_offset = self.processes.len().saturating_sub(visible);
        self.offset = self.offset.min(max_offset);
        if self.selected >= self.offset + visible {
            self.offset = self.selected.saturating_sub(visible - 1);
        }
    }

    // Placeholder for killing process
    fn kill_selected(&mut self) -> Result<(), String> {
        let msg = "Kill process disabled in this version.";
        self.status_message = Some(msg.to_string());
        Err(msg.to_string())
    }
}

// Helper: Determine CPU usage color
fn get_cpu_color(pct: f32) -> Color {
    if pct > 80.0 {
        Color::Red
    } else if pct > 50.0 {
        Color::Yellow
    } else if pct > 20.0 {
        Color::Cyan
    } else {
        Color::Green
    }
}

// Draw a single CPU usage gauge
fn draw_cpu_gauge(pct: f32, title: String) -> Gauge<'static> {
    let color = get_cpu_color(pct);
    let usage = pct.clamp(0.0, 100.0);

    let block = Block::default()
        .title(Line::from(title))
        .borders(Borders::ALL);

    Gauge::default()
        .block(block)
        .gauge_style(Style::default().fg(color).bg(Color::Black))
        .label(Span::from(format!("{:.1}%", usage)))
        .percent(usage as u16)
}

// Main UI rendering
fn ui<B: ratatui::backend::Backend>(f: &mut ratatui::Frame<B>, app: &mut App) {
    let size = f.size();

    let cpus = app.sys.cpus();
    let cpu_count = cpus.len().max(1);

    // Determine grid layout for CPU gauges
    let columns = if cpu_count > 8 {
        4
    } else if cpu_count > 4 {
        2
    } else {
        1
    };
    let rows = ((cpu_count as f32) / (columns as f32)).ceil() as usize;
    let cpu_grid_height = (rows * 3) as u16;

    // Split terminal into sections
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(cpu_grid_height), // CPU gauges
            Constraint::Length(4),               // Memory/Swap
            Constraint::Min(10),                 // Process table
            Constraint::Length(1),               // Status/filter
        ])
        .split(size);

    draw_cpu_grid(f, &chunks[0], &app.sys);
    draw_mem_swap(f, &chunks[1], &app.sys);
    draw_process_table(f, &chunks[2], app);
    draw_status_and_filter(f, &chunks[3], app);
}

// Draw CPU grid
fn draw_cpu_grid<B: ratatui::backend::Backend>(
    f: &mut ratatui::Frame<B>,
    area: &Rect,
    sys: &System,
) {
    let cpus = sys.cpus();
    let cpu_count = cpus.len().max(1);

    let columns = if cpu_count > 8 {
        4
    } else if cpu_count > 4 {
        2
    } else {
        1
    };
    let rows = ((cpu_count as f32) / (columns as f32)).ceil() as usize;

    let grid_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints((0..rows).map(|_| Constraint::Length(3)).collect::<Vec<_>>())
        .split(*area);

    let mut cpu_index = 0;

    for r in 0..rows {
        let remaining = cpu_count.saturating_sub(r * columns);
        let row_len = remaining.min(columns);
        if row_len == 0 {
            break;
        }

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(
                (0..row_len)
                    .map(|_| Constraint::Percentage(100 / row_len as u16))
                    .collect::<Vec<_>>(),
            )
            .split(grid_rows[r]);

        for rect in cols.as_ref() {
            if cpu_index < cpu_count {
                let usage = cpus[cpu_index].cpu_usage().clamp(0.0, 100.0);
                let title = format!("CPU{}", cpu_index);
                f.render_widget(draw_cpu_gauge(usage, title), *rect);
                cpu_index += 1;
            }
        }
    }
}

// Draw memory and swap gauges
fn draw_mem_swap<B: ratatui::backend::Backend>(
    f: &mut ratatui::Frame<B>,
    area: &Rect,
    sys: &System,
) {
    let mem_swap_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(*area);

    // Memory
    let total_mem = sys.total_memory().max(1);
    let used_mem = sys.used_memory();
    let mem_percent = ((used_mem as f64 / total_mem as f64) * 100.0).clamp(0.0, 100.0) as u16;

    let mem_gauge = Gauge::default()
        .block(
            Block::default()
                .title(Line::from("Memory"))
                .borders(Borders::ALL),
        )
        .gauge_style(Style::default().fg(Color::Green))
        .label(Span::from(format!(
            "{:.0}/{:.0} MB",
            used_mem as f64 / 1024.0,
            total_mem as f64 / 1024.0
        )))
        .percent(mem_percent);

    f.render_widget(mem_gauge, mem_swap_layout[0]);

    // Swap
    let total_swap = sys.total_swap();
    let used_swap = sys.used_swap();
    let swap_percent = if total_swap > 0 {
        ((used_swap as f64 / total_swap as f64) * 100.0).clamp(0.0, 100.0) as u16
    } else {
        0
    };

    let swap_gauge = Gauge::default()
        .block(
            Block::default()
                .title(Line::from("Swap"))
                .borders(Borders::ALL),
        )
        .gauge_style(Style::default().fg(Color::Magenta))
        .label(Span::from(format!(
            "{:.0}/{:.0} MB",
            used_swap as f64 / 1024.0,
            total_swap as f64 / 1024.0
        )))
        .percent(swap_percent);

    f.render_widget(swap_gauge, mem_swap_layout[1]);
}

// Draw process table
fn draw_process_table<B: ratatui::backend::Backend>(
    f: &mut ratatui::Frame<B>,
    area: &Rect,
    app: &App,
) {
    // Table header
    let title_span = Span::styled(
        "Processes (c=CPU, m=Mem, p=PID, q=quit)",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let header_cells = ["PID", "USER", "%CPU", "MEM(MB)", "Command"]
        .iter()
        .map(|h| Cell::from(*h));
    let header = Row::new(header_cells).style(
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    );

    let max_rows = (area.height as usize).saturating_sub(2);
    let processes = &app.processes;
    let end = (app.offset + max_rows).min(processes.len());
    let slice = &processes.get(app.offset..end).unwrap_or(&[]);

    // Rows
    let rows: Vec<Row> = slice
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let is_selected = (app.offset + i) == app.selected;
            let mut style = Style::default().fg(if p.cpu >= HIGH_CPU_THRESHOLD {
                Color::Red
            } else if p.mem_mb >= HIGH_MEM_MB_THRESHOLD {
                Color::Yellow
            } else {
                Color::White
            });
            if is_selected {
                style = style
                    .bg(Color::Rgb(50, 50, 50))
                    .add_modifier(Modifier::BOLD);
            }

            let cells = vec![
                Cell::from(format!("{}", p.pid)),
                Cell::from("user"),
                Cell::from(format!("{:>5.1}", p.cpu)),
                Cell::from(format!("{}", p.mem_mb)),
                Cell::from(p.name.clone()),
            ];
            Row::new(cells).style(style)
        })
        .collect();

    let table = Table::new(rows)
        .header(header)
        .block(Block::default().title(title_span).borders(Borders::ALL))
        .widths(&[
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Percentage(50),
        ])
        .column_spacing(1);

    f.render_widget(table, *area);
}

// Draw status line and filter input
fn draw_status_and_filter<B: ratatui::backend::Backend>(
    f: &mut ratatui::Frame<B>,
    area: &Rect,
    app: &mut App,
) {
    let mut spans: Vec<Span> = Vec::new();

    if let Some(msg) = &app.status_message {
        spans.push(Span::styled(
            format!("STATUS: {}", msg),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    } else if app.filter_mode {
        spans.push(Span::styled(
            "Filter: ",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(&app.filter_input));
        spans.push(Span::raw("_ (Enter=apply, Esc=cancel)"));
    } else {
        spans.push(Span::styled(
            "↑/↓/PgUp/PgDown: Navigate | c/m/p: Sort | /: Filter | q: Quit | k: Kill DISABLED",
            Style::default().fg(Color::DarkGray),
        ));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true }),
        *area,
    );
}

// Main function
fn main() -> Result<(), io::Error> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = App::new();
    app.refresh();

    let mut last_tick = Instant::now();
    loop {
        let frame_size = terminal.get_frame().size();

        terminal.draw(|f| ui(f, &mut app))?;

        let timeout = REFRESH_INTERVAL
            .checked_sub(last_tick.elapsed())
            .unwrap_or(Duration::from_millis(0));

        if event::poll(timeout)? {
            if let CEvent::Key(key_event) = event::read()? {
                handle_key_event(key_event, &mut app, frame_size.height as usize);
            }
        }

        if last_tick.elapsed() >= REFRESH_INTERVAL {
            app.refresh();
            last_tick = Instant::now();
        }
    }
}

// Handle keyboard input
fn handle_key_event(key: KeyEvent, app: &mut App, height: usize) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => cleanup_and_exit(),
        KeyCode::Up => app.previous(),
        KeyCode::Down => app.next(height),
        KeyCode::PageUp => app.page_up(height),
        KeyCode::PageDown => app.page_down(height),
        KeyCode::Char('c') => {
            app.sort_by = SortBy::CpuDesc;
            app.refresh();
        }
        KeyCode::Char('m') => {
            app.sort_by = SortBy::MemDesc;
            app.refresh();
        }
        KeyCode::Char('p') => {
            app.sort_by = SortBy::PidAsc;
            app.refresh();
        }
        KeyCode::Char('k') => {
            let _ = app.kill_selected();
        }
        KeyCode::Char('/') => {
            app.filter_mode = true;
            app.filter_input.clear();
        }
        KeyCode::Esc => {
            if app.filter_mode {
                app.filter_mode = false;
                app.filter_input.clear();
            }
        }
        KeyCode::Enter => {
            if app.filter_mode {
                app.filter = if app.filter_input.is_empty() {
                    None
                } else {
                    Some(app.filter_input.clone())
                };
                app.filter_mode = false;
                app.selected = 0;
                app.offset = 0;
                app.refresh();
            }
        }
        KeyCode::Char(c) => {
            if app.filter_mode {
                app.filter_input.push(c);
            }
        }
        KeyCode::Backspace => {
            if app.filter_mode {
                app.filter_input.pop();
            }
        }
        _ => {}
    }
}

// Cleanup terminal before exit
fn cleanup_and_exit() {
    let _ = disable_raw_mode();
    let mut stdout = stdout();
    let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture);
    std::process::exit(0);
}
