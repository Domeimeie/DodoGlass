//! The full-screen terminal interface.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Gauge, Paragraph, Row, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Sparkline, Table, TableState,
};
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::mpsc::UnboundedReceiver as Receiver;

use crate::config::Config;
use crate::ffmpeg::{Job, Video};
use crate::headless::clock;
use crate::runner::{Event, Outcome};

const ACCENT: Color = Color::Cyan;
const SECOND: Color = Color::Magenta;
const MUTED: Color = Color::DarkGray;
const TRACK: Color = Color::Rgb(45, 45, 55);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_LOG_LINES: usize = 2000;
/// Width of the small progress bars in the video table.
const BAR_WIDTH: usize = 8;

enum JobState {
    Queued,
    Running,
    Finished(Outcome),
}

struct JobView {
    job: Job,
    state: JobState,
    seconds: f64,
    speed: Option<f64>,
    started: Option<Instant>,
    took: Option<Duration>,
}

pub struct App {
    input: PathBuf,
    threads: usize,
    cfg: Config,
    videos: Vec<Video>,
    jobs: Vec<JobView>,
    /// For each video: its HLS and DASH job, if requested.
    video_jobs: Vec<[Option<usize>; 2]>,
    /// Log lines with the job they came from (None: from the program itself).
    logs: VecDeque<(Option<usize>, String)>,
    /// Combined encoding speed of all running jobs (x100), sampled over time.
    speeds: Vec<u64>,
    started: Instant,
    finished: Option<Duration>,
    tick: usize,
    all_done: bool,
    cancel: Arc<AtomicBool>,
    table: TableState,
}

#[derive(Default)]
struct Counts {
    running: usize,
    queued: usize,
    done: usize,
    failed: usize,
    cancelled: usize,
}

impl App {
    pub fn new(
        input: PathBuf,
        threads: usize,
        cfg: Config,
        videos: Vec<Video>,
        jobs: Vec<Job>,
        warnings: Vec<String>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let mut video_jobs = vec![[None, None]; videos.len()];
        for (i, job) in jobs.iter().enumerate() {
            video_jobs[job.video][job.format.slot()] = Some(i);
        }
        let mut logs: VecDeque<_> = warnings
            .into_iter()
            .map(|w| (None, format!("⚠ {w}")))
            .collect();
        for v in &videos {
            if let Err(e) = &v.source {
                logs.push_back((None, format!("✘ {}: {e}", v.name)));
            }
        }
        App {
            input,
            threads,
            cfg,
            videos,
            jobs: jobs
                .into_iter()
                .map(|job| JobView {
                    job,
                    state: JobState::Queued,
                    seconds: 0.0,
                    speed: None,
                    started: None,
                    took: None,
                })
                .collect(),
            video_jobs,
            logs,
            speeds: Vec::new(),
            started: Instant::now(),
            finished: None,
            tick: 0,
            all_done: false,
            cancel,
            table: TableState::default(),
        }
    }

    fn cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn counts(&self) -> Counts {
        let mut c = Counts {
            failed: self.videos.iter().filter(|v| v.source.is_err()).count(),
            ..Counts::default()
        };
        for j in &self.jobs {
            match &j.state {
                JobState::Queued => c.queued += 1,
                JobState::Running => c.running += 1,
                JobState::Finished(Outcome::Done) => c.done += 1,
                JobState::Finished(Outcome::Failed(_)) => c.failed += 1,
                JobState::Finished(Outcome::Cancelled) => c.cancelled += 1,
            }
        }
        c
    }

    fn succeeded(&self) -> bool {
        let c = self.counts();
        c.failed == 0 && c.cancelled == 0 && c.done == self.jobs.len()
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::JobStarted(i) => {
                self.jobs[i].state = JobState::Running;
                self.jobs[i].started = Some(Instant::now());
            }
            Event::Progress {
                job,
                seconds,
                speed,
                ..
            } => {
                self.jobs[job].seconds = seconds;
                self.jobs[job].speed = speed;
            }
            Event::Log { job, line } => {
                self.logs.push_back((Some(job), line));
                if self.logs.len() > MAX_LOG_LINES {
                    self.logs.pop_front();
                }
            }
            Event::JobFinished { job, outcome } => {
                let j = &mut self.jobs[job];
                j.took = j.started.map(|s| s.elapsed());
                j.state = JobState::Finished(outcome);
            }
            Event::AllDone => {
                self.all_done = true;
                self.finished = Some(self.started.elapsed());
            }
        }
    }

    /// Records the combined speed of everything running, for the speed graph.
    fn sample_speed(&mut self) {
        let running: Vec<f64> = self
            .jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::Running))
            .filter_map(|j| j.speed)
            .collect();
        if !running.is_empty() {
            self.speeds
                .push((running.iter().sum::<f64>() * 100.0) as u64);
        }
    }

    /// Fraction of one job that is done; finished jobs (in any way) count as complete.
    fn job_progress(&self, j: &JobView) -> f64 {
        match j.state {
            JobState::Finished(_) => 1.0,
            _ => {
                let duration = self.videos[j.job.video].duration();
                if duration > 0.0 {
                    (j.seconds / duration).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            }
        }
    }

    fn job_label(&self, i: usize) -> String {
        let j = &self.jobs[i].job;
        format!("[{} {}]", self.videos[j.video].name, j.format.label())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let ladder_height = self.cfg.renditions.len() as u16 + 4;
        let [header, overall, middle, log, footer] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(ladder_height + 4),
            Constraint::Length(9),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [videos, side] =
            Layout::horizontal([Constraint::Min(60), Constraint::Length(44)]).areas(middle);
        let [ladder, speed] =
            Layout::vertical([Constraint::Length(ladder_height), Constraint::Min(4)]).areas(side);

        self.draw_header(frame, header);
        self.draw_overall(frame, overall);
        self.draw_videos(frame, videos);
        self.draw_ladder(frame, ladder);
        self.draw_speed(frame, speed);
        self.draw_log(frame, log);
        self.draw_footer(frame, footer);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let kind = if self.input.is_dir() {
            "folder"
        } else {
            "file"
        };
        let formats: Vec<&str> = ["HLS", "DASH"]
            .into_iter()
            .enumerate()
            .filter(|(slot, _)| self.video_jobs.iter().any(|v| v[*slot].is_some()))
            .map(|(_, f)| f)
            .collect();
        let text = vec![
            Line::from(vec![
                "Input  ".fg(MUTED),
                Span::styled(self.input.display().to_string(), Style::new().bold()),
                format!("  ({kind})").fg(MUTED),
            ]),
            Line::from(vec![
                "Batch  ".fg(MUTED),
                format!("{} video(s)", self.videos.len()).fg(ACCENT),
                " · ".fg(MUTED),
                format!("{} job(s)", self.jobs.len()).into(),
                " · ".fg(MUTED),
                formats.join(" + ").into(),
                " · ".fg(MUTED),
                format!("{} in parallel", self.threads).fg(SECOND),
            ]),
        ];
        let block = panel(" ◆ DodoGlass Transcoder ")
            .title_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD));
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_overall(&self, frame: &mut Frame, area: Rect) {
        let ratio = if self.jobs.is_empty() {
            1.0
        } else {
            self.jobs.iter().map(|j| self.job_progress(j)).sum::<f64>() / self.jobs.len() as f64
        };
        let c = self.counts();
        let finished = c.done + c.failed + c.cancelled;
        let gauge = Gauge::default()
            .block(panel(" Overall "))
            .ratio(ratio)
            .label(format!(
                "{:.0}%  ·  {finished}/{} jobs finished",
                ratio * 100.0,
                self.jobs.len()
            ))
            .use_unicode(true)
            .gauge_style(Style::new().fg(ACCENT).bg(TRACK));
        frame.render_widget(gauge, area);
    }

    fn draw_videos(&mut self, frame: &mut Frame, area: Rect) {
        let header = Row::new(["", "Video", "Source", "HLS", "DASH"])
            .style(Style::new().fg(SECOND).add_modifier(Modifier::BOLD));
        let rows: Vec<Row> = (0..self.videos.len()).map(|v| self.video_row(v)).collect();
        let row_count = rows.len();
        let table = Table::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Fill(1),
                Constraint::Length(17),
                Constraint::Length(18),
                Constraint::Length(18),
            ],
        )
        .header(header)
        .row_highlight_style(Style::new().bg(TRACK))
        .block(panel(" Videos "));
        frame.render_stateful_widget(table, area, &mut self.table);

        // Header row and borders take 3 lines; show a scrollbar if the rest doesn't fit.
        if row_count > area.height.saturating_sub(3) as usize {
            let mut state = ScrollbarState::new(row_count).position(self.table.offset());
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .thumb_style(Style::new().fg(ACCENT))
                    .track_style(Style::new().fg(MUTED)),
                area.inner(Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                &mut state,
            );
        }
    }

    fn video_row(&self, v: usize) -> Row<'static> {
        let video = &self.videos[v];
        let jobs: Vec<&JobView> = self.video_jobs[v]
            .iter()
            .flatten()
            .map(|&i| &self.jobs[i])
            .collect();
        let (icon, color) = if video.source.is_err()
            || jobs
                .iter()
                .any(|j| matches!(j.state, JobState::Finished(Outcome::Failed(_))))
        {
            ("✘".to_string(), Color::Red)
        } else if jobs.iter().any(|j| matches!(j.state, JobState::Running)) {
            (SPINNER[self.tick % SPINNER.len()].to_string(), ACCENT)
        } else if jobs
            .iter()
            .all(|j| matches!(j.state, JobState::Finished(Outcome::Done)))
        {
            ("✔".to_string(), Color::Green)
        } else if jobs
            .iter()
            .any(|j| matches!(j.state, JobState::Finished(Outcome::Cancelled)))
        {
            ("⊘".to_string(), Color::Yellow)
        } else {
            ("○".to_string(), MUTED)
        };

        let source = match &video.source {
            Ok(s) => Line::from(format!("{}×{} · {}", s.width, s.height, clock(s.duration))),
            Err(_) => Line::from("unreadable".fg(Color::Red)),
        };
        Row::new([
            Cell::from(Span::styled(icon, Style::new().fg(color))),
            Cell::from(video.name.clone()),
            Cell::from(source),
            Cell::from(self.job_cell(self.video_jobs[v][0])),
            Cell::from(self.job_cell(self.video_jobs[v][1])),
        ])
    }

    /// One format's status in the video table: a small bar while running.
    fn job_cell(&self, job: Option<usize>) -> Line<'static> {
        let Some(i) = job else {
            return Line::from("–".fg(MUTED));
        };
        let j = &self.jobs[i];
        match &j.state {
            JobState::Queued => Line::from("queued".fg(MUTED)),
            JobState::Running => {
                let pct = self.job_progress(j);
                let filled = (pct * BAR_WIDTH as f64).round() as usize;
                let mut spans = vec![
                    "━".repeat(filled).fg(ACCENT),
                    "━".repeat(BAR_WIDTH - filled).fg(TRACK),
                    format!(" {:>3.0}%", pct * 100.0).into(),
                ];
                if let Some(s) = j.speed {
                    spans.push(format!(" {s:.1}x").fg(MUTED));
                }
                Line::from(spans)
            }
            JobState::Finished(Outcome::Done) => Line::from(
                format!("✔ {:.1}s", j.took.unwrap_or_default().as_secs_f64()).fg(Color::Green),
            ),
            JobState::Finished(Outcome::Failed(_)) => Line::from("✘ failed".fg(Color::Red)),
            JobState::Finished(Outcome::Cancelled) => Line::from("⊘ cancelled".fg(Color::Yellow)),
        }
    }

    fn draw_ladder(&self, frame: &mut Frame, area: Rect) {
        let header = Row::new(["Name", "Height", "Bitrate", "FPS"])
            .style(Style::new().fg(SECOND).add_modifier(Modifier::BOLD));
        let rows = self.cfg.renditions.iter().map(|r| {
            Row::new([
                r.name.clone(),
                format!("{}p", r.height),
                format!("{} kb/s", r.bitrate_kbps),
                self.cfg
                    .fps_for(r)
                    .map(|f| f.to_string())
                    .unwrap_or_else(|| "source".into()),
            ])
        });
        let summary = Line::from(vec![
            format!(" {}s segments", self.cfg.segment_duration).fg(ACCENT),
            " · ".fg(MUTED),
            format!("x264 {}", self.cfg.preset).into(),
            " · ".fg(MUTED),
            format!("audio {}k ", self.cfg.audio_bitrate_kbps).into(),
        ]);
        let table = Table::new(
            rows,
            [
                Constraint::Length(14),
                Constraint::Length(6),
                Constraint::Length(10),
                Constraint::Length(6),
            ],
        )
        .header(header)
        .block(panel(" Ladder ").title_bottom(summary));
        frame.render_widget(table, area);
    }

    fn draw_speed(&self, frame: &mut Frame, area: Rect) {
        let width = area.width.saturating_sub(2) as usize;
        let recent = &self.speeds[self.speeds.len().saturating_sub(width)..];
        let now = if self.counts().running > 0 {
            recent.last().copied().unwrap_or(0) as f64 / 100.0
        } else {
            0.0
        };
        let spark = Sparkline::default()
            .block(panel(&format!(" Total speed · {now:.1}x ")))
            .data(recent)
            .style(Style::new().fg(SECOND));
        frame.render_widget(spark, area);
    }

    fn draw_log(&self, frame: &mut Frame, area: Rect) {
        let visible = area.height.saturating_sub(2) as usize;
        let start = self.logs.len().saturating_sub(visible);
        let lines: Vec<Line> = self
            .logs
            .iter()
            .skip(start)
            .map(|(job, l)| {
                let style = if l.starts_with("$ ") {
                    Style::new().fg(Color::Yellow)
                } else if l.starts_with('⚠') {
                    Style::new().fg(Color::Yellow).bold()
                } else if l.starts_with('✘') || l.to_lowercase().contains("error") {
                    Style::new().fg(Color::Red)
                } else {
                    Style::new().fg(Color::Gray)
                };
                let mut spans = Vec::new();
                if let Some(i) = job {
                    spans.push(Span::styled(
                        format!("{} ", self.job_label(*i)),
                        Style::new().fg(SECOND),
                    ));
                }
                spans.push(Span::styled(l.as_str(), style));
                Line::from(spans)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).block(panel(" ffmpeg log ")), area);
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let elapsed = self.finished.unwrap_or_else(|| self.started.elapsed());
        let c = self.counts();
        let (status, color) = if !self.all_done {
            if self.cancelling() {
                ("cancelling…", Color::Yellow)
            } else {
                ("transcoding…", ACCENT)
            }
        } else if self.succeeded() {
            ("all done", Color::Green)
        } else if self.cancelling() {
            ("cancelled", Color::Yellow)
        } else {
            ("finished with errors", Color::Red)
        };
        let key_hint = if self.all_done { " exit " } else { " cancel " };
        let line = Line::from(vec![
            " q ".fg(Color::Black).bg(ACCENT).bold(),
            key_hint.fg(MUTED),
            " ↑↓ ".fg(Color::Black).bg(ACCENT).bold(),
            " scroll ".fg(MUTED),
            "  ⏱ ".fg(MUTED),
            clock(elapsed.as_secs_f64()).into(),
            "  ".into(),
            Span::styled(status, Style::new().fg(color).bold()),
            "   ".into(),
            format!("{} running", c.running).fg(ACCENT),
            " · ".fg(MUTED),
            format!("{} queued", c.queued).into(),
            " · ".fg(MUTED),
            format!("{} done", c.done).fg(Color::Green),
            " · ".fg(MUTED),
            format!("{} failed", c.failed).fg(if c.failed > 0 { Color::Red } else { MUTED }),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }
}

fn panel(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(MUTED))
        .title(Span::styled(
            title.to_string(),
            Style::new().fg(Color::White).bold(),
        ))
}

/// Runs the interface until the user quits. Returns true if every job succeeded.
pub fn run(mut app: App, mut rx: Receiver<Event>) -> Result<bool> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &mut rx);
    ratatui::restore();
    result.map(|()| app.succeeded())
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    rx: &mut Receiver<Event>,
) -> Result<()> {
    loop {
        while let Ok(event) = rx.try_recv() {
            app.handle(event);
        }
        // About twice a second, matching how often ffmpeg reports progress.
        if app.tick.is_multiple_of(5) {
            app.sample_speed();
        }
        terminal.draw(|frame| app.draw(frame))?;

        // After a cancel, leave as soon as every ffmpeg has stopped.
        if app.all_done && app.cancelling() {
            return Ok(());
        }
        if event::poll(Duration::from_millis(100))?
            && let TermEvent::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            let ctrl_c =
                key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => app.table.select_next(),
                KeyCode::Up | KeyCode::Char('k') => app.table.select_previous(),
                KeyCode::Char('q') | KeyCode::Esc => quit_or_cancel(app),
                _ if ctrl_c => quit_or_cancel(app),
                _ => {}
            }
            if app.all_done && app.cancelling() {
                return Ok(());
            }
        }
        app.tick += 1;
    }
}

fn quit_or_cancel(app: &mut App) {
    // Once everything is finished, quitting is just leaving; the cancel flag
    // doubles as the "leave now" signal checked by the event loop.
    app.cancel.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::Format;
    use crate::probe::SourceInfo;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Renders the interface mid-batch into an in-memory screen.
    #[test]
    fn renders_mid_batch() {
        let cfg = Config::load(None).unwrap();
        let src = SourceInfo {
            duration: 30.0,
            width: 1920,
            height: 1080,
            fps: 24.0,
            audio_codec: Some("aac".into()),
        };
        let video = |name: &str, source: Result<SourceInfo, String>| Video {
            path: format!("videos/{name}.mp4").into(),
            name: name.into(),
            out_dir: format!("out/{name}").into(),
            source,
        };
        let videos = vec![
            video("holiday", Ok(src.clone())),
            video("concert", Ok(src.clone())),
            video("broken", Err("invalid data".into())),
            video("lecture", Ok(src.clone())),
        ];
        let mut jobs = Vec::new();
        for (i, v) in videos.iter().enumerate() {
            if v.source.is_ok() {
                for format in [Format::Hls, Format::Dash] {
                    jobs.push(Job {
                        video: i,
                        format,
                        args: vec![],
                        manifest: v.out_dir.join("x"),
                    });
                }
            }
        }
        let mut app = App::new(
            "videos".into(),
            4,
            cfg,
            videos,
            jobs,
            vec![],
            Arc::new(AtomicBool::new(false)),
        );
        for i in 0..4 {
            app.handle(Event::JobStarted(i));
        }
        app.handle(Event::JobFinished {
            job: 0,
            outcome: Outcome::Done,
        });
        for (job, seconds, speed) in [(1, 21.0, 1.4), (2, 9.0, 1.3), (3, 4.5, 1.2)] {
            app.handle(Event::Progress {
                job,
                seconds,
                speed: Some(speed),
            });
            app.sample_speed();
        }
        app.handle(Event::Log {
            job: 2,
            line: "[hls @ 0x55] Opening 'out/concert/hls/720p/seg_002.ts' for writing".into(),
        });

        let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        println!("{}", terminal.backend());
    }
}
